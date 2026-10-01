//! What a person changes in the tree through an action: writing, deleting
//! and renaming files or whole folders, drafts not yet published included.
//! Each file changes under its own rule: the files the member may write
//! change on disk and are published at once, and the others become
//! requests to their owners, one per owner.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use pigeon_core::ledger::{Ledger, Version};
use pigeon_core::patch::{Change, Content};
use pigeon_core::path::GroupPath;
use pigeon_core::statement::Mode;
use pigeon_store::disk::{self, fs_path, install, temporary_path};
use serde::Serialize;

use crate::disk_sync::file_stat;
use crate::engine::{Engine, Inner, Work};

/// One change a person asks for. A folder is the prefix of its files'
/// paths, so deleting or renaming a folder deletes or renames every file
/// inside.
#[derive(Clone, Debug)]
pub enum Edit {
    Write { path: GroupPath, bytes: Vec<u8> },
    Delete { path: GroupPath },
    Rename { from: GroupPath, to: GroupPath },
}

/// Where an edit went: the paths changed on disk and published at once,
/// or as soon as the member has joined, and the requests filed for the
/// rest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Edited {
    pub published: Vec<GroupPath>,
    pub requests: Vec<GroupPath>,
}

/// The content one path is to hold.
enum Target {
    Bytes(Vec<u8>),
    Moved {
        from: GroupPath,
        content: Content,
    },
    /// The draft at `from`, which only this machine's disk holds.
    Draft {
        from: GroupPath,
    },
    Gone,
}

/// The live files at `path` or inside the folder `path`.
fn files_at<'a>(ledger: &'a Ledger, path: &GroupPath) -> Vec<&'a Version> {
    let key = path.key();
    ledger
        .live()
        .filter(|version| version.path.key() == key || version.path.is_inside(path.as_str()))
        .collect()
}

/// The drafts among `drafts` at `path` or inside the folder `path` that
/// the ledger holds no live file at.
fn drafts_at<'a>(ledger: &Ledger, drafts: &'a [GroupPath], path: &GroupPath) -> Vec<&'a GroupPath> {
    let key = path.key();
    drafts
        .iter()
        .filter(|draft| draft.key() == key || draft.is_inside(path.as_str()))
        .filter(|draft| !ledger.head(&draft.key()).is_some_and(Version::is_live))
        .collect()
}

/// Spells out `edit` as the content each path is to hold, given the
/// `drafts` this machine holds, moves first so that a moved file leaves
/// its source before the source is deleted.
fn targets(ledger: &Ledger, drafts: &[GroupPath], edit: Edit) -> Result<Vec<(GroupPath, Target)>> {
    match edit {
        Edit::Write { path, bytes } => Ok(vec![(path, Target::Bytes(bytes))]),
        Edit::Delete { path } => {
            let files = files_at(ledger, &path);
            let drafts = drafts_at(ledger, drafts, &path);
            if files.is_empty() && drafts.is_empty() {
                bail!("no file at {path}");
            }
            Ok(files
                .into_iter()
                .map(|version| &version.path)
                .chain(drafts)
                .map(|path| (path.clone(), Target::Gone))
                .collect())
        }
        Edit::Rename { from, to } => {
            let files = files_at(ledger, &from);
            let moved_drafts = drafts_at(ledger, drafts, &from);
            if files.is_empty() && moved_drafts.is_empty() {
                bail!("no file at {from}");
            }
            let sources = files
                .into_iter()
                .map(|version| {
                    let content = version.content.expect("a live version has content");
                    let from = version.path.clone();
                    (version.path.clone(), Target::Moved { from, content })
                })
                .chain(moved_drafts.into_iter().map(|draft| {
                    (
                        draft.clone(),
                        Target::Draft {
                            from: draft.clone(),
                        },
                    )
                }));
            let mut moves = Vec::new();
            let mut gone = Vec::new();
            for (source, moved) in sources {
                let target = if source.key() == from.key() {
                    to.clone()
                } else {
                    source.moved(from.as_str(), to.as_str())?
                };
                let taken = ledger.head(&target.key()).is_some_and(Version::is_live)
                    || drafts.iter().any(|draft| draft.key() == target.key());
                if target.key() != source.key() && taken {
                    bail!("{target} exists");
                }
                if source.key() != target.key() {
                    gone.push((source, Target::Gone));
                }
                moves.push((target, moved));
            }
            moves.extend(gone);
            Ok(moves)
        }
    }
}

