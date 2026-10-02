//! What the engine shows: its status, with which member owns each machine
//! that runs another version of pigeon and which version, the group's
//! files as this machine holds them, a file's history across its moves,
//! the members with their machines, the selection, the times a pin can
//! choose and the retention, each as plain data for the command line and
//! the API.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use pigeon_core::clock::{MachineId, Stamp, rfc3339};
use pigeon_core::ledger::Version;
use pigeon_core::name::MemberName;
use pigeon_core::patch::{Content, VersionRef};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::retention::Retention;
use pigeon_core::selection::{Cutoff, Rule, compile, matches};
use pigeon_core::statement::is_statement;
use pigeon_net::hello::{Heard, Standing};
use pigeon_store::index::IndexEntry;
use serde::Serialize;

use crate::engine::{Engine, JoinState};

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub member: MemberName,
    pub machine: MachineId,
    pub root: std::path::PathBuf,
    pub join: JoinState,
    pub peers: Vec<MachineId>,
    /// The machines that run another version of pigeon, whose protocol
    /// this one does not speak.
    pub incompatible: Vec<IncompatibleMachine>,
    /// The relay that carries what no direct connection can, once reached.
    pub relay: Option<String>,
    /// Edits waiting to settle.
    pub pending: usize,
    /// Blobs being fetched.
    pub fetching: usize,
    pub patches: usize,
    /// The latest errors, oldest first.
    pub errors: Vec<String>,
}

/// A machine that runs another version of pigeon, the member whose
/// patches it signed, if any reached this one, and which version it runs,
/// as far as it told.
#[derive(Clone, Debug, Serialize)]
pub struct IncompatibleMachine {
    pub machine: MachineId,
    pub member: Option<MemberName>,
    pub standing: Standing,
    pub version: Option<String>,
    pub commit: Option<String>,
    pub protocol: Option<String>,
}

impl IncompatibleMachine {
    fn new(machine: MachineId, member: Option<MemberName>, heard: &Heard) -> Self {
        let announced = match heard {
            Heard::Announced(announcement) => Some(announcement),
            Heard::PreHello | Heard::Unheard => None,
        };
        Self {
            machine,
            member,
            standing: Standing::of(heard),
            version: announced.map(|told| told.version.clone()),
            commit: announced.map(|told| told.commit.clone()),
            protocol: announced.map(|told| told.protocol.clone()),
        }
    }
}

/// One file of the group as this machine sees it.
#[derive(Clone, Debug, Serialize)]
pub struct FileView {
    pub path: GroupPath,
    /// The member whose personal path holds the file, if any.
    pub owner: Option<MemberName>,
    /// The member who made its current version.
    pub author: MemberName,
    pub content: Content,
    pub stamp: Stamp,
    pub time: String,
    pub cutoff: Cutoff,
    /// Whether the disk shows some version of the file.
    pub held: bool,
    /// Whether the disk shows a version other than the current one.
    pub outdated: bool,
}

/// A time at which some files a pattern matches have a version, and how
/// many: pinning the pattern holds something else at each such time and
/// the same between two of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PinTime {
    pub time: String,
    pub files: usize,
}

/// One accepted version of a file.
#[derive(Clone, Debug, Serialize)]
pub struct VersionView {
    pub stamp: Stamp,
    pub time: String,
    pub path: GroupPath,
    pub content: Option<Content>,
    pub author: MemberName,
    pub replaces: Option<Stamp>,
    /// The version it moved, at another path.
    pub continues: Option<VersionRef>,
}

impl From<&Version> for VersionView {
    fn from(version: &Version) -> Self {
        Self {
            stamp: version.stamp,
            time: version.stamp.rfc3339(),
            path: version.path.clone(),
            content: version.content,
            author: version.author.clone(),
            replaces: version.replaces,
            continues: version.continues.clone(),
        }
    }
}

/// A member, with the key their name is bound to, the machines that
/// signed patches for them, and which of those this one talks to now,
/// itself included.
#[derive(Clone, Debug, Serialize)]
pub struct MemberView {
    pub name: MemberName,
    pub key: iroh_base::PublicKey,
    pub joined: String,
    pub machines: Vec<MachineId>,
    pub online: Vec<MachineId>,
}

impl Engine {
    /// # Panics
    ///
    /// Panics if a panic poisoned the error list.
    pub async fn status(&self) -> Status {
        let inner = &self.inner;
        let work = inner.work.lock().await;
        let ledger = inner.ledger.lock();
        let owners: HashMap<MachineId, &MemberName> = ledger
            .patches()
            .map(|patch| (patch.cert.machine, &patch.cert.name))
            .collect();
        Status {
            member: inner.member.clone(),
            machine: inner.me(),
            root: inner.root.clone(),
            join: work.join.clone(),
            peers: inner.node.peers(),
            incompatible: inner
                .node
                .incompatible()
                .into_iter()
                .map(|(machine, heard)| {
                    let member = owners.get(&machine).map(|name| (*name).clone());
                    IncompatibleMachine::new(machine, member, &heard)
                })
                .collect(),
            relay: inner.node.home_relay().map(|url| url.to_string()),
            pending: work.pending.len(),
            fetching: work.fetching.len(),
            patches: ledger.patches().count(),
            errors: inner
                .errors
                .lock()
                .expect("no panic holds the errors")
                .iter()
                .cloned()
                .collect(),
        }
    }

