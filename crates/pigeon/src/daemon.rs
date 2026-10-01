//! The groups running on this machine: one engine per group, started from
//! each group's folders, restarted from them when the user reloads their
//! configurations, and created when the user founds or joins a group,
//! which then waits for the group's verdict on the member's name; the
//! groups heard before joining, to show the names one may join under; and
//! why the daemon stops, which a restart onto a newly installed program is
//! one reason for.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use pigeon_core::clock::ntp_time;
use pigeon_core::name::MemberName;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_store::config::{Config, ConfigFile};
use pigeon_store::group_dirs::GroupDirs;
use pigeon_store::group_key::GroupKey;
use pigeon_store::legacy;
use pigeon_sync::{Engine, JoinState, Listener, Names, Options};
use tokio::sync::{Mutex, RwLock, RwLockReadGuard, watch};

use crate::home::Home;
use crate::program::Program;
use crate::shared_root::{create_root, shared_root};

/// How long, beyond the time a new machine listens, joining waits for the
/// group's verdict on the name.
const VERDICT: Duration = Duration::from_secs(30);

/// How long hearing a group before joining it waits for one of its
/// machines.
const HEARING: Duration = Duration::from_secs(30);

/// The configuration of `data` once `member` claims it, following their
/// personal folder.
fn claimed(dirs: &GroupDirs, member: MemberName) -> Result<Config> {
    let mut config = dirs.load_config(ntp_time(SystemTime::now()))?;
    config.selection.set(Rule {
        pattern: format!("{}/", member.tag()),
        cutoff: Cutoff::PlusInfinity,
    })?;
    config.member = member;
    Ok(config)
}

/// Why the daemon stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Quit,
    /// To run the program now installed where the running one came from.
    Restart,
}

/// Every group on this machine.
pub struct Daemon {
    home: Home,
    options: Options,
    groups: RwLock<BTreeMap<String, Engine>>,
    listeners: Mutex<BTreeMap<String, Listener>>,
    program: Program,
    stop: watch::Sender<Option<Stop>>,
}

impl Daemon {
    /// Starts every group of `home`, upgrading first those an older pigeon
    /// wrote. A group that fails to upgrade or start is reported and
    /// skipped, so that it never stops the others.
    ///
    /// # Errors
    ///
    /// Fails if the groups folder or the running program cannot be read.
    pub async fn start(home: Home, options: Options) -> Result<Self> {
        let program = Program::running()?;
        for name in home.folder_names()? {
            if let Err(error) = legacy::upgrade(&home.group(&name)) {
                eprintln!("pigeon: group {name} does not upgrade: {error:#}");
            }
        }
        let mut groups = BTreeMap::new();
        for name in home.group_names()? {
            match Engine::start(&home.group(&name), options.clone()).await {
                Ok(engine) => {
                    groups.insert(name, engine);
                }
                Err(error) => eprintln!("pigeon: group {name} does not start: {error:#}"),
            }
        }
        Ok(Self {
            home,
            options,
            groups: RwLock::new(groups),
            listeners: Mutex::new(BTreeMap::new()),
            program,
            stop: watch::Sender::new(None),
        })
    }

    /// The program this daemon runs.
    #[must_use]
    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Asks the daemon to stop for `stop`, unless it already stops.
    pub fn stop(&self, stop: Stop) {
        self.stop.send_if_modified(|held| {
            let first = held.is_none();
            if first {
                *held = Some(stop);
            }
            first
        });
    }

    /// Why the daemon stops, once asked to.
    #[must_use]
    pub fn stopping(&self) -> watch::Receiver<Option<Stop>> {
        self.stop.subscribe()
    }

    /// Asks the daemon to restart onto the program now installed where the
    /// running one came from, unless it is the same; returns whether it
    /// restarts.
    ///
    /// # Errors
    ///
    /// Fails if the program file cannot be read.
    pub fn restart(&self) -> Result<bool> {
        let replaced = self.program.replaced()?;
        if replaced {
            self.stop(Stop::Restart);
        }
        Ok(replaced && *self.stop.borrow() == Some(Stop::Restart))
    }

