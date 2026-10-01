//! What the engine shows: its status, the group's files as this machine
//! holds them, a file's history, the members, the requests, the set-aside
//! list, the selection and the retention, each as plain data for the
//! command line and the API.

use std::collections::HashMap;

use anyhow::Result;
use pigeon_core::clock::{MachineId, Stamp};
use pigeon_core::ledger::Version;
use pigeon_core::name::MemberName;
use pigeon_core::patch::Content;
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::retention::Retention;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_core::statement::{Decision, RequestStatement};
use pigeon_store::aside::AsideItem;
use pigeon_store::index::IndexEntry;
use serde::Serialize;

use crate::engine::{Engine, JoinState};
use crate::statements::requests_folder;

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub member: MemberName,
    pub machine: MachineId,
    pub root: std::path::PathBuf,
    pub join: JoinState,
    pub peers: Vec<MachineId>,
    /// The machines that run another version of pigeon, whose protocol
    /// this one does not speak.
    pub incompatible: Vec<MachineId>,
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

/// One file of the group as this machine sees it.
#[derive(Clone, Debug, Serialize)]
pub struct FileView {
    pub path: GroupPath,
    pub owner: MemberName,
    pub content: Content,
    pub stamp: Stamp,
    pub time: String,
    pub cutoff: Cutoff,
    /// Whether the disk shows some version of the file.
    pub held: bool,
    /// Whether the disk shows a version other than the current one.
    pub outdated: bool,
    pub writable: bool,
}

/// An edit waiting to stay unchanged long enough to be published.
#[derive(Clone, Debug, Serialize)]
pub struct PendingView {
    pub path: GroupPath,
    /// Whether the edit deletes the file.
    pub deleted: bool,
    /// Seconds until it is published, if it stays unchanged.
    pub due_in: u64,
    /// Whether publishing freezes the file, as in a drop folder.
    pub freezes: bool,
    /// The cutoff the selection gives the file.
    pub cutoff: Cutoff,
}

/// One accepted version of a file.
#[derive(Clone, Debug, Serialize)]
pub struct VersionView {
    pub stamp: Stamp,
    pub time: String,
    pub path: GroupPath,
    pub content: Option<Content>,
    pub owner: MemberName,
    pub replaces: Option<Stamp>,
    pub applies: Option<GroupPath>,
}

impl From<&Version> for VersionView {
    fn from(version: &Version) -> Self {
        Self {
            stamp: version.stamp,
            time: version.stamp.rfc3339(),
            path: version.path.clone(),
            content: version.content,
            owner: version.owner.clone(),
            replaces: version.replaces,
            applies: version.applies.clone(),
        }
    }
}

