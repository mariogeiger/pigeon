//! What a person can ask of the engine besides editing the tree and the
//! selection: change the retention, publish waiting edits at once, file and
//! answer requests, and resolve set-aside items.

use std::sync::Arc;

use anyhow::{Result, bail};
use pigeon_core::patch::Change;
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::retention::Retention;
use pigeon_core::statement::{Decision, Mode};
use pigeon_store::disk::fs_path;

use crate::disk_sync::{Probe, file_stat};
use crate::engine::{Engine, Inner, JoinState, Work};

impl Inner {
    pub(crate) fn ensure_writable(&self, work: &Work, path: &GroupPath) -> Result<()> {
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
    pub(crate) async fn publish_now(
        self: &Arc<Self>,
        work: &mut Work,
        paths: &[GroupPath],
    ) -> Result<()> {
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
    /// Publishes at once the edits waiting at `under` or inside it, or
    /// everywhere.
    ///
    /// # Errors
    ///
    /// Fails if no edit waits there, the member has not joined yet, or a
    /// file is still changing, whose edit then waits anew.
    pub async fn publish(&self, under: Option<&GroupPath>) -> Result<()> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        if work.join != JoinState::Joined {
            bail!("{} has not joined the group yet", inner.config.member);
        }
        let keys: Vec<PathKey> = work
            .pending
            .keys()
            .filter(|key| under.is_none_or(|under| key.is_within(&under.key())))
            .cloned()
            .collect();
        if keys.is_empty() {
            match under {
                Some(under) => bail!("no edit waits to be published at {under}"),
                None => bail!("no edit waits to be published"),
            }
        }
        inner.publish_settled(&mut work, &keys).await;
        let changing: Vec<&str> = keys
            .iter()
            .filter_map(|key| work.pending.get(key))
            .map(|pending| pending.path.as_str())
            .collect();
        if !changing.is_empty() {
            bail!("still changing, so waiting anew: {}", changing.join(", "));
        }
        Ok(())
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

    /// Replaces this machine's retention, which the next protection pass
    /// applies.
    ///
    /// # Errors
    ///
    /// Fails if the state cannot be written.
    pub async fn set_retention(&self, retention: &Retention) -> Result<()> {
        let mut work = self.inner.work.lock().await;
        self.inner.state.set_retention(retention)?;
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
