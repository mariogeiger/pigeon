//! The changes that wait for someone, shown alike: each request neither
//! applied nor refused, one change each, and each item a machine set
//! aside, the same change unsigned; and what anyone does to one of them,
//! from any machine: apply it, ask its owner, place it at another path, or
//! discard it.

use std::sync::Arc;

use anyhow::{Result, bail};
use pigeon_core::clock::{MachineId, Stamp};
use pigeon_core::ledger::Version;
use pigeon_core::name::MemberName;
use pigeon_core::patch::{Change, Content};
use pigeon_core::path::GroupPath;
use pigeon_core::statement::{
    AsideItem, Decision, Mode, Reason, RequestStatement, STATEMENTS, aside_folder,
};
use pigeon_store::disk::fs_path;
use serde::Serialize;

use crate::disk_sync::file_stat;
use crate::engine::{Engine, Inner, Work};
use crate::statements::requests_folder;

/// Why a change waits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Waits {
    /// A proposal nobody decided yet.
    Proposed,
    /// Forced or accepted, on its way to its owner's machines.
    Applying,
    /// A machine of its author set it aside: nobody signed it yet.
    SetAside { reason: Reason, machine: MachineId },
}

/// One change waiting for someone.
#[derive(Clone, Debug, Serialize)]
pub struct WaitingChange {
    /// The statement that holds it: its request, or its set-aside file.
    pub entry: GroupPath,
    /// Who made it: the request's author, or the member whose machine set
    /// it aside.
    pub author: MemberName,
    /// Who decides it: the owner of its path, or its author when the path
    /// is not portable.
    pub owner: MemberName,
    /// The path it changes, not always portable when it was set aside.
    pub path: String,
    /// The content it gives the path, or `None` for a deletion.
    pub content: Option<Content>,
    pub replaces: Option<Stamp>,
    pub message: String,
    pub time: String,
    /// Whether it replaces a version other than its path's current one.
    pub outdated: bool,
    pub waits: Waits,
}

/// The statement holding a waiting change.
enum Entry {
    Request(Version, RequestStatement),
    SetAside(Version, AsideItem),
}

/// The change that deletes `version`'s file.
fn deletion(version: &Version) -> Change {
    Change {
        path: version.path.clone(),
        content: None,
        replaces: Some(version.stamp),
    }
}

impl Inner {
    /// The live statement at `entry` holding a waiting change.
    async fn entry(&self, entry: &GroupPath) -> Result<Entry> {
        let head = self.ledger.lock().head(&entry.key()).cloned();
        let Some((version, content)) = head.and_then(|head| head.content.map(|c| (head, c))) else {
            bail!("no change waits at {entry}");
        };
        if version.path.is_inside(&requests_folder()) {
            Ok(Entry::Request(
                version,
                self.read_statement(&content).await?,
            ))
        } else if version.path.is_inside(&aside_folder()) {
            Ok(Entry::SetAside(
                version,
                self.read_statement(&content).await?,
            ))
        } else {
            bail!("no change waits at {entry}")
        }
    }

    /// Why the request at `version` waits, if it does.
    async fn request_waits(
        &self,
        version: &Version,
        statement: &RequestStatement,
    ) -> Option<Waits> {
        if self.applied_requests().contains(&version.path) {
            return None;
        }
        match self.decision(&version.path).await {
            Some(Decision::Refuse) => None,
            Some(Decision::Accept) => Some(Waits::Applying),
            None if statement.mode == Mode::Force || version.owner == statement.owner => {
                Some(Waits::Applying)
            }
            None => Some(Waits::Proposed),
        }
    }

    /// The proposal at `version`, which must wait for a decision.
    async fn proposal(&self, version: &Version, statement: &RequestStatement) -> Result<()> {
        match self.request_waits(version, statement).await {
            Some(Waits::Proposed) => Ok(()),
            Some(_) => bail!("{} is on its way to its owner", version.path),
            None => bail!("{} is done", version.path),
        }
    }

    /// Fails unless `to` is free, here and in the ledger, outside the
    /// statements folder.
    fn ensure_free(&self, to: &GroupPath) -> Result<()> {
        if to.is_inside(STATEMENTS) {
            bail!("{to} lies in the statements folder");
        }
        let live = self
            .ledger
            .lock()
            .head(&to.key())
            .is_some_and(Version::is_live);
        if live || file_stat(&fs_path(&self.root, to)).is_some() {
            bail!("{to} exists");
        }
        Ok(())
    }

    /// Files `change`, made by `author`, in `mode`, then resolves the
    /// set-aside item at `version` by deleting its file.
    async fn send(
        self: &Arc<Self>,
        work: &mut Work,
        change: Change,
        mode: Mode,
        message: &str,
        version: &Version,
    ) -> Result<Vec<GroupPath>> {
        let author = &version.owner;
        let mut requests = self
            .request(work, vec![change], mode, message, author)
            .await?;
        requests.extend(
            self.request(work, vec![deletion(version)], Mode::Force, message, author)
                .await?,
        );
        Ok(requests)
    }
}

