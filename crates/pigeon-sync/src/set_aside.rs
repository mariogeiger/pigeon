//! Setting aside what this machine may not publish as it is: edits to
//! files it may not write, versions it published that fell, and files whose
//! names no portable path holds, each once; showing each item to the group
//! through a set-aside file, and forgetting it once someone resolved it by
//! deleting that file.

use std::path::Path;

use anyhow::Result;
use pigeon_core::clock::Stamp;
use pigeon_core::statement::{AsideItem, Reason, aside_path};
use pigeon_store::disk;
use pigeon_store::index::hash_file;

use crate::disk_sync::{Probe, file_stat};
use crate::engine::{Inner, JoinState, Work, now};

impl Inner {
    /// Adds `item` unless the list holds the same content for the path.
    pub(crate) fn record_aside(&self, item: &AsideItem) -> Result<()> {
        let known = self.state.aside()?.into_iter().any(|(_, known)| {
            known.path == item.path
                && known.content.map(|content| content.hash)
                    == item.content.map(|content| content.hash)
        });
        if !known {
            self.state.set_aside(item)?;
        }
        Ok(())
    }

    pub(crate) async fn set_aside_file(
        &self,
        work: &mut Work,
        probe: &Probe,
        replaces: Option<Stamp>,
        reason: Reason,
    ) -> Result<()> {
        let Some(stat) = probe.stat else {
            return Ok(());
        };
        let content = self.import(work, &probe.location, &stat).await?;
        self.record_aside(&AsideItem {
            path: probe.path.as_str().to_owned(),
            content: Some(content),
            replaces,
            reason,
            time: now(),
        })
    }

    /// Sets aside a file the scan could not name, once per location.
    pub(crate) async fn set_aside_unportable(
        &self,
        work: &mut Work,
        location: &Path,
        reason: String,
    ) -> Result<()> {
        let path = location
            .strip_prefix(&self.root)
            .unwrap_or(location)
            .to_string_lossy()
            .replace('\\', "/");
        let known = self
            .state
            .aside()?
            .into_iter()
            .any(|(_, item)| item.path == path && matches!(item.reason, Reason::Unportable(_)));
        let Some(stat) = file_stat(location).filter(|_| !known) else {
            return Ok(());
        };
        let content = self.import(work, location, &stat).await?;
        self.state.set_aside(&AsideItem {
            path,
            content: Some(content),
            replaces: None,
            reason: Reason::Unportable(reason),
            time: now(),
        })?;
        Ok(())
    }

    /// Publishes a set-aside file for each item that has none, and forgets
    /// each item whose file was deleted, with the file it was found in when
    /// that file's name no portable path holds and the disk still shows the
    /// item's content there.
    pub(crate) async fn share_aside(&self, work: &mut Work) -> Result<()> {
        if work.join != JoinState::Joined {
            return Ok(());
        }
        let files = self.state.aside_files()?;
        for (id, item) in self.state.aside()? {
            let Some(file) = files.get(&id) else {
                let stamp = self.clock.stamp();
                let file = aside_path(&stamp);
                let body = serde_json::to_vec_pretty(&item)?;
                self.publish_statement(work, stamp, file.clone(), body)
                    .await?;
                self.state.set_aside_file(id, &file)?;
                continue;
            };
            let deleted = self
                .ledger
                .lock()
                .head(&file.key())
                .is_some_and(|head| !head.is_live());
            if deleted {
                self.state.take_aside(id)?;
                self.remove_unportable(&item)?;
                work.protect_due = true;
            }
        }
        Ok(())
    }

    /// Removes the file whose name no portable path holds that `item` was
    /// found in, if the disk still shows the item's content there.
    fn remove_unportable(&self, item: &AsideItem) -> Result<()> {
        let (Reason::Unportable(_), Some(content)) = (&item.reason, item.content) else {
            return Ok(());
        };
        let location = self.root.join(&item.path);
        if hash_file(&location).is_ok_and(|hash| hash == content.hash) {
            disk::remove(&self.root, &location)?;
        }
        Ok(())
    }
}
