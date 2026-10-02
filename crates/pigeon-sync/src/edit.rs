//! What a person changes in the tree through an action, which publishes it
//! at once, whoever owns the files: writing, deleting and renaming files or
//! whole folders, a renamed file continuing its history, and restoring
//! files to what they held at a time. A draft, which only this machine's
//! disk holds, moves or goes on disk and is published at once.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use pigeon_core::ledger::{Ledger, Version};
use pigeon_core::patch::{Change, Content, VersionRef};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::restore::restore;
use pigeon_core::selection::{compile, matches};
use pigeon_core::statement::is_statement;
use pigeon_store::disk::{self, fs_path};
use serde::Serialize;

use crate::disk_sync::file_stat;
use crate::engine::{Engine, Inner, JoinState, Work};

/// One change a person asks for. A folder is the prefix of its files'
/// paths, so deleting or renaming a folder deletes or renames every file
/// inside.
#[derive(Clone, Debug)]
pub enum Edit {
    Write { path: GroupPath, bytes: Vec<u8> },
    Delete { path: GroupPath },
    Rename { from: GroupPath, to: GroupPath },
}

/// The paths an action changed, all published.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Edited {
    pub published: Vec<GroupPath>,
}

/// The content one path is to hold.
enum Target {
    Bytes(Vec<u8>),
    /// The content of the published version it moves from.
    Moved(Content, VersionRef),
    /// The draft at `from`, which only this machine's disk holds.
    Draft {
        from: GroupPath,
    },
    Gone,
}

/// The live files at `path` or inside the folder `path`.
fn files_at<'a>(ledger: &'a Ledger, path: &GroupPath) -> Vec<&'a Version> {
    let folder = path.key();
    ledger
        .live()
        .filter(|version| version.path.key().is_within(&folder))
        .collect()
}

/// The drafts among `drafts` at `path` or inside the folder `path` that
/// the ledger holds no live file at.
fn drafts_at<'a>(ledger: &Ledger, drafts: &'a [GroupPath], path: &GroupPath) -> Vec<&'a GroupPath> {
    let folder = path.key();
    drafts
        .iter()
        .filter(|draft| draft.key().is_within(&folder))
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
                .filter_map(|version| {
                    let content = version.content?;
                    let moved = Target::Moved(content, version.reference());
                    Some((version.path.clone(), moved))
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
                let target = source.moved(&from, &to)?;
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
    /// Makes the disk at `path` hold the draft `target`, which moves from
    /// its place or goes.
    fn edit_draft(&self, path: &GroupPath, target: &Target) -> Result<()> {
        let root = &self.root;
        let location = fs_path(root, path);
        match target {
            Target::Draft { from } => {
                if let Some(parent) = location.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                let source = fs_path(root, from);
                std::fs::rename(&source, &location)
                    .with_context(|| format!("moving {from} to {path}"))?;
                disk::remove(root, &source)?;
            }
            _ => {
                if file_stat(&location).is_some() {
                    disk::remove(root, &location)?;
                }
            }
        }
        Ok(())
    }

    /// The change that makes `path` hold `target`.
    async fn edit_change(
        &self,
        work: &mut Work,
        path: GroupPath,
        target: Target,
    ) -> Result<Change> {
        let head = self.ledger.lock().head(&path.key()).cloned();
        let (content, continues) = match target {
            Target::Bytes(bytes) => {
                let mut content = self.add_content(work, bytes).await?;
                content.executable = head
                    .as_ref()
                    .and_then(|head| head.content)
                    .is_some_and(|old| old.executable);
                (Some(content), None)
            }
            Target::Moved(content, from) => (
                Some(content),
                Some(from).filter(|from| from.path.key() != path.key()),
            ),
            Target::Draft { from } => {
                bail!("{from} is not published yet: only its machine moves it")
            }
            Target::Gone => (None, None),
        };
        Ok(Change {
            path,
            content,
            replaces: head.map(|head| head.stamp),
            continues,
        })
    }

    /// Publishes `changes` at once, if the member has joined and none lies
    /// in the statements folder, and brings their paths into agreement.
    async fn publish_changes(
        self: &Arc<Self>,
        work: &mut Work,
        changes: Vec<Change>,
    ) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        if work.join != JoinState::Joined {
            bail!("{} has not joined the group yet", self.member);
        }
        if let Some(change) = changes
            .iter()
            .find(|change| is_statement(&change.path.key()))
        {
            bail!("{} lies in the statements folder", change.path);
        }
        self.ledger
            .lock()
            .check(&self.member, &self.cert.member, &changes)?;
        let keys: Vec<PathKey> = changes.iter().map(|change| change.path.key()).collect();
        self.publish_at(self.clock.stamp(), changes)?;
        work.protect_due = true;
        self.refresh_keys(work, &keys).await;
        Ok(())
    }

    async fn edit(self: &Arc<Self>, work: &mut Work, edits: Vec<Edit>) -> Result<Edited> {
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
        let mut moved = Vec::new();
        let mut changes = Vec::new();
        for (path, target) in planned {
            let draft = match &target {
                Target::Draft { .. } => true,
                Target::Gone => !self
                    .ledger
                    .lock()
                    .head(&path.key())
                    .is_some_and(Version::is_live),
                Target::Bytes(_) | Target::Moved(..) => false,
            };
            if draft {
                self.edit_draft(&path, &target)?;
                moved.push(path);
            } else {
                changes.push(self.edit_change(work, path, target).await?);
            }
        }
        let mut published: Vec<GroupPath> =
            changes.iter().map(|change| change.path.clone()).collect();
        self.publish_changes(work, changes).await?;
        self.publish_now(work, &moved).await?;
        published.extend(moved);
        Ok(Edited { published })
    }
}

impl Engine {
    /// Carries out `edits` and publishes them at once: drafts move or go
    /// on disk first.
    ///
    /// # Errors
    ///
    /// Fails if a path holds no file, a rename's target exists, the member
    /// has not joined, a path lies in the statements folder, or the disk
    /// refuses.
    pub async fn edit(&self, edits: Vec<Edit>) -> Result<Edited> {
        let mut work = self.inner.work.lock().await;
        self.inner.edit(&mut work, edits).await
    }

    /// Makes every file the gitignore `pattern` matches hold what it held
    /// at `time`, by new versions that leave the history whole: a file
    /// that lived then and was deleted since comes back, one created since
    /// goes, and a moved file keeps its path.
    ///
    /// # Errors
    ///
    /// Fails if `pattern` is no gitignore pattern, the files already hold
    /// what they held then, or the member has not joined.
    pub async fn restore(&self, pattern: &str, time: u64) -> Result<Edited> {
        let matcher = compile(pattern)?;
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        let changes = restore(&inner.ledger.lock(), |path| matches(&matcher, path), time);
        if changes.is_empty() {
            bail!("{pattern} holds what it held then already");
        }
        let published = changes.iter().map(|change| change.path.clone()).collect();
        inner.publish_changes(&mut work, changes).await?;
        Ok(Edited { published })
    }
}
