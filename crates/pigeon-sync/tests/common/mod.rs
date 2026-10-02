//! A group of engines on this host, with short timings; the paths and
//! rules tests name; and ways to wait for what the engines converge to, or
//! past the time something that must not happen would take.

#![allow(dead_code)]

mod converged;

#[allow(unused_imports)]
pub use converged::converged;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iroh::address_lookup::MemoryLookup;
use pigeon_core::clock::MachineId;
use pigeon_core::name::MemberName;
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_net::Timings;
use pigeon_store::config::{Config, ConfigFile};
use pigeon_store::group_dirs::GroupDirs;
use pigeon_store::group_key::GroupKey;
use pigeon_sync::{Engine, JoinState, Network, Options};
use tempfile::TempDir;

pub struct Machine {
    pub engine: Engine,
    pub root: PathBuf,
    pub dirs: GroupDirs,
    options: Options,
    /// The directory holding the root and the group's directories, which
    /// lives as long as the machine.
    dir: TempDir,
}

impl Machine {
    pub fn file(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.file(relative)).ok()
    }

    /// Makes the machine follow `pattern`.
    pub async fn follow(&self, pattern: &str) {
        self.engine
            .set_rule(rule(pattern, Cutoff::PlusInfinity))
            .await
            .unwrap();
    }

    /// Waits until the edits made before the call had the time to settle
    /// and a pass of the timer came after, for what must not happen by
    /// then: the engine notices them by the end of the first scan begun
    /// after the call, or, when none comes, within the time an edit takes
    /// to settle, by which the watcher reported them.
    pub async fn wait_past_settling(&self) {
        let settle = self.options.settle_personal.max(self.options.settle_draft);
        let scans = self.engine.status().scans;
        let noticed = tokio::time::Instant::now() + settle;
        while self.engine.status().scans < scans + 2 && tokio::time::Instant::now() < noticed {
            tokio::time::sleep(self.options.tick).await;
        }
        tokio::time::sleep(settle).await;
        self.wait_for_ticks(2).await;
    }

    /// Waits until a garbage collection that began after a pass of the
    /// timer recomputed what the engine protects ended, by when a blob
    /// nothing protects is gone.
    pub async fn wait_past_collection(&self) {
        self.wait_for_ticks(2).await;
        let collections = self.engine.status().collections;
        eventually("two garbage collections begin", || async {
            self.engine.status().collections >= collections + 2
        })
        .await;
    }

    /// Waits until `count` passes of the timer ended since the call, the
    /// last of them begun after it.
    pub async fn wait_for_ticks(&self, count: u64) {
        let ticks = self.engine.status().ticks;
        eventually("passes of the timer end", || async {
            self.engine.status().ticks >= ticks + count
        })
        .await;
    }

    /// Writes a file as a person would.
    pub fn edit(&self, relative: &str, text: &str) {
        let path = self.file(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    pub async fn restart(self) -> Self {
        self.restart_after(|_, _| {}).await
    }

    /// Stops the engine, lets `meddle` change its directories and root, as
    /// another version of pigeon would have left them, and starts it again.
    pub async fn restart_after(self, meddle: impl FnOnce(&GroupDirs, &Path)) -> Self {
        let Self {
            engine,
            root,
            dirs,
            options,
            dir,
        } = self;
        engine.shutdown().await.unwrap();
        meddle(&dirs, &root);
        Self {
            engine: Engine::start(&dirs, options.clone()).await.unwrap(),
            root,
            dirs,
            options,
            dir,
        }
    }

    /// Stops the engine and starts it again on the network `lookup` makes,
    /// where it finds only the machines on it.
    pub async fn restart_on(mut self, lookup: &MemoryLookup) -> Self {
        self.options.network = Network::Local(lookup.clone());
        self.restart().await
    }

    /// Stops the engine, lets `meddle` change the disk given the old root,
    /// and starts it again with `root` as its root, as a person editing its
    /// configuration does; why it does not start, if it does not.
    pub async fn restart_in(
        self,
        root: PathBuf,
        meddle: impl FnOnce(&Path),
    ) -> anyhow::Result<Self> {
        let Self {
            engine,
            root: old,
            dirs,
            options,
            dir,
        } = self;
        engine.shutdown().await.unwrap();
        meddle(&old);
        let mut config = dirs.load_config().unwrap();
        config.root = root.clone();
        ConfigFile::create(&dirs, config).unwrap();
        let engine = Engine::start(&dirs, options.clone()).await?;
        Ok(Self {
            engine,
            root,
            dirs,
            options,
            dir,
        })
    }

    /// Starts another machine on the same network, joining with `key`.
    pub async fn join_with(&self, key: &str, member: &str) -> Machine {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        start(
            key.parse().unwrap(),
            member,
            (dir, root),
            self.options.clone(),
        )
        .await
    }

    /// Starts another machine on the same network, joining with `key`,
    /// whose root is a link to a folder elsewhere, as macOS makes `/name`.
    #[cfg(unix)]
    pub async fn join_through_link(&self, key: &str, member: &str) -> Machine {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("elsewhere");
        std::fs::create_dir(&folder).unwrap();
        let root = dir.path().join("root");
        std::os::unix::fs::symlink(&folder, &root).unwrap();
        start(
            key.parse().unwrap(),
            member,
            (dir, root),
            self.options.clone(),
        )
        .await
    }
}

/// Starts a machine of the member `member`, joining with `key`.
pub async fn start_with(key: GroupKey, member: &str, options: Options) -> Machine {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    start(key, member, (dir, root), options).await
}

async fn start(
    key: GroupKey,
    member: &str,
    (dir, root): (TempDir, PathBuf),
    options: Options,
) -> Machine {
    let dirs = GroupDirs::new(dir.path().join("config"), dir.path().join("data"));
    let mut secrets = dirs.secrets().unwrap();
    secrets.key = Some(key);
    dirs.save_secrets(&secrets).unwrap();
    let config = Config::new(MemberName::parse(member).unwrap(), root.clone());
    ConfigFile::create(&dirs, config).unwrap();
    let engine = Engine::start(&dirs, options.clone()).await.unwrap();
    Machine {
        engine,
        root,
        dirs,
        options,
        dir,
    }
}

/// The group path `text`.
pub fn path(text: &str) -> GroupPath {
    GroupPath::parse(text).unwrap()
}

/// The rule giving the files `pattern` matches `cutoff`.
pub fn rule(pattern: &str, cutoff: Cutoff) -> Rule {
    Rule {
        pattern: pattern.into(),
        cutoff,
    }
}

/// Stops every machine.
pub async fn shut_down(machines: impl IntoIterator<Item = Machine>) {
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

pub fn is_read_only(path: &Path) -> bool {
    std::fs::metadata(path).unwrap().permissions().readonly()
}

pub fn options(lookup: &MemoryLookup) -> Options {
    Options {
        network: Network::Local(lookup.clone()),
        settle_personal: Duration::from_millis(100),
        settle_draft: Duration::from_millis(400),
        rescan: Duration::from_secs(60),
        tick: Duration::from_millis(50),
        join_delay: Duration::from_millis(300),
        gc: Duration::from_millis(300),
        node: Timings {
            recheck: Duration::from_millis(300),
            stall: Duration::from_secs(2),
        },
        ..Options::default()
    }
}

/// Starts one machine per member, all in one group whose key
/// names the first machine.
pub async fn group(members: &[&str]) -> Vec<Machine> {
    group_with(members, |_| {}).await
}

/// The same, with the timings `tune` changes.
pub async fn group_with(members: &[&str], tune: impl Fn(&mut Options)) -> Vec<Machine> {
    group_on(&MemoryLookup::new(), members, tune).await
}

/// The same, on `lookup`.
pub async fn group_on(
    lookup: &MemoryLookup,
    members: &[&str],
    tune: impl Fn(&mut Options),
) -> Vec<Machine> {
    let dirs: Vec<TempDir> = members
        .iter()
        .map(|_| tempfile::tempdir().unwrap())
        .collect();
    let first: MachineId =
        GroupDirs::new(dirs[0].path().join("config"), dirs[0].path().join("data"))
            .secrets()
            .unwrap()
            .machine
            .public();
    let key = GroupKey::generate(MemberName::parse("friends").unwrap(), vec![first]);
    let mut machines = Vec::new();
    for (dir, member) in dirs.into_iter().zip(members) {
        let root = dir.path().join("root");
        let mut options = options(lookup);
        tune(&mut options);
        machines.push(start(key.clone(), member, (dir, root), options).await);
    }
    machines
}

/// Waits until every machine's member joined, as every machine sees it.
pub async fn joined(machines: &[Machine]) {
    let mut names = Vec::new();
    for machine in machines {
        eventually("the member joins", || async {
            machine.engine.status().join == JoinState::Joined
        })
        .await;
        names.push(machine.engine.status().member);
    }
    for machine in machines {
        eventually("every machine knows every member", || async {
            let members = machine.engine.members();
            names
                .iter()
                .all(|name| members.iter().any(|member| member.name == *name))
        })
        .await;
    }
}

/// Waits up to twenty seconds for `condition`.
pub async fn eventually<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
