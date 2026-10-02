//! What a person can ask of the engine besides editing the tree, the
//! selection and the suggestions: change the retention and publish
//! waiting edits at once.

use std::sync::Arc;

use anyhow::{Result, bail};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::retention::Retention;
use pigeon_store::config::Config;

use crate::engine::{Engine, Inner, Work};

impl Inner {
    /// Compares `paths` with the ledger and publishes their edits at once.
    pub(crate) async fn publish_now(
        self: &Arc<Self>,
        work: &mut Work,
        paths: &[GroupPath],
    ) -> Result<()> {
        let keys: Vec<PathKey> = paths.iter().map(GroupPath::key).collect();
        let mut prober = self.prober(work);
        for path in paths {
            let probe = prober.probe(path);
            self.sync_key(work, &mut prober, &path.key(), Some((path.clone(), probe)))
                .await?;
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
        inner.ensure_joined(&work)?;
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
}
