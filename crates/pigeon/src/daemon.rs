//! The groups of this machine: one engine per group, started from each
//! group's folders, restarted from them when the user reloads their
//! configurations or applies one, and created when the user creates or
//! joins a group, which then waits for the group's verdict on the member's
//! name, and why each group that does not start does not; which group a
//! call names; the groups heard before joining, to show the names one may
//! join under; and why the daemon stops, which a restart onto a newly
//! installed program is one reason for.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use pigeon_core::name::MemberName;
use pigeon_core::selection::Rule;
use pigeon_store::config::{Config, ConfigFile};
use pigeon_store::group_dirs::{GroupDirs, write_private};
use pigeon_store::group_key::GroupKey;
use pigeon_sync::{Amount, Delta, Engine, JoinState, Listener, Names, Options};
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock, RwLockReadGuard, watch};

use crate::catalog::GROUP;
use crate::config_preview::{self, Freed};
use crate::home::Home;
use crate::program::Program;
use crate::shared_root::{create_root, shared_root};

/// How long, beyond the time a new machine listens, joining waits for the
/// group's verdict on the name.
const VERDICT: Duration = Duration::from_secs(30);

/// How long hearing a group before joining it waits for one of its
/// machines.
const HEARING: Duration = Duration::from_secs(30);

/// The text of the configuration of the group `dirs` once `member` claims
/// it, following their own folder too.
fn claimed(dirs: &GroupDirs, member: MemberName) -> Result<String> {
    let mut config = dirs.load_config()?;
    config.selection.set(Rule::follow_own_folder(&member))?;
    config.member = member;
    config
        .render()
        .map_err(|reason| anyhow!("{}: {reason}", dirs.config_path().display()))
}

/// The error for the group `name`, which this machine is not in.
fn absent(name: &str) -> anyhow::Error {
    anyhow!("no group {name} on this machine: see `pigeon group list`")
}

/// The groups of this machine: the engine of each one running, and why
/// each of the others does not start.
#[derive(Default)]
pub struct Groups {
    running: BTreeMap<String, Engine>,
    failed: BTreeMap<String, String>,
}

impl Groups {
    /// The running groups, by name.
    #[must_use]
    pub fn running(&self) -> &BTreeMap<String, Engine> {
        &self.running
    }

    /// The groups that do not start, by name, each with why.
    #[must_use]
    pub fn failed(&self) -> &BTreeMap<String, String> {
        &self.failed
    }

    /// Keeps the group `name` as it `started`, running or failing to;
    /// gives back why when it does not start.
    fn keep(&mut self, name: String, started: Result<Engine>) -> Result<()> {
        match started {
            Ok(engine) => {
                self.failed.remove(&name);
                self.running.insert(name, engine);
                Ok(())
            }
            Err(error) => {
                self.failed.insert(name, format!("{error:#}"));
                Err(error)
            }
        }
    }

    /// Forgets the group `name`, giving back its engine if it runs.
    fn remove(&mut self, name: &str) -> Option<Engine> {
        self.failed.remove(name);
        self.running.remove(name)
    }

    /// The group a call names, or the machine's only group, running or
    /// not.
    ///
    /// # Errors
    ///
    /// Fails, naming the command to run, if the call names none on a
    /// machine with no group or several.
    pub fn name<'a>(&'a self, name: Option<&'a str>) -> Result<&'a str> {
        if let Some(name) = name {
            return Ok(name);
        }
        let names: BTreeSet<&str> = self
            .running
            .keys()
            .chain(self.failed.keys())
            .map(String::as_str)
            .collect();
        let mut all = names.iter();
        match (all.next(), all.next()) {
            (Some(name), None) => Ok(name),
            (None, _) => {
                bail!(
                    "this machine is in no group: run `pigeon group create` or `pigeon group join`"
                )
            }
            (Some(_), Some(_)) => bail!(
                "this machine is in several groups: pass --{} with one of {}",
                GROUP.name,
                names.into_iter().collect::<Vec<_>>().join(", ")
            ),
        }
    }

    /// The group a call names, or the machine's only group, running or
    /// not, if it is on this machine.
    ///
    /// # Errors
    ///
    /// Fails, naming the command to run, if the group is not on this
    /// machine, or the call names none on a machine with no group or
    /// several.
    pub fn known<'a>(&'a self, name: Option<&'a str>) -> Result<&'a str> {
        let name = self.name(name)?;
        if self.running.contains_key(name) || self.failed.contains_key(name) {
            return Ok(name);
        }
        Err(absent(name))
    }

    /// The running group a call names, or the machine's only group.
    ///
    /// # Errors
    ///
    /// Fails, naming the command to run, if the group is unknown or does
    /// not start, telling why, or the call names none on a machine with no
    /// group or several.
    pub fn choose(&self, name: Option<&str>) -> Result<(&str, &Engine)> {
        let name = self.name(name)?;
        if let Some((name, engine)) = self.running.get_key_value(name) {
            return Ok((name.as_str(), engine));
        }
        match self.failed.get(name) {
            Some(why) => bail!(
                "the group {name} does not start: {why}; mend its files, then run `pigeon daemon reload`, or take it off this machine with `pigeon group leave --{} {name}`",
                GROUP.name
            ),
            None => Err(absent(name)),
        }
    }
}

