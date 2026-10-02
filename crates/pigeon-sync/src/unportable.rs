//! The names on disk that no group path holds, which pigeon keeps out of
//! the group and never renames by itself: the scans find them, each with
//! the portable name closest to it that its folder leaves free, and a
//! person renames one to that name, which brings what it holds into the
//! group.

use anyhow::{Context, Result, bail};
use pigeon_core::path::GroupPath;
use pigeon_store::disk;
use pigeon_store::probe::Unportable;
use serde::Serialize;

use crate::engine::{Engine, Work};
use crate::watch::Rescan;

/// A name kept out of the group, as a person may rename it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct UnportableView {
    /// Where it lies, its name as the disk spells it.
    pub path: String,
    /// Why no group path holds it.
    pub reason: String,
    /// The path renaming it gives it.
    pub proposal: String,
}

impl Work {
    /// Takes the names a scan of `under`, or of the whole root, kept out,
    /// in place of those it saw before.
    pub(crate) fn note_unportable(&mut self, under: Option<&GroupPath>, found: Vec<Unportable>) {
        match under {
            Some(under) => self.unportable.retain(|_, known| !known.lies_within(under)),
            None => self.unportable.clear(),
        }
        self.unportable.extend(
            found
                .into_iter()
                .map(|unportable| (unportable.path(), unportable)),
        );
    }
}

impl Engine {
    /// The names the last scans kept out of the group, by path.
    pub async fn unportable(&self) -> Vec<UnportableView> {
        let work = self.inner.work.lock().await;
        work.unportable
            .values()
            .map(|unportable| UnportableView {
                path: unportable.path(),
                reason: unportable.reason.clone(),
                proposal: unportable
                    .proposed()
                    .map_or_else(|_| unportable.proposal.clone(), String::from),
            })
            .collect()
    }

    /// Renames on disk the name at `path` that no group path holds to the
    /// portable name proposed for it as the disk shows it now, then
    /// compares what it holds with the ledger: the path it takes.
    ///
    /// # Errors
    ///
    /// Fails if the group waits for its root, no name kept out lies at
    /// `path`, the proposed name is taken, or the rename fails.
    pub async fn make_portable(&self, path: &str) -> Result<GroupPath> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        if let Some(problem) = &work.root_problem {
            bail!("{problem}");
        }
        let Some(known) = work.unportable.get(path).cloned() else {
            bail!("no name kept out of the group lies at {path}");
        };
        let fresh = inner
            .prober(&work)
            .unportable_at(known.folder.as_ref(), &known.name);
        let Some(fresh) = fresh else {
            work.unportable.remove(path);
            bail!("{path} is no longer a name kept out of the group");
        };
        let proposed = fresh.proposed()?;
        let to = fresh.location.with_file_name(&fresh.proposal);
        let renamed = disk::rename(&inner.root, &fresh.location, &to)
            .with_context(|| format!("renaming {path} to {proposed}"))?;
        if !renamed {
            bail!("{proposed} exists");
        }
        work.unportable.remove(path);
        inner
            .refresh(&mut work, &Rescan::Under(proposed.clone()))
            .await;
        Ok(proposed)
    }
}