    #[must_use]
    pub fn home(&self) -> &Home {
        &self.home
    }

    /// The running groups, by name.
    pub async fn groups(&self) -> RwLockReadGuard<'_, BTreeMap<String, Engine>> {
        self.groups.read().await
    }

    /// Founds the group `name` with this machine as its first, and joins
    /// it as `member`; returns the group key to share.
    ///
    /// # Errors
    ///
    /// Fails if a name is invalid, the group exists here, or it does not
    /// start.
    pub async fn create(&self, name: &str, member: &str, root: Option<PathBuf>) -> Result<String> {
        let name = MemberName::parse(name).context("the group name")?;
        let dirs = self.home.group(name.as_str());
        let machine = dirs.secrets()?.machine;
        let key = GroupKey::generate(name, vec![machine.public()]);
        self.add(key, member, root).await
    }

    /// Joins the group `key` admits as `member`; returns the group key.
    ///
    /// # Errors
    ///
    /// Fails if the key or name is invalid, the group exists here, it does
    /// not start, or it refuses the name.
    pub async fn join(&self, key: &str, member: &str, root: Option<PathBuf>) -> Result<String> {
        let key: GroupKey = key.parse().context("the group key")?;
        self.add(key, member, root).await
    }

    /// Fails, naming the command that shows it, if this machine is in the
    /// group `name`.
    fn not_in(&self, groups: &BTreeMap<String, Engine>, name: &str) -> Result<()> {
        if groups.contains_key(name) || self.home.group_names()?.iter().any(|group| group == name) {
            bail!(
                "this machine is already in the group {name}: see `pigeon group status --group {name}`"
            );
        }
        Ok(())
    }

    /// Hears the group `key` admits without joining it, waiting a while
    /// for one of its machines, and returns the names one may join under.
    ///
    /// # Errors
    ///
    /// Fails if the key is invalid, this machine is in the group, or the
    /// group cannot be heard.
    pub async fn hear(&self, key: &str) -> Result<Names> {
        let key: GroupKey = key.parse().context("the group key")?;
        let name = key.name.to_string();
        self.not_in(&*self.groups.read().await, &name)?;
        let mut listeners = self.listeners.lock().await;
        if listeners
            .get(&name)
            .is_some_and(|listener| *listener.key() != key)
            && let Some(listener) = listeners.remove(&name)
        {
            listener.shutdown().await?;
        }
        if !listeners.contains_key(&name) {
            let listener = Listener::start(&self.home.group(&name), key, &self.options).await?;
            listeners.insert(name.clone(), listener);
        }
        let mut heard = listeners[&name].heard();
        drop(listeners);
        let _ = tokio::time::timeout(HEARING, heard.wait_for(|heard| *heard)).await;
        let listeners = self.listeners.lock().await;
        let listener = listeners
            .get(&name)
            .ok_or_else(|| anyhow!("this machine joined {name} meanwhile"))?;
        Ok(listener.names())
    }

    async fn add(&self, key: GroupKey, member: &str, root: Option<PathBuf>) -> Result<String> {
        let member = MemberName::parse(member).context("the member name")?;
        let name = key.name.to_string();
        let mut groups = self.groups.write().await;
        self.not_in(&groups, &name)?;
        if let Some(listener) = self.listeners.lock().await.remove(&name) {
            listener.shutdown().await?;
        }
        let root = root.unwrap_or_else(|| shared_root(&name));
        create_root(&root)?;
        let dirs = self.home.group(&name);
        let mut secrets = dirs.secrets()?;
        secrets.key = Some(key);
        secrets.renewal = None;
        dirs.save_secrets(&secrets)?;
        ConfigFile::create(&dirs, Config::new(member, root))?;
        let key = match Engine::start(&dirs, self.options.clone()).await {
            Ok(engine) => {
                let key = engine.group_key();
                groups.insert(name.clone(), engine);
                key
            }
            Err(error) => {
                let _ = dirs.remove();
                return Err(error);
            }
        };
        drop(groups);
        self.verdict(&name).await?;
        Ok(key)
    }

    /// Waits until the group accepts or refuses the name of this machine's
    /// member in `group`, or a while longer than a new machine listens,
    /// and fails, naming the command to run next, if it refuses.
    async fn verdict(&self, group: &str) -> Result<()> {
        let deadline = tokio::time::Instant::now() + self.options.join_delay + VERDICT;
        loop {
            let status = {
                let groups = self.groups.read().await;
                let engine = groups.get(group).ok_or_else(|| {
                    anyhow!("no group {group} on this machine: see `pigeon group list`")
                })?;
                engine.status().await
            };
            match status.join {
                JoinState::Pending if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                JoinState::Pending | JoinState::Joined => return Ok(()),
                JoinState::Taken(reason) | JoinState::Excluded(reason) => bail!(
                    "{reason}: claim another name with `pigeon member claim --group {group} --member <name>`"
                ),
            }
        }
    }

    /// Makes this machine claim the name `member` in `group` after losing
    /// its previous claim, and follows the name's personal folder.
    ///
    /// # Errors
    ///
    /// Fails if the group is unknown, the member has already joined, the
    /// name or the configuration is invalid, the group does not restart,
    /// or it refuses the name.
    pub async fn claim(&self, group: &str, member: &str) -> Result<()> {
        let member = MemberName::parse(member).context("the member name")?;
        let mut groups = self.groups.write().await;
        let engine = groups
            .remove(group)
            .ok_or_else(|| anyhow!("no group {group} on this machine: see `pigeon group list`"))?;
        let status = engine.status().await;
        let dirs = self.home.group(group);
        let claimed = if status.join == JoinState::Joined {
            Err(anyhow!("{} has already joined {group}", status.member))
        } else {
            claimed(&dirs, member)
        };
        let config = match claimed {
            Ok(config) => config,
            Err(error) => {
                groups.insert(group.to_owned(), engine);
                return Err(error);
            }
        };
        engine.shutdown().await?;
        ConfigFile::create(&dirs, config)?;
        let engine = Engine::start(&dirs, self.options.clone()).await?;
        groups.insert(group.to_owned(), engine);
        drop(groups);
        self.verdict(group).await
    }

    /// Restarts every group from its folders, so that the edits of
    /// its `config.toml` apply; starts the groups added there and stops
    /// those gone. Changes nothing unless every configuration reads.
    /// Returns the groups running.
    ///
    /// # Errors
    ///
    /// Fails, naming the file and what in it is wrong, if a configuration
    /// is invalid, or naming the groups that do not start again.
    pub async fn reload(&self) -> Result<Vec<String>> {
        let mut groups = self.groups.write().await;
        let names = self.home.group_names()?;
        let now = ntp_time(SystemTime::now());
        for name in &names {
            self.home.group(name).load_config(now)?;
        }
        for (name, engine) in std::mem::take(&mut *groups) {
            if let Err(error) = engine.shutdown().await {
                eprintln!("pigeon: group {name} did not stop cleanly: {error:#}");
            }
        }
        let mut failed = Vec::new();
        for name in names {
            match Engine::start(&self.home.group(&name), self.options.clone()).await {
                Ok(engine) => {
                    groups.insert(name, engine);
                }
                Err(error) => failed.push(format!("{name} does not start: {error:#}")),
            }
        }
        if !failed.is_empty() {
            bail!("{}", failed.join("; "));
        }
        Ok(groups.keys().cloned().collect())
    }

    /// Stops every group.
    pub async fn shutdown(self) {
        for (name, listener) in self.listeners.into_inner() {
            if let Err(error) = listener.shutdown().await {
                eprintln!("pigeon: hearing {name} did not stop cleanly: {error:#}");
            }
        }
        for (name, engine) in self.groups.into_inner() {
            if let Err(error) = engine.shutdown().await {
                eprintln!("pigeon: group {name} did not stop cleanly: {error:#}");
            }
        }
    }
}
