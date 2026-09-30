//! Keeping on this machine what it may not publish as it is: edits to
//! files it may not write, versions it published that fell, and files whose
//! names no portable path holds, each once.

use std::path::Path;

use anyhow::Result;
use pigeon_core::clock::Stamp;
use pigeon_store::aside::{AsideItem, Reason};

use crate::disk_sync::{Probe, file_stat};
use crate::engine::{Inner, Work, now};

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
            .strip_prefix(&self.config.root)
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
}