/// Stops `engine` of the group `name`, telling rather than failing when it
/// does not stop cleanly, since what follows never depends on it.
async fn shut_down(name: &str, engine: Engine) {
    if let Err(error) = engine.shutdown().await {
        eprintln!("pigeon: group {name} did not stop cleanly: {error:#}");
    }
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
    groups: RwLock<Groups>,
    listeners: Mutex<BTreeMap<String, Listener>>,
    program: Program,
    stop: watch::Sender<Option<Stop>>,
}

impl Daemon {
    /// Starts every group of `home`. A group that fails to start is kept
    /// with why, which the calls naming it and the list of groups tell,
    /// and skipped, so that it never stops the others.
    ///
    /// # Errors
    ///
    /// Fails if the groups folder or the running program cannot be read.
    pub async fn start(home: Home, options: Options) -> Result<Self> {
        let program = Program::running()?;
        let mut groups = Groups::default();
        for name in home.group_names()? {
            let started = Engine::start(&home.group(&name), options.clone()).await;
            if let Err(error) = groups.keep(name.clone(), started) {
                eprintln!("pigeon: group {name} does not start: {error:#}");
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

    /// The groups of this machine.
    pub async fn groups(&self) -> RwLockReadGuard<'_, Groups> {
        self.groups.read().await
    }

    /// Creates the group `name` with this machine as its first, and joins
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
    fn not_in(&self, groups: &Groups, name: &str) -> Result<()> {
        if groups.running.contains_key(name)
            || self.home.group_names()?.iter().any(|group| group == name)
        {
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
        dirs.save_secrets(&secrets)?;
        ConfigFile::create(&dirs, Config::new(member, root))?;
        let key = match Engine::start(&dirs, self.options.clone()).await {
            Ok(engine) => {
                let key = engine.group_key();
                groups.running.insert(name.clone(), engine);
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
                groups.choose(Some(group))?.1.status()
            };
            match status.join {
                JoinState::Pending if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                JoinState::Pending | JoinState::Joined => return Ok(()),
                JoinState::Taken(reason) => bail!(
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
        let status = groups.choose(Some(group))?.1.status();
        if status.join == JoinState::Joined {
            bail!("{} has already joined {group}", status.member);
        }
        let text = claimed(&self.home.group(group), member)?;
        self.restart_from(&mut groups, group, &text).await?;
        drop(groups);
        self.verdict(group).await
    }

    /// Makes this machine leave `group`, even one that does not start: it
    /// stops syncing it and forgets its configuration, key, secrets and
    /// state, keeping its files in the root. The group keeps the member's
    /// name and every version.
    ///
    /// # Errors
    ///
    /// Fails if the group is not on this machine or its folders cannot be
    /// removed.
    pub async fn leave(&self, group: &str) -> Result<()> {
        let mut groups = self.groups.write().await;
        self.require_present(&groups, group)?;
        let running = groups.remove(group);
        if let Some(engine) = running {
            shut_down(group, engine).await;
        }
        self.home.group(group).remove()?;
        Ok(())
    }

    /// Fails unless `group` is on this machine, running or not.
    fn require_present(&self, groups: &Groups, group: &str) -> Result<()> {
        if groups.running.contains_key(group)
            || self.home.group_names()?.iter().any(|name| name == group)
        {
            return Ok(());
        }
        Err(absent(group))
    }

    /// Whether no other machine is known to hold the history of `group`,
    /// which leaving it would then lose for good.
    ///
    /// # Errors
    ///
    /// Fails if the group is not on this machine.
    pub async fn holds_only_copy(&self, group: &str) -> Result<bool> {
        let groups = self.groups.read().await;
        self.require_present(&groups, group)?;
        let Some(engine) = groups.running.get(group) else {
            return Ok(false);
        };
        let me = engine.machine();
        Ok(engine
            .members()
            .iter()
            .all(|member| member.machines.iter().all(|machine| *machine == me)))
    }

    /// Restarts every group from its folders, so that the edits of its
    /// `config.toml` apply; starts the groups added there and stops those
    /// gone. A group whose configuration does not read is left as it is,
    /// running or not, and the rest restart. Unless `yes`, changes nothing
    /// if the edits free space on this machine. Returns, for each group
    /// running before, what its edits download, free and pin.
    ///
    /// # Errors
    ///
    /// Fails, naming the file and what in it is wrong, for each
    /// configuration that is invalid, naming what the edits free unless
    /// `yes`, or naming the groups that do not start again.
    pub async fn reload(&self, yes: bool) -> Result<Vec<Value>> {
        let mut groups = self.groups.write().await;
        let names = self.home.group_names()?;
        let mut changes = Vec::new();
        let mut freed = Vec::new();
        let mut left = BTreeSet::new();
        let mut problems = Vec::new();
        for name in &names {
            match self.reading(&groups, name).await {
                Ok(Some((amount, change))) => {
                    freed.push((name.clone(), amount));
                    changes.push(change);
                }
                Ok(None) => {}
                Err(error) => {
                    problems.push(format!("{name} is left as it is: {error:#}"));
                    left.insert(name.clone());
                }
            }
        }
        Freed::new(freed).refuse_unless(yes)?;
        let stopping: Vec<String> = groups
            .running
            .keys()
            .filter(|name| !left.contains(*name))
            .cloned()
            .collect();
        for name in stopping {
            if let Some(engine) = groups.running.remove(&name) {
                shut_down(&name, engine).await;
            }
        }
        for name in names.into_iter().filter(|name| !left.contains(name)) {
            let started = Engine::start(&self.home.group(&name), self.options.clone()).await;
            if let Err(error) = groups.keep(name.clone(), started) {
                problems.push(format!("{name} does not start: {error:#}"));
            }
        }
        if !problems.is_empty() {
            bail!("{}", problems.join("; "));
        }
        Ok(changes)
    }

    /// What reloading would do to the group `name` if it runs, as what its
    /// edits free and, as a result, what they download, free and pin.
    ///
    /// # Errors
    ///
    /// Fails if the configuration of the group does not read.
    async fn reading(&self, groups: &Groups, name: &str) -> Result<Option<(Amount, Value)>> {
        let dirs = self.home.group(name);
        dirs.load_config()?;
        let Some(engine) = groups.running.get(name) else {
            return Ok(None);
        };
        let path = dirs.config_path();
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let (_, preview) = config_preview::plan(engine, &text)
            .await
            .with_context(|| path.display().to_string())?;
        let mut change = config_preview::summary(&preview);
        change["group"] = json!(name);
        Ok(Some((config_preview::total(&preview, Delta::Free), change)))
    }

    /// Writes `text` as the `config.toml` of `group` and restarts the
    /// group from it, unless the text is no configuration, the file is no
    /// longer at `version` when one is given, or, unless `yes`, the text
    /// frees space on this machine.
    ///
    /// # Errors
    ///
    /// Fails saying which of these refuses it, or if the file cannot be
    /// written or the group does not start again.
    pub async fn apply_config(
        &self,
        group: &str,
        text: &str,
        version: Option<&str>,
        yes: bool,
    ) -> Result<()> {
        let mut groups = self.groups.write().await;
        let path = self.home.group(group).config_path();
        let current = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        if version.is_some_and(|version| version != config_preview::version(&current)) {
            bail!(
                "{} changed since you read it: read it again, then apply your edits to it",
                path.display()
            );
        }
        if !groups.failed.contains_key(group) {
            let (_, engine) = groups.choose(Some(group))?;
            let (_, preview) = config_preview::plan(engine, text).await?;
            Freed::by(group, &preview).refuse_unless(yes)?;
        }
        self.restart_from(&mut groups, group, text).await
    }

    /// Restarts the group `group`, running or failing to start, from
    /// `text`, written as its `config.toml`. A group that does not start
    /// from it gets back the configuration it had and starts from that,
    /// so that a running group keeps running.
    ///
    /// # Errors
    ///
    /// Fails, telling why, if the group does not start from `text`, or
    /// does not even start again from the configuration it had.
    async fn restart_from(&self, groups: &mut Groups, group: &str, text: &str) -> Result<()> {
        let dirs = self.home.group(group);
        let path = dirs.config_path();
        let previous = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let engine = groups.running.remove(group);
        let was_running = engine.is_some();
        if let Some(engine) = engine {
            shut_down(group, engine).await;
        } else if !groups.failed.contains_key(group) {
            return Err(absent(group));
        }
        let started = match write_private(&path, text.as_bytes()) {
            Ok(()) => Engine::start(&dirs, self.options.clone()).await,
            Err(error) => Err(error.into()),
        };
        let error = match started {
            Ok(engine) => {
                groups.keep(group.to_owned(), Ok(engine))?;
                return Ok(());
            }
            Err(error) => error,
        };
        let restored = write_private(&path, previous.as_bytes()).map_err(anyhow::Error::from);
        let started = match restored {
            Ok(()) => Engine::start(&dirs, self.options.clone()).await,
            Err(error) => Err(error),
        };
        let kept = groups.keep(group.to_owned(), started);
        if was_running {
            kept?;
        }
        let outcome = if was_running {
            "so it runs on with the one it had"
        } else {
            "so its file stays as it was"
        };
        Err(error.context(format!(
            "{group} does not start from this configuration, {outcome}"
        )))
    }

    /// Stops every group.
    pub async fn shutdown(self) {
        for (name, listener) in self.listeners.into_inner() {
            if let Err(error) = listener.shutdown().await {
                eprintln!("pigeon: hearing {name} did not stop cleanly: {error:#}");
            }
        }
        for (name, engine) in self.groups.into_inner().running {
            shut_down(&name, engine).await;
        }
    }
}