    /// The group key, which lets another member's machine join; it names
    /// this machine among the machines to dial first.
    #[must_use]
    pub fn group_key(&self) -> String {
        let mut key = self.inner.key.clone();
        if !key.bootstrap.contains(&self.inner.me()) {
            key.bootstrap.insert(0, self.inner.me());
        }
        key.to_string()
    }

    /// The time of this machine's clock, never behind a version it has
    /// seen.
    #[must_use]
    pub fn now(&self) -> u64 {
        self.inner.clock.stamp().time
    }

    /// The bytes of `content`, if this machine holds them.
    ///
    /// # Errors
    ///
    /// Fails if the blob store cannot be read.
    pub async fn read(&self, content: &Content) -> Result<Option<Vec<u8>>> {
        let blobs = &self.inner.blobs;
        if !blobs.has(&content.hash).await? {
            return Ok(None);
        }
        Ok(Some(blobs.read(&content.hash).await?))
    }

    /// Every file of the group at `under` or inside it.
    ///
    /// # Errors
    ///
    /// Fails if the index cannot be read.
    pub async fn list(&self, under: Option<&GroupPath>) -> Result<Vec<FileView>> {
        let inner = &self.inner;
        let entries: HashMap<PathKey, IndexEntry> = inner
            .state
            .index(under)?
            .into_iter()
            .map(|entry| (entry.path.key(), entry))
            .collect();
        let work = inner.work.lock().await;
        let ledger = inner.ledger.lock();
        let mut files: Vec<FileView> = ledger
            .live()
            .filter(|version| under.is_none_or(|under| version.path.is_within(under)))
            .filter_map(|version| {
                let content = version.content?;
                let entry = entries.get(&version.path.key());
                let held = entry.is_some_and(|entry| entry.seen.is_some());
                Some(FileView {
                    path: version.path.clone(),
                    owner: ledger.owner(&version.path),
                    author: version.author.clone(),
                    content,
                    stamp: version.stamp,
                    time: version.stamp.rfc3339(),
                    cutoff: work.config.selection.cutoff(&version.path),
                    held,
                    outdated: held && entry.and_then(|entry| entry.synced) != Some(version.stamp),
                })
            })
            .collect();
        files.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
        Ok(files)
    }

    /// The history of the file at `path`, oldest first: every version it
    /// descends from, across the paths it moved from.
    #[must_use]
    pub fn history(&self, path: &GroupPath) -> Vec<VersionView> {
        let ledger = self.inner.ledger.lock();
        let mut history: Vec<VersionView> = ledger
            .lineage(&path.key())
            .into_iter()
            .map(VersionView::from)
            .collect();
        history.reverse();
        history
    }

    /// Every time at which pinning `pattern` holds something new, newest
    /// first.
    ///
    /// # Errors
    ///
    /// Fails if `pattern` is no gitignore pattern.
    pub fn pin_times(&self, pattern: &str) -> Result<Vec<PinTime>> {
        let matcher = compile(pattern)?;
        let ledger = self.inner.ledger.lock();
        let mut files = BTreeMap::<u64, usize>::new();
        for key in ledger.keys() {
            let Some(head) = ledger.head(key) else {
                continue;
            };
            if is_statement(&head.path.key()) || !matches(&matcher, &head.path) {
                continue;
            }
            for version in ledger.versions(key) {
                *files.entry(version.stamp.time).or_default() += 1;
            }
        }
        Ok(files
            .into_iter()
            .rev()
            .map(|(time, files)| PinTime {
                time: rfc3339(time),
                files,
            })
            .collect())
    }

    #[must_use]
    pub fn members(&self) -> Vec<MemberView> {
        let inner = &self.inner;
        let mut online: BTreeSet<MachineId> = inner.node.peers().into_iter().collect();
        online.insert(inner.me());
        let ledger = inner.ledger.lock();
        let mut machines = BTreeMap::<&MemberName, BTreeSet<MachineId>>::new();
        for patch in ledger.patches() {
            machines
                .entry(&patch.cert.name)
                .or_default()
                .insert(patch.cert.machine);
        }
        ledger
            .members()
            .iter()
            .map(|(name, member)| {
                let machines: Vec<MachineId> = machines
                    .get(name)
                    .map(|machines| machines.iter().copied().collect())
                    .unwrap_or_default();
                MemberView {
                    name: name.clone(),
                    key: member.key,
                    joined: member.joined.rfc3339(),
                    online: machines
                        .iter()
                        .copied()
                        .filter(|machine| online.contains(machine))
                        .collect(),
                    machines,
                }
            })
            .collect()
    }

    /// This machine's retention.
    pub async fn retention(&self) -> Retention {
        self.inner.work.lock().await.config.retention
    }

    /// The selection's rules, the last matching one winning.
    pub async fn selection(&self) -> Vec<Rule> {
        self.inner
            .work
            .lock()
            .await
            .config
            .selection
            .rules()
            .cloned()
            .collect()
    }
}