/// A member, with the key their name is bound to, none once excluded, and
/// who last rebound it and when.
#[derive(Clone, Debug, Serialize)]
pub struct MemberView {
    pub name: MemberName,
    pub key: Option<iroh_base::PublicKey>,
    pub joined: String,
    pub rebound: Option<RebindingView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RebindingView {
    pub by: MemberName,
    pub time: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RequestView {
    pub path: GroupPath,
    pub author: MemberName,
    pub time: String,
    pub statement: RequestStatement,
    pub decision: Option<Decision>,
    pub applied: bool,
    /// Whether a change replaces a version other than its file's current
    /// one: the request was made without seeing a later change.
    pub outdated: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AsideView {
    pub id: u64,
    #[serde(flatten)]
    pub item: AsideItem,
}

impl Engine {
    /// The edits waiting to be published at `under` or inside it, or
    /// everywhere, by path.
    pub async fn pending(&self, under: Option<&GroupPath>) -> Vec<PendingView> {
        let inner = &self.inner;
        let work = inner.work.lock().await;
        let mut pending: Vec<PendingView> = work
            .pending
            .iter()
            .filter(|(key, _)| under.is_none_or(|under| key.is_within(&under.key())))
            .map(|(_, pending)| {
                let settle = inner.settle_time(&pending.path);
                PendingView {
                    path: pending.path.clone(),
                    deleted: pending.stat.is_none(),
                    due_in: settle.saturating_sub(pending.since.elapsed()).as_secs(),
                    freezes: inner.ledger.lock().freezes(&pending.path),
                    cutoff: work.selection.cutoff(&pending.path),
                }
            })
            .collect();
        pending.sort_by(|a, b| a.path.cmp(&b.path));
        pending
    }

    /// # Panics
    ///
    /// Panics if a panic poisoned the error list.
    pub async fn status(&self) -> Status {
        let inner = &self.inner;
        let work = inner.work.lock().await;
        Status {
            member: inner.config.member.clone(),
            machine: inner.me(),
            root: inner.config.root.clone(),
            join: work.join.clone(),
            peers: inner.node.peers(),
            incompatible: inner.node.incompatible(),
            relay: inner.node.home_relay().map(|url| url.to_string()),
            pending: work.pending.len(),
            fetching: work.fetching.len(),
            patches: inner.ledger.lock().patches().count(),
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
        let mut key = self.inner.config.key.clone();
        key.secret = self.inner.node.secret().borrow().secret.clone();
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
            .filter(|version| {
                under.is_none_or(|under| {
                    version.path.key() == under.key()
                        || version
                            .path
                            .key()
                            .as_str()
                            .starts_with(&format!("{}/", under.key().as_str()))
                })
            })
            .filter_map(|version| {
                let content = version.content?;
                let entry = entries.get(&version.path.key());
                let held = entry.is_some_and(|entry| entry.seen.is_some());
                Some(FileView {
                    path: version.path.clone(),
                    owner: version.owner.clone(),
                    content,
                    stamp: version.stamp,
                    time: version.stamp.rfc3339(),
                    cutoff: work.selection.cutoff(&version.path),
                    held,
                    outdated: held && entry.and_then(|entry| entry.synced) != Some(version.stamp),
                    writable: inner.writable(&ledger, &work, &version.path),
                })
            })
            .collect();
        files.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
        Ok(files)
    }

    /// Every accepted version of `path`, oldest first.
    #[must_use]
    pub fn history(&self, path: &GroupPath) -> Vec<VersionView> {
        self.inner
            .ledger
            .lock()
            .versions(&path.key())
            .iter()
            .map(VersionView::from)
            .collect()
    }

    #[must_use]
    pub fn members(&self) -> Vec<MemberView> {
        self.inner
            .ledger
            .lock()
            .members()
            .iter()
            .map(|(name, member)| MemberView {
                name: name.clone(),
                key: member.key,
                joined: member.joined.rfc3339(),
                rebound: member.rebound.as_ref().map(|rebinding| RebindingView {
                    by: rebinding.by.clone(),
                    time: rebinding.stamp.rfc3339(),
                }),
            })
            .collect()
    }

    /// Every live request whose statement is here.
    pub async fn requests(&self) -> Vec<RequestView> {
        let inner = &self.inner;
        let folder = requests_folder();
        let requests: Vec<Version> = inner
            .ledger
            .lock()
            .live()
            .filter(|version| version.path.is_inside(&folder))
            .cloned()
            .collect();
        let applied = inner.applied_requests();
        let mut views = Vec::new();
        for request in requests {
            let Some(content) = request.content else {
                continue;
            };
            let Ok(statement) = inner.read_statement::<RequestStatement>(&content).await else {
                continue;
            };
            let decision = inner.decision(&request.path, &statement.owner).await;
            let outdated = {
                let ledger = inner.ledger.lock();
                statement.changes.iter().any(|change| {
                    ledger.head(&change.path.key()).map(|head| head.stamp) != change.replaces
                })
            };
            views.push(RequestView {
                outdated,
                applied: applied.contains(&request.path),
                path: request.path,
                author: request.owner,
                time: request.stamp.rfc3339(),
                statement,
                decision,
            });
        }
        views.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
        views
    }

    /// The set-aside list.
    ///
    /// # Errors
    ///
    /// Fails if the state cannot be read.
    pub fn aside(&self) -> Result<Vec<AsideView>> {
        Ok(self
            .inner
            .state
            .aside()?
            .into_iter()
            .map(|(id, item)| AsideView { id, item })
            .collect())
    }

    /// This machine's retention.
    ///
    /// # Errors
    ///
    /// Fails if the state cannot be read.
    pub fn retention(&self) -> Result<Retention> {
        Ok(self.inner.state.retention()?)
    }

    /// The selection's rules, the last matching one winning.
    pub async fn selection(&self) -> Vec<Rule> {
        self.inner
            .work
            .lock()
            .await
            .selection
            .rules()
            .cloned()
            .collect()
    }
}
