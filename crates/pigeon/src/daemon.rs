//! The groups running on this machine: one engine per group, started from
//! the data directories in the pigeon folder, and created when the user
//! founds or joins a group, which then waits for the group's verdict on
//! the member's name; and why the daemon stops, which a restart onto a
//! newly installed program is one reason for.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use pigeon_core::name::MemberName;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_store::config::GroupConfig;
use pigeon_store::group_key::GroupKey;
use pigeon_sync::{Engine, JoinState, Options};
use tokio::sync::{RwLock, RwLockReadGuard, watch};

use crate::home::Home;
use crate::program::Program;
use crate::shared_root::{create_root, shared_root};

/// How long, beyond the time a new machine listens, joining waits for the
/// group's verdict on the name.
const VERDICT: Duration = Duration::from_secs(30);

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
    program: Program,
    stop: watch::Sender<Option<Stop>>,
}

impl Daemon {
    /// Starts every group of `home`. A group that fails to start is
    /// reported and skipped, so that it never stops the others.
    ///
    /// # Errors
    ///
    /// Fails if the groups folder or the running program cannot be read.
    pub async fn start(home: Home, options: Options) -> Result<Self> {
        let program = Program::running()?;
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
        let data = self.home.group(name.as_str());
        let machine = data.machine_key()?;
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

    async fn add(&self, key: GroupKey, member: &str, root: Option<PathBuf>) -> Result<String> {
        let member = MemberName::parse(member).context("the member name")?;
        let name = key.name.to_string();
        let mut groups = self.groups.write().await;
        if groups.contains_key(&name) || self.home.group_names()?.contains(&name) {
            bail!(
                "this machine is already in the group {name}: see `pigeon group status --group {name}`"
            );
        }
        let root = root.unwrap_or_else(|| shared_root(&name));
        create_root(&root)?;
        let data = self.home.group(&name);
        let machine = data.machine_key()?;
        data.save_config(&GroupConfig::join(key, member, root, &machine))?;
        let key = match Engine::start(&data, self.options.clone()).await {
            Ok(engine) => {
                let key = engine.group_key();
                groups.insert(name.clone(), engine);
                key
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(data.path());
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
    /// name is invalid, the group does not restart, or it refuses the
    /// name.
    pub async fn claim(&self, group: &str, member: &str) -> Result<()> {
        let member = MemberName::parse(member).context("the member name")?;
        let mut groups = self.groups.write().await;
        let engine = groups
            .remove(group)
            .ok_or_else(|| anyhow!("no group {group} on this machine: see `pigeon group list`"))?;
        let status = engine.status().await;
        if status.join == JoinState::Joined {
            groups.insert(group.to_owned(), engine);
            bail!("{} has already joined {group}", status.member);
        }
        engine.shutdown().await?;
        let data = self.home.group(group);
        let config = data.load_config()?;
        let machine = data.machine_key()?;
        data.save_config(&GroupConfig::join(
            config.key,
            member.clone(),
            config.root,
            &machine,
        ))?;
        let engine = Engine::start(&data, self.options.clone()).await?;
        engine
            .set_rule(Rule {
                pattern: format!("{}/", member.folder()),
                cutoff: Cutoff::PlusInfinity,
            })
            .await?;
        groups.insert(group.to_owned(), engine);
        drop(groups);
        self.verdict(group).await
    }

    /// Stops every group.
    pub async fn shutdown(self) {
        for (name, engine) in self.groups.into_inner() {
            if let Err(error) = engine.shutdown().await {
                eprintln!("pigeon: group {name} did not stop cleanly: {error:#}");
            }
        }
    }
}