impl Engine {
    /// Every change waiting for someone, oldest first.
    pub async fn waiting_changes(&self) -> Vec<WaitingChange> {
        let inner = &self.inner;
        let (requests, aside) = (requests_folder(), aside_folder());
        let entries: Vec<GroupPath> = inner
            .ledger
            .lock()
            .live()
            .filter(|version| version.path.is_inside(&requests) || version.path.is_inside(&aside))
            .map(|version| version.path.clone())
            .collect();
        let mut views = Vec::new();
        for path in entries {
            let Ok(entry) = inner.entry(&path).await else {
                continue;
            };
            let (version, changes, message, waits) = match entry {
                Entry::Request(version, statement) => {
                    let Some(waits) = inner.request_waits(&version, &statement).await else {
                        continue;
                    };
                    let changes = statement
                        .changes
                        .into_iter()
                        .map(|change| {
                            (
                                change.path.as_str().to_owned(),
                                change.content,
                                change.replaces,
                            )
                        })
                        .collect();
                    (version, changes, statement.message, waits)
                }
                Entry::SetAside(version, item) => {
                    let waits = Waits::SetAside {
                        reason: item.reason,
                        machine: version.stamp.machine,
                    };
                    let changes = vec![(item.path, item.content, item.replaces)];
                    (version, changes, String::new(), waits)
                }
            };
            for (path, content, replaces) in changes {
                let ledger = inner.ledger.lock();
                let (owner, outdated) = match GroupPath::parse(&path) {
                    Ok(parsed) => (
                        ledger.owner_of(&parsed, &version.owner),
                        ledger.head(&parsed.key()).map(|head| head.stamp) != replaces,
                    ),
                    Err(_) => (version.owner.clone(), false),
                };
                views.push(WaitingChange {
                    entry: version.path.clone(),
                    author: version.owner.clone(),
                    owner,
                    path,
                    content,
                    replaces,
                    message: message.clone(),
                    time: version.stamp.rfc3339(),
                    outdated,
                    waits: waits.clone(),
                });
            }
        }
        views.sort_by(|a, b| a.time.cmp(&b.time).then_with(|| a.entry.cmp(&b.entry)));
        views
    }

    /// Applies the change at `entry`: accepts a proposal, or forces a
    /// set-aside item on its path's owner, with `message`, which resolves
    /// the item.
    ///
    /// # Errors
    ///
    /// Fails if no change waits there, it is on its way already, a
    /// set-aside item's path is not portable, or a statement cannot be
    /// filed.
    pub async fn apply_change(&self, entry: &GroupPath, message: &str) -> Result<Vec<GroupPath>> {
        self.send_change(entry, Mode::Force, message).await
    }

    /// Asks the owner of a set-aside item's path to accept it, with
    /// `message`, which resolves the item.
    ///
    /// # Errors
    ///
    /// Fails if no item waits there, its path is not portable, or a
    /// statement cannot be filed.
    pub async fn ask_change(&self, entry: &GroupPath, message: &str) -> Result<Vec<GroupPath>> {
        self.send_change(entry, Mode::Propose, message).await
    }

    async fn send_change(
        &self,
        entry: &GroupPath,
        mode: Mode,
        message: &str,
    ) -> Result<Vec<GroupPath>> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        match inner.entry(entry).await? {
            Entry::Request(version, statement) => {
                if mode == Mode::Propose {
                    bail!("{entry} asks its owner already");
                }
                inner.proposal(&version, &statement).await?;
                inner.decide(&mut work, entry, Decision::Accept).await?;
                inner.apply_requests(&mut work).await;
                Ok(Vec::new())
            }
            Entry::SetAside(version, item) => {
                let path = GroupPath::parse(&item.path)?;
                let replaces = inner.ledger.lock().head(&path.key()).map(|head| head.stamp);
                let change = Change {
                    path,
                    content: item.content,
                    replaces,
                };
                inner.send(&mut work, change, mode, message, &version).await
            }
        }
    }

    /// Places the change at `entry` at the free path `to` instead, in
    /// `mode`, with `message`: the new file belongs to the change's author
    /// unless its path names its owner, and the proposal is then refused,
    /// or the set-aside item resolved.
    ///
    /// # Errors
    ///
    /// Fails if no change waits there, it is on its way already, deletes
    /// or changes several files, `to` is taken or lies in the statements
    /// folder, or a statement cannot be filed.
    pub async fn place_change(
        &self,
        entry: &GroupPath,
        to: &GroupPath,
        mode: Mode,
        message: &str,
    ) -> Result<Vec<GroupPath>> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        inner.ensure_free(to)?;
        let placed = |content: Option<Content>| match content {
            Some(content) => Ok(Change {
                path: to.clone(),
                content: Some(content),
                replaces: None,
            }),
            None => bail!("{entry} deletes a file: there is nothing to place"),
        };
        match inner.entry(entry).await? {
            Entry::Request(version, statement) => {
                inner.proposal(&version, &statement).await?;
                let [change] = &statement.changes[..] else {
                    bail!("{entry} changes several files: decide it whole");
                };
                let change = placed(change.content)?;
                let requests = inner
                    .request(&mut work, vec![change], mode, message, &version.owner)
                    .await?;
                inner.decide(&mut work, entry, Decision::Refuse).await?;
                Ok(requests)
            }
            Entry::SetAside(version, item) => {
                let change = placed(item.content)?;
                inner.send(&mut work, change, mode, message, &version).await
            }
        }
    }

    /// Discards the change at `entry`: refuses a proposal, or asks the
    /// member whose machine set an item aside, in `mode`, to delete its
    /// file.
    ///
    /// # Errors
    ///
    /// Fails if no change waits there, it is on its way already, or a
    /// statement cannot be filed.
    pub async fn discard_change(&self, entry: &GroupPath, mode: Mode) -> Result<Vec<GroupPath>> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        match inner.entry(entry).await? {
            Entry::Request(version, statement) => {
                inner.proposal(&version, &statement).await?;
                inner.decide(&mut work, entry, Decision::Refuse).await?;
                Ok(Vec::new())
            }
            Entry::SetAside(version, item) => {
                let message = format!("discards what was set aside at {}", item.path);
                inner
                    .request(
                        &mut work,
                        vec![deletion(&version)],
                        mode,
                        &message,
                        &version.owner,
                    )
                    .await
            }
        }
    }
}
