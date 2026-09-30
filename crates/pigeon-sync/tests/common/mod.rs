//! A group of engines on this host, with short timings, and a way to wait
//! for what they converge to.

#![allow(dead_code)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iroh::address_lookup::MemoryLookup;
use pigeon_core::clock::MachineId;
use pigeon_core::name::MemberName;
use pigeon_store::config::{DataDir, GroupConfig};
use pigeon_store::group_key::GroupKey;
use pigeon_sync::{Engine, JoinState, Network, Options};
use tempfile::TempDir;

pub struct Machine {
    pub engine: Engine,
    pub root: PathBuf,
    pub data: DataDir,
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
        let engine = Engine::start(&self.data, self.options.clone())
            .await
            .unwrap();
        Self { engine, ..self }
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
        ..Options::default()
    }
}

/// Starts one machine per `(member, password)`, all in one group whose key
/// names the first machine, and waits until every member joined.
pub async fn group(members: &[(&str, &str)]) -> Vec<Machine> {
    let lookup = MemoryLookup::new();
    let dirs: Vec<TempDir> = members
        .iter()
        .map(|_| tempfile::tempdir().unwrap())
        .collect();
    let datas: Vec<DataDir> = dirs
        .iter()
        .map(|dir| {
            let path = dir.path().join("data");
            std::fs::create_dir_all(&path).unwrap();
            DataDir::new(path)
        })
        .collect();
    let first: MachineId = datas[0].machine_key().unwrap().public();
    let key = GroupKey::generate(MemberName::parse("friends").unwrap(), vec![first]);
    let mut machines = Vec::new();
    for ((dir, data), (member, password)) in dirs.into_iter().zip(datas).zip(members) {
        let root = dir.path().join("root");
        let config = GroupConfig::join(
            key.clone(),
            MemberName::parse(member).unwrap(),
            password,
            root.clone(),
            &data.machine_key().unwrap(),
        );
        data.save_config(&config).unwrap();
        let options = options(&lookup);
        let engine = Engine::start(&data, options.clone()).await.unwrap();
        machines.push(Machine {
            engine,
            root,
            data,
            options,
            _dir: dir,
        });
    }
    machines
}

/// Waits until every machine's member joined.
pub async fn joined(machines: &[Machine]) {
    for machine in machines {
        eventually("the member joins", || async {
            machine.engine.status().await.join == JoinState::Joined
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
