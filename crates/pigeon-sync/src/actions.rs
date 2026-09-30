//! What a person can ask of the engine: change the selection, write,
//! delete or rename files and publish at once, file and answer requests,
//! and resolve set-aside items.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use pigeon_core::patch::Change;
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_core::statement::{Decision, Mode};
use pigeon_store::disk::{self, fs_path, install, temporary_path};

use crate::disk_sync::{Probe, file_stat};
use crate::engine::{Engine, Inner, Work};
use crate::watch::Rescan;

impl Inner {
    fn ensure_writable(&self, work: &Work, path: &GroupPath) -> Result<()> {
        let ledger = self.ledger.lock();
        if !self.writable(&ledger, work, path) {
            bail!(
                "{path} is not writable by {}: propose a request",
                self.config.member
            );
        }
        Ok(())
    }

    /// Compares `paths` with the ledger and publishes their edits at once.
    async fn publish_now(self: &Arc<Self>, work: &mut Work, paths: &[GroupPath]) -> Result<()> {
        let keys: Vec<PathKey> = paths.iter().map(GroupPath::key).collect();
        for path in paths {
            let probe = Probe::at(&self.config.root, path.clone());
            self.sync_key(work, &path.key(), Some(probe)).await?;
        }
        self.publish_settled(work, &keys).await;
        Ok(())
    }
}

impl Engine {
    /// Makes `rule` the last rule of the selection. A rule that stops
    /// holding files also removes the held copies nobody modified.
    ///
    /// # Errors
    ///
    /// Fails if the pattern is invalid or the state cannot be written.
    pub async fn set_rule(&self, rule: Rule) -> Result<()> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        work.selection.set(rule)?;
        inner.state.set_selection(&work.selection)?;
        for entry in inner.state.index(None)? {
            if work.selection.cutoff(&entry.path) != Cutoff::MinusInfinity {
                continue;
            }
            let location = fs_path(&inner.config.root, &entry.path);
            let stat = file_stat(&location);
            let untouched = entry.seen.map(|seen| seen.stat) == stat;
            if untouched && !work.pending.contains_key(&entry.path.key()) {
                if stat.is_some() {
                    disk::remove(&inner.config.root, &location)?;
                }
                inner.state.update_index([(&entry.path.key(), None)])?;
            }
        }
        inner.refresh(&mut work, &Rescan::All).await;
        Ok(())
    }

    /// Writes `bytes` to the file at `path` and publishes it.
    ///
    /// # Errors
    ///
    /// Fails if the member may not write the file or the disk refuses.
    pub async fn write(&self, path: &GroupPath, bytes: &[u8]) -> Result<()> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        inner.ensure_writable(&work, path)?;
        let location = fs_path(&inner.config.root, path);
        if let Some(parent) = location.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let temporary = temporary_path(&location);
        std::fs::write(&temporary, bytes)
            .with_context(|| format!("writing {}", temporary.display()))?;
        let executable = file_stat(&location)
            .and_then(|stat| stat.executable)
            .unwrap_or(false);
        install(&temporary, &location, executable, true)?;
        inner
            .publish_now(&mut work, std::slice::from_ref(path))
            .await
    }

    /// Deletes the file at `path` and publishes the deletion.
    ///
    /// # Errors
    ///
    /// Fails if the member may not delete the file or the disk refuses.
    pub async fn delete(&self, path: &GroupPath) -> Result<()> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        inner.ensure_writable(&work, path)?;
        disk::remove(&inner.config.root, &fs_path(&inner.config.root, path))?;
        inner
            .publish_now(&mut work, std::slice::from_ref(path))
            .await
    }

    /// Moves the file at `from` to `to` and publishes both in one patch.
    ///
    /// # Errors
    ///
    /// Fails if the member may not write both paths, `to` exists, or the
    /// disk refuses.
    pub async fn rename(&self, from: &GroupPath, to: &GroupPath) -> Result<()> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        inner.ensure_writable(&work, from)?;
        inner.ensure_writable(&work, to)?;
        let root = &inner.config.root;
        let source = fs_path(root, from);
        let target = fs_path(root, to);
        if from.key() != to.key() && file_stat(&target).is_some() {
            bail!("{to} exists");
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::rename(&source, &target).with_context(|| format!("moving {from} to {to}"))?;
        disk::remove(root, &source)?;
        inner
            .publish_now(&mut work, &[from.clone(), to.clone()])
            .await
    }

    /// Asks the owners of the changed files to apply `changes`, one
    /// request per owner, and returns the requests' paths.
    ///
    /// # Errors
    ///
    /// Fails if the member has not joined or the request cannot be stored.
    pub async fn request(
        &self,
        changes: Vec<Change>,
        mode: Mode,
        message: &str,
    ) -> Result<Vec<GroupPath>> {
        let mut work = self.inner.work.lock().await;
        self.inner.request(&mut work, changes, mode, message).await
    }

    /// Answers a request addressed to this member.
    ///
    /// # Errors
    ///
    /// Fails if the request is unknown or addressed to someone else.
    pub async fn decide(&self, request: &GroupPath, decision: Decision) -> Result<()> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        inner.decide(&mut work, request, decision).await?;
        inner.apply_requests(&mut work).await;
        Ok(())
    }

    /// Forgets a set-aside item.
    ///
    /// # Errors
    ///
    /// Fails if the item is unknown.
    pub async fn discard_aside(&self, id: u64) -> Result<()> {
        let mut work = self.inner.work.lock().await;
        if self.inner.state.take_aside(id)?.is_none() {
            bail!("no set-aside item {id}");
        }
        work.protect_due = true;
        Ok(())
    }

    /// Writes a set-aside item's content to the free path `to` and
    /// publishes it there.
    ///
    /// # Errors
    ///
    /// Fails if the item is unknown or a deletion, `to` is taken or not
    /// writable, or the disk refuses.
    pub async fn restore_aside(&self, id: u64, to: &GroupPath) -> Result<()> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        let Some((_, item)) = inner
            .state
            .aside()?
            .into_iter()
            .find(|(known, _)| *known == id)
        else {
            bail!("no set-aside item {id}");
        };
        let Some(content) = item.content else {
            bail!("set-aside item {id} is a deletion");
        };
        inner.ensure_writable(&work, to)?;
        let location = fs_path(&inner.config.root, to);
        if file_stat(&location).is_some()
            || inner
                .ledger
                .lock()
                .head(&to.key())
                .is_some_and(pigeon_core::ledger::Version::is_live)
        {
            bail!("{to} exists");
        }
        inner
            .blobs
            .export(&content.hash, &location, content.executable, true)
            .await?;
        inner
            .publish_now(&mut work, std::slice::from_ref(to))
            .await?;
        inner.state.take_aside(id)?;
        Ok(())
    }

    /// Files a set-aside item as a request to the owner of its path.
    ///
    /// # Errors
    ///
    /// Fails if the item is unknown, its path is not portable, or the
    /// request cannot be filed.
    pub async fn request_aside(
        &self,
        id: u64,
        mode: Mode,
        message: &str,
    ) -> Result<Vec<GroupPath>> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        let Some((_, item)) = inner
            .state
            .aside()?
            .into_iter()
            .find(|(known, _)| *known == id)
        else {
            bail!("no set-aside item {id}");
        };
        let path = GroupPath::parse(&item.path)?;
        let replaces = inner.ledger.lock().head(&path.key()).map(|head| head.stamp);
        let change = Change {
            path,
            content: item.content,
            replaces,
        };
        let paths = inner
            .request(&mut work, vec![change], mode, message)
            .await?;
        inner.state.take_aside(id)?;
        Ok(paths)
    }
}
