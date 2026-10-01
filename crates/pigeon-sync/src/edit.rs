//! What a person changes in the tree through an action: writing, deleting
//! and renaming files or whole folders. Each file changes under its own
//! rule: the files the member may write change on disk and are published at
//! once, and the others become requests to their owners, one per owner.

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
    Moved { from: GroupPath, content: Content },
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

/// Spells out `edit` as the content each path is to hold, moves first so
/// that a moved file leaves its source before the source is deleted.
fn targets(ledger: &Ledger, edit: Edit) -> Result<Vec<(GroupPath, Target)>> {
    match edit {
        Edit::Write { path, bytes } => Ok(vec![(path, Target::Bytes(bytes))]),
        Edit::Delete { path } => {
            let files = files_at(ledger, &path);
            if files.is_empty() {
                bail!("no file at {path}");
            }
            Ok(files
                .into_iter()
                .map(|version| (version.path.clone(), Target::Gone))
                .collect())
        }
        Edit::Rename { from, to } => {
            let files = files_at(ledger, &from);
            if files.is_empty() {
                bail!("no file at {from}");
            }
            let mut moves = Vec::new();
            let mut sources = Vec::new();
            for version in files {
                let source = version.path.clone();
                let target = if source.key() == from.key() {
                    to.clone()
                } else {
                    source.moved(from.as_str(), to.as_str())?
                };
                if target.key() != source.key()
                    && ledger.head(&target.key()).is_some_and(Version::is_live)
                {
                    bail!("{target} exists");
                }
                let content = version.content.expect("a live version has content");
                moves.push((
                    target,
                    Target::Moved {
                        from: source.clone(),
                        content,
                    },
                ));
                if source.key() != moves[moves.len() - 1].0.key() {
                    sources.push((source, Target::Gone));
                }
            }
            moves.extend(sources);
            Ok(moves)
        }
    }
}

impl Inner {
    /// Makes the disk at `path` hold `target`, for a path the member may
    /// write.
    async fn edit_disk(&self, work: &Work, path: &GroupPath, target: Target) -> Result<()> {
        let root = &self.config.root;
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
        let mut planned = Vec::new();
        {
            let ledger = self.ledger.lock();
            for edit in edits {
                planned.extend(targets(&ledger, edit)?);
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
            self.request(work, changes, mode, message).await?
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
