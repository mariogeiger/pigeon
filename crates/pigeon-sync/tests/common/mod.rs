//! A group of engines on this host, with short timings, and a way to wait
//! for what they converge to.

#![allow(dead_code)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iroh::address_lookup::MemoryLookup;
use pigeon_core::clock::MachineId;
use pigeon_core::name::MemberName;
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
    _dir: TempDir,
}

impl Machine {
    pub fn file(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.file(relative)).ok()
    }

    /// Writes a file as a person would, making it writable first.
    pub fn edit(&self, relative: &str, text: &str) {
        let path = self.file(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if path.exists() {
            make_writable(&path);
        }
        std::fs::write(path, text).unwrap();
    }

    pub async fn restart(self) -> Self {
        self.engine.shutdown().await.unwrap();
        let engine = Engine::start(&self.dirs, self.options.clone())
            .await
            .unwrap();
        Self { engine, ..self }
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
        _dir: dir,
    }
}

pub fn make_writable(path: &Path) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    #[cfg(unix)]
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o644);
    #[cfg(not(unix))]
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions).unwrap();
}

pub fn is_read_only(path: &Path) -> bool {
    std::fs::metadata(path).unwrap().permissions().readonly()
}

pub fn options(lookup: &MemoryLookup) -> Options {
    Options {
        network: Network::Local(lookup.clone()),
        settle_personal: Duration::from_millis(100),
        settle_drop: Duration::from_millis(400),
        rescan: Duration::from_secs(60),
        tick: Duration::from_millis(50),
        join_delay: Duration::from_millis(300),
        gc: Duration::from_millis(300),
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
    let lookup = MemoryLookup::new();
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
        let mut options = options(&lookup);
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
            machine.engine.status().await.join == JoinState::Joined
        })
        .await;
        names.push(machine.engine.status().await.member);
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
