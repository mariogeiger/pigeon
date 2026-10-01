//! What a person can ask of the engine besides editing the tree and the
//! selection: change the retention, publish waiting edits at once, file and
//! answer requests, and resolve the set-aside items of any machine through
//! requests to their members.

use std::sync::Arc;

use anyhow::{Result, bail};
use pigeon_core::ledger::Version;
use pigeon_core::patch::Change;
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::retention::Retention;
use pigeon_core::statement::{AsideItem, Decision, Mode, STATEMENTS, aside_folder};
use pigeon_store::config::Config;
use pigeon_store::disk::fs_path;

use crate::disk_sync::{Probe, file_stat};
use crate::engine::{Engine, Inner, JoinState, Work};

impl Inner {
    /// Compares `paths` with the ledger and publishes their edits at once.
    pub(crate) async fn publish_now(
        self: &Arc<Self>,
        work: &mut Work,
        paths: &[GroupPath],
    ) -> Result<()> {
        let keys: Vec<PathKey> = paths.iter().map(GroupPath::key).collect();
        for path in paths {
            let probe = Probe::at(&self.root, path.clone());
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
            bail!("{} has not joined the group yet", inner.member);
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
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        inner
            .request(&mut work, changes, mode, message, &inner.member)
            .await
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

    /// Replaces this machine's retention, which the next protection pass
    /// applies.
    ///
    /// # Errors
    ///
    /// Fails if the configuration cannot be written.
    pub async fn set_retention(&self, retention: &Retention) -> Result<()> {
        let mut work = self.inner.work.lock().await;
        let mut config = Config::clone(&work.config);
        config.retention = *retention;
        work.config.save(config)?;
        work.protect_due = true;
        Ok(())
    }

    /// Resolves the set-aside item at `file` by deleting the file: asks its
    /// member, forcing it.
    ///
    /// # Errors
    ///
    /// Fails if no set-aside item lies at `file` or the request cannot be
    /// filed.
    pub async fn discard_aside(&self, file: &GroupPath) -> Result<Vec<GroupPath>> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        let (item, version) = inner.aside_file(file).await?;
        let message = format!("discards what was set aside at {}", item.path);
        inner
            .request(
                &mut work,
                vec![deletion(&version)],
                Mode::Force,
                &message,
                &version.owner,
            )
            .await
    }

    /// Restores the set-aside item at `file` to the free path `to`, which
    /// belongs to its member unless it names its owner, and resolves the
    /// item: asks the owners, forcing it.
    ///
    /// # Errors
    ///
    /// Fails if no set-aside item lies at `file`, it is a deletion, `to`
    /// is taken here or lies in the statements folder, or the request
    /// cannot be filed.
    pub async fn restore_aside(&self, file: &GroupPath, to: &GroupPath) -> Result<Vec<GroupPath>> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        let (item, version) = inner.aside_file(file).await?;
        let Some(content) = item.content else {
            bail!("{file} sets a deletion aside");
        };
        if to.is_inside(STATEMENTS) {
            bail!("{to} lies in the statements folder");
        }
        if file_stat(&fs_path(&inner.root, to)).is_some()
            || inner
                .ledger
                .lock()
                .head(&to.key())
                .is_some_and(Version::is_live)
        {
            bail!("{to} exists");
        }
        let restored = Change {
            path: to.clone(),
            content: Some(content),
            replaces: None,
        };
        let message = format!("restores what was set aside at {} to {to}", item.path);
        inner
            .request(
                &mut work,
                vec![restored, deletion(&version)],
                Mode::Force,
                &message,
                &version.owner,
            )
            .await
    }

    /// Files the set-aside item at `file` as a request to the owner of its
    /// path, and resolves the item: asks its member, forcing it.
    ///
    /// # Errors
    ///
    /// Fails if no set-aside item lies at `file`, its path is not portable,
    /// or the requests cannot be filed.
    pub async fn request_aside(
        &self,
        file: &GroupPath,
        mode: Mode,
        message: &str,
    ) -> Result<Vec<GroupPath>> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        let (item, version) = inner.aside_file(file).await?;
        let path = GroupPath::parse(&item.path)?;
        let replaces = inner.ledger.lock().head(&path.key()).map(|head| head.stamp);
        let change = Change {
            path,
            content: item.content,
            replaces,
        };
        let member = &version.owner;
        let mut requests = inner
            .request(&mut work, vec![change], mode, message, member)
            .await?;
        requests.extend(
            inner
                .request(
                    &mut work,
                    vec![deletion(&version)],
                    Mode::Force,
                    message,
                    member,
                )
                .await?,
        );
        Ok(requests)
    }
}

impl Inner {
    /// The set-aside item at `file`, and the version of the file.
    pub(crate) async fn aside_file(&self, file: &GroupPath) -> Result<(AsideItem, Version)> {
        let head = self.ledger.lock().head(&file.key()).cloned();
        match head {
            Some(version) if file.is_inside(&aside_folder()) => match version.content {
                Some(content) => Ok((self.read_statement(&content).await?, version)),
                None => bail!("the set-aside item at {file} was resolved"),
            },
            _ => bail!("no set-aside item at {file}"),
        }
    }
}

/// The change that deletes `version`'s file.
fn deletion(version: &Version) -> Change {
    Change {
        path: version.path.clone(),
        content: None,
        replaces: Some(version.stamp),
    }
}