impl Inner {
    /// Makes the disk at `path` hold `target`, for a path the member may
    /// write.
    async fn edit_disk(&self, work: &Work, path: &GroupPath, target: Target) -> Result<()> {
        let root = &self.root;
        let location = fs_path(root, path);
        if let Some(parent) = location.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        match target {
            Target::Bytes(bytes) => {
                let temporary = temporary_path(&location);
                std::fs::write(&temporary, bytes)
                    .with_context(|| format!("writing {}", temporary.display()))?;
                let executable = file_stat(&location)
                    .and_then(|stat| stat.executable)
                    .unwrap_or(false);
                install(&temporary, &location, executable, true)?;
            }
            Target::Moved { from, content } => {
                let source = fs_path(root, &from);
                let movable = file_stat(&source).is_some() && {
                    let ledger = self.ledger.lock();
                    self.writable(&ledger, work, &from)
                };
                if movable {
                    std::fs::rename(&source, &location)
                        .with_context(|| format!("moving {from} to {path}"))?;
                    disk::remove(root, &source)?;
                } else {
                    self.blobs
                        .export(&content.hash, &location, content.executable, true)
                        .await
                        .with_context(|| format!("{from} is not on this machine"))?;
                }
            }
            Target::Draft { from } => {
                let source = fs_path(root, &from);
                std::fs::rename(&source, &location)
                    .with_context(|| format!("moving {from} to {path}"))?;
                disk::remove(root, &source)?;
            }
            Target::Gone => {
                if file_stat(&location).is_some() {
                    disk::remove(root, &location)?;
                }
            }
        }
        Ok(())
    }

    /// The change asking the owner of `path` to make it hold `target`.
    async fn edit_change(
        &self,
        work: &mut Work,
        path: GroupPath,
        target: Target,
    ) -> Result<Change> {
        let head = self.ledger.lock().head(&path.key()).cloned();
        let content = match target {
            Target::Bytes(bytes) => {
                let mut content = self.add_content(work, bytes).await?;
                content.executable = head
                    .as_ref()
                    .and_then(|head| head.content)
                    .is_some_and(|old| old.executable);
                Some(content)
            }
            Target::Moved { content, .. } => Some(content),
            Target::Draft { from } => {
                bail!("{from} is not published yet: only its machine moves it")
            }
            Target::Gone => None,
        };
        Ok(Change {
            path,
            content,
            replaces: head.map(|head| head.stamp),
        })
    }

    async fn edit(
        self: &Arc<Self>,
        work: &mut Work,
        edits: Vec<Edit>,
        mode: Mode,
        message: &str,
    ) -> Result<Edited> {
        let drafts: Vec<GroupPath> = work
            .pending
            .values()
            .filter(|pending| pending.stat.is_some())
            .map(|pending| pending.path.clone())
            .collect();
        let mut planned = Vec::new();
        {
            let ledger = self.ledger.lock();
            for edit in edits {
                planned.extend(targets(&ledger, &drafts, edit)?);
            }
        }
        let mut published = Vec::new();
        let mut changes = Vec::new();
        for (path, target) in planned {
            let writable = {
                let ledger = self.ledger.lock();
                self.writable(&ledger, work, &path)
            };
            if writable {
                self.edit_disk(work, &path, target).await?;
                published.push(path);
            } else {
                changes.push(self.edit_change(work, path, target).await?);
            }
        }
        self.publish_now(work, &published).await?;
        let requests = if changes.is_empty() {
            Vec::new()
        } else {
            self.request(work, changes, mode, message, &self.member)
                .await?
        };
        Ok(Edited {
            published,
            requests,
        })
    }
}

impl Engine {
    /// Carries out `edits`: what the member may write changes on disk and
    /// is published at once, and the rest is requested from its owners in
    /// `mode`, with `message`.
    ///
    /// # Errors
    ///
    /// Fails if a path holds no file, a rename's target exists, a moved
    /// file's content is not on this machine, or the disk refuses.
    pub async fn edit(&self, edits: Vec<Edit>, mode: Mode, message: &str) -> Result<Edited> {
        let mut work = self.inner.work.lock().await;
        self.inner.edit(&mut work, edits, mode, message).await
    }
}
