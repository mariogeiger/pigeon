//! Publishing the edits that settled: importing their files, keeping only
//! the changes the ledger would accept, signing them as one patch, and
//! freezing published drop files on disk.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use pigeon_core::patch::{Change, Content, ContentHash};
use pigeon_core::path::PathKey;
use pigeon_store::disk::{self, Stat, fs_path};
use pigeon_store::index::{IndexEntry, Seen};

use crate::disk_sync::{change_at, file_stat};
use crate::engine::{Inner, JoinState, Pending, Wake, Work};

impl Inner {
    /// Imports the file at `location` into the blob store.
    pub(crate) async fn import(
        &self,
        work: &mut Work,
        location: &Path,
        stat: &Stat,
    ) -> Result<Content> {
        let (tag, size) = self.blobs.import(location).await?;
        let content = Content {
            hash: ContentHash(*tag.hash().as_bytes()),
            size,
            executable: stat.executable.unwrap_or(false),
        };
        work.tags.push(tag);
        work.protect_due = true;
        Ok(content)
    }

    /// Publishes the edits that stayed unchanged long enough, or every
    /// edit at `at_once` without waiting, once the member has joined.
    pub(crate) async fn publish_settled(&self, work: &mut Work, at_once: &[PathKey]) {
        if work.join != JoinState::Joined {
            return;
        }
        let settled = self.take_settled(work, at_once).await;
        let (changes, entries) = self.keep_acceptable(settled);
        if changes.is_empty() {
            return;
        }
        let stamp = self.clock.stamp();
        let frozen: Vec<(PathBuf, bool)> = {
            let ledger = self.ledger.lock();
            entries
                .iter()
                .filter(|(_, entry)| ledger.freezes(&entry.path))
                .filter_map(|(_, entry)| {
                    let location = fs_path(&self.config.root, &entry.path);
                    entry.seen.map(|seen| (location, seen.content.executable))
                })
                .collect()
        };
        if let Err(error) = self.publish_at(stamp, changes, None) {
            self.report(format!("publishing: {error:#}"));
            return;
        }
        let entries: Vec<(PathKey, IndexEntry)> = entries
            .into_iter()
            .map(|(key, entry)| {
                let synced = Some(stamp);
                (key, IndexEntry { synced, ..entry })
            })
            .collect();
        if let Err(error) = self
            .state
            .update_index(entries.iter().map(|(key, entry)| (key, Some(entry))))
        {
            self.report(error);
        }
        for (location, executable) in frozen {
            if let Err(error) = disk::set_permissions(&location, executable, false) {
                self.report(error);
            }
        }
    }

    /// Takes the pending edits that settled, with the index entries they
    /// leave once published. An edit that changed again waits anew.
    async fn take_settled(
        &self,
        work: &mut Work,
        at_once: &[PathKey],
    ) -> Vec<(Change, (PathKey, IndexEntry))> {
        let mut settled = Vec::new();
        let keys: Vec<PathKey> = work.pending.keys().cloned().collect();
        for key in keys {
            let pending = work.pending[&key].clone();
            if work.is_frozen(&pending.path) {
                continue;
            }
            let settle = if self.ledger.lock().freezes(&pending.path) {
                self.options.settle_drop
            } else {
                self.options.settle_personal
            };
            if !at_once.contains(&key) && pending.since.elapsed() < settle {
                continue;
            }
            let stat = file_stat(&pending.location);
            if stat != pending.stat {
                let since = Instant::now();
                let again = Pending {
                    stat,
                    since,
                    ..pending
                };
                work.pending.insert(key, again);
                continue;
            }
            work.pending.remove(&key);
            match self.prepare(work, &key, &pending).await {
                Ok(Some((change, entry))) => settled.push((change, (key, entry))),
                Ok(None) => {}
                Err(error) => self.report(format!("{}: {error:#}", key.as_str())),
            }
        }
        settled
    }

    /// Keeps the changes the ledger accepts together; the paths of the
    /// others are compared again, which sets their edits aside.
    fn keep_acceptable(
        &self,
        settled: Vec<(Change, (PathKey, IndexEntry))>,
    ) -> (Vec<Change>, Vec<(PathKey, IndexEntry)>) {
        let ledger = self.ledger.lock();
        let accepts = |changes: &[Change]| {
            let member = &self.config.member;
            ledger
                .check(member, &self.config.cert.member, changes, false)
                .is_ok()
        };
        let all: Vec<Change> = settled.iter().map(|(change, _)| change.clone()).collect();
        let (kept, refused): (Vec<_>, Vec<_>) = if accepts(&all) {
            (settled, Vec::new())
        } else {
            settled
                .into_iter()
                .partition(|(change, _)| accepts(std::slice::from_ref(change)))
        };
        if !refused.is_empty() {
            let keys = refused.into_iter().map(|(_, (key, _))| key).collect();
            let _ = self.wake.send(Wake::Keys(keys));
        }
        kept.into_iter().unzip()
    }

    /// The change a settled edit makes, with the index entry it leaves, or
    /// `None` when the file only changed its time.
    async fn prepare(
        &self,
        work: &mut Work,
        key: &PathKey,
        pending: &Pending,
    ) -> Result<Option<(Change, IndexEntry)>> {
        let entry = self.state.index_entry(key)?;
        let synced = entry.as_ref().and_then(|entry| entry.synced);
        let synced_content = synced.and_then(|stamp| {
            change_at(&self.ledger.lock(), &stamp, key).and_then(|change| change.content)
        });
        let seen = match pending.stat {
            Some(stat) => Some(Seen {
                stat,
                content: self.import(work, &pending.location, &stat).await?,
            }),
            None => None,
        };
        let content = seen.map(|seen| seen.content);
        let entry = IndexEntry {
            path: pending.path.clone(),
            seen,
            synced,
        };
        if content == synced_content {
            if entry.synced.is_some() || entry.seen.is_some() {
                self.state.update_index([(key, Some(&entry))])?;
            }
            return Ok(None);
        }
        let change = Change {
            path: pending.path.clone(),
            content,
            replaces: synced,
        };
        Ok(Some((change, entry)))
    }
}
