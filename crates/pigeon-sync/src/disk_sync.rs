//! Bringing disk and ledger into agreement: how each path's disk compares
//! with the version it last matched and with the one the selection holds,
//! and carrying out the steps `reconcile` returns, then publishing the edits
//! that settled.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::Result;
use pigeon_core::clock::{MachineId, Stamp, ntp_time};
use pigeon_core::ledger::{Ledger, Version};
use pigeon_core::patch::{Change, Content, ContentHash};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_core::statement::{STATEMENTS, member_path};
use pigeon_store::aside::AsideItem;
use pigeon_store::disk::{self, Stat, fs_path};
use pigeon_store::index::{IndexEntry, Seen, observe};
use pigeon_store::scan::scan;

use crate::engine::{Inner, Pending, Wake, Work, now};
use crate::reconcile::{Disk, Lost, Step, View, reconcile};
use crate::watch::Rescan;

/// Stands for any content when asking the ledger whether a path is
/// writable.
const ANY_CONTENT: Content = Content {
    hash: ContentHash([0; 32]),
    size: 0,
    executable: false,
};

/// Where a path is on disk and what is there.
#[derive(Clone, Debug)]
pub(crate) struct Probe {
    pub path: GroupPath,
    pub location: PathBuf,
    /// The file's metadata, `None` when no file is there.
    pub stat: Option<Stat>,
}

impl Probe {
    pub(crate) fn at(root: &Path, path: GroupPath) -> Self {
        let location = fs_path(root, &path);
        let stat = file_stat(&location);
        Self {
            path,
            location,
            stat,
        }
    }
}

/// The metadata of a regular file at `location`, not following links.
pub(crate) fn file_stat(location: &Path) -> Option<Stat> {
    std::fs::symlink_metadata(location)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|metadata| Stat::of(&metadata))
}

/// A modification time in NTP64.
fn stat_time(stat: &Stat) -> u64 {
    let nanos = u64::try_from(stat.modified.max(0)).unwrap_or(u64::MAX);
    ntp_time(UNIX_EPOCH + Duration::from_nanos(nanos))
}

/// The gitignore pattern that matches exactly `path`.
pub(crate) fn exact_pattern(path: &GroupPath) -> String {
    let mut pattern = String::from("/");
    for character in path.as_str().chars() {
        if matches!(character, '[' | ']' | '*' | '?' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern
}

/// The version `selection` holds at `key`.
pub(crate) fn target(
    ledger: &Ledger,
    key: &PathKey,
    cutoff: Cutoff,
    held: bool,
) -> Option<Version> {
    match cutoff {
        Cutoff::PlusInfinity => ledger.head(key).cloned(),
        Cutoff::At(time) => ledger.version_at(key, time).cloned(),
        Cutoff::MinusInfinity => ledger.head(key).filter(|_| held).cloned(),
    }
}

/// The change a patch made at `key`.
pub(crate) fn change_at(
    ledger: &Ledger,
    stamp: &pigeon_core::clock::Stamp,
    key: &PathKey,
) -> Option<Change> {
    ledger
        .patch(stamp)?
        .patch
        .changes
        .iter()
        .find(|change| change.path.key() == *key)
        .cloned()
}

/// How the disk at `probe` compares with the synced content, and what it
/// holds when known. The file is hashed only when its metadata changed and
/// `hash` asks for its content.
fn compare_disk(
    probe: &Probe,
    previous: Option<Seen>,
    synced: Option<Content>,
    hash: bool,
) -> Result<(Disk, Option<Seen>)> {
    let Some(stat) = probe.stat else {
        let disk = if synced.is_some() {
            Disk::Removed { at: now() }
        } else {
            Disk::Unchanged
        };
        return Ok((disk, None));
    };
    let seen = match previous.filter(|seen| seen.stat == stat) {
        Some(seen) => Some(seen),
        None if !hash => None,
        None => Some(observe(&probe.location, stat, previous.as_ref())?),
    };
    let disk = if seen.is_some_and(|seen| Some(seen.content) == synced) {
        Disk::Unchanged
    } else {
        Disk::Changed {
            at: stat_time(&stat),
        }
    };
    Ok((disk, seen))
}

/// What the ledger says of one path, read under its lock.
struct Look {
    target: Option<Version>,
    synced: Option<Change>,
    writable: bool,
    lost: Option<Lost>,
}

impl Inner {
    /// Whether this machine may publish a change at `path` now.
    pub(crate) fn writable(&self, ledger: &Ledger, work: &Work, path: &GroupPath) -> bool {
        if !work.join.syncs()
            || path.is_inside(STATEMENTS)
            || matches!(work.selection.cutoff(path), Cutoff::At(_))
        {
            return false;
        }
        let name = &self.config.member;
        let mut changes = vec![Change {
            path: path.clone(),
            content: Some(ANY_CONTENT),
            replaces: ledger.head(&path.key()).map(|version| version.stamp),
        }];
        if !ledger.members().contains_key(name) {
            changes.push(Change {
                path: member_path(name),
                content: Some(ANY_CONTENT),
                replaces: None,
            });
        }
        ledger
            .check(name, &self.config.cert.member, &changes, false)
            .is_ok()
    }

    fn look(
        &self,
        work: &Work,
        key: &PathKey,
        path: &GroupPath,
        entry: Option<&IndexEntry>,
    ) -> Look {
        let ledger = self.ledger.lock();
        let cutoff = work.selection.cutoff(path);
        let target = target(&ledger, key, cutoff, entry.is_some());
        let synced_stamp = entry.and_then(|entry| entry.synced);
        let synced = synced_stamp.and_then(|stamp| change_at(&ledger, &stamp, key));
        let target_stamp = target.as_ref().map(|version| version.stamp);
        let lost = synced_stamp
            .filter(|stamp| stamp.machine == self.me() && Some(*stamp) != target_stamp)
            .and_then(|stamp| match ledger.outcome(&stamp) {
                Some(Err(rejection)) => Some(Lost::Rejected(rejection.to_string())),
                _ if ledger
                    .unseen_versions(key)
                    .iter()
                    .any(|version| version.stamp == stamp) =>
                {
                    Some(Lost::Superseded)
                }
                _ => None,
            });
        let writable = self.writable(&ledger, work, path);
        Look {
            target,
            synced,
            writable,
            lost,
        }
    }

    /// Compares every path the scan, the index, and, for the whole root,
    /// the ledger know with the ledger.
    pub(crate) async fn refresh(self: &Arc<Self>, work: &mut Work, rescan: &Rescan) {
        if !work.join.syncs() {
            return;
        }
        let root = self.config.root.clone();
        let under = match rescan {
            Rescan::Under(path) => Some(path.clone()),
            Rescan::All => None,
        };
        let found = scan(&root, under.as_ref());
        let mut probes: BTreeMap<PathKey, Option<Probe>> = found
            .files
            .into_iter()
            .map(|(key, file)| {
                let probe = Probe {
                    path: file.path,
                    location: file.location,
                    stat: Some(file.stat),
                };
                (key, Some(probe))
            })
            .collect();
        match self.state.index(under.as_ref()) {
            Ok(entries) => {
                for entry in entries {
                    probes.entry(entry.path.key()).or_insert(None);
                }
            }
            Err(error) => self.report(error),
        }
        if under.is_none() {
            let ledger = self.ledger.lock();
            for key in ledger.keys() {
                probes.entry(key.clone()).or_insert(None);
            }
        }
        for skipped in found.skipped {
            if let Err(error) = self
                .set_aside_unportable(work, &skipped.location, skipped.reason)
                .await
            {
                self.report(error);
            }
        }
        for (key, probe) in probes {
            if let Err(error) = self.sync_key(work, &key, probe).await {
                self.report(format!("{}: {error:#}", key.as_str()));
            }
        }
    }

    /// Compares the given paths with the ledger.
    pub(crate) async fn refresh_keys(self: &Arc<Self>, work: &mut Work, keys: &[PathKey]) {
        if !work.join.syncs() {
            return;
        }
        for key in keys {
            if let Err(error) = self.sync_key(work, key, None).await {
                self.report(format!("{}: {error:#}", key.as_str()));
            }
        }
    }

    /// Brings one path into agreement.
    pub(crate) async fn sync_key(
        self: &Arc<Self>,
        work: &mut Work,
        key: &PathKey,
        probe: Option<Probe>,
    ) -> Result<()> {
        let entry = self.state.index_entry(key)?;
        let root = self.config.root.clone();
        let probe = if let Some(probe) = probe {
            probe
        } else {
            let path = match &entry {
                Some(entry) => entry.path.clone(),
                None => match self.ledger.lock().head(key) {
                    Some(head) => head.path.clone(),
                    None => return Ok(()),
                },
            };
            Probe::at(&root, path)
        };
        let look = self.look(work, key, &probe.path, entry.as_ref());
        let synced_stamp = entry.as_ref().and_then(|entry| entry.synced);
        let target_stamp = look.target.as_ref().map(|version| version.stamp);
        let moved = target_stamp != synced_stamp;
        let synced_content = look.synced.as_ref().and_then(|change| change.content);
        let previous = entry.as_ref().and_then(|entry| entry.seen);
        let hash = !look.writable || moved;
        let (disk, seen) = compare_disk(&probe, previous, synced_content, hash)?;
        let disk_content = match (probe.stat, seen) {
            (None, _) => Some(None),
            (Some(_), Some(seen)) => Some(Some(seen.content)),
            (Some(_), None) => None,
        };
        if moved && disk_content == Some(look.target.as_ref().and_then(|version| version.content)) {
            work.pending.remove(key);
            let adopted = look.target.map(|version| IndexEntry {
                path: version.path,
                seen,
                synced: Some(version.stamp),
            });
            self.state.update_index([(key, adopted.as_ref())])?;
            return Ok(());
        }
        let view = View {
            synced: synced_stamp,
            target: target_stamp,
            writable: look.writable,
            lost: look.lost.clone(),
        };
        let steps = reconcile(disk, &view);
        if steps.is_empty() {
            work.pending.remove(key);
            if let Some(entry) = &entry
                && entry.seen != seen
                && (seen.is_some() || probe.stat.is_none())
            {
                let refreshed = IndexEntry {
                    seen,
                    ..entry.clone()
                };
                self.state.update_index([(key, Some(&refreshed))])?;
            }
            return Ok(());
        }
        if !steps.contains(&Step::Settle) {
            work.pending.remove(key);
        }
        for step in steps {
            self.carry_out(work, key, &probe, step, &look, synced_stamp)
                .await?;
        }
        Ok(())
    }

    /// Carries out one step of `reconcile` at `key`.
    async fn carry_out(
        self: &Arc<Self>,
        work: &mut Work,
        key: &PathKey,
        probe: &Probe,
        step: Step,
        look: &Look,
        synced: Option<Stamp>,
    ) -> Result<()> {
        match step {
            Step::Settle => {
                let since = match work.pending.get(key) {
                    Some(pending) if pending.stat == probe.stat => pending.since,
                    _ => Instant::now(),
                };
                let pending = Pending {
                    path: probe.path.clone(),
                    location: probe.location.clone(),
                    stat: probe.stat,
                    since,
                };
                work.pending.insert(key.clone(), pending);
            }
            Step::SetAsideDisk(reason) => {
                self.set_aside_file(work, probe, synced, reason).await?;
            }
            Step::SetAsideSynced(reason) => {
                if let Some(change) = &look.synced {
                    self.record_aside(&AsideItem {
                        path: change.path.as_str().to_owned(),
                        content: change.content,
                        replaces: change.replaces,
                        reason,
                        time: now(),
                    })?;
                    work.protect_due = true;
                }
            }
            Step::Materialize => {
                self.materialize(work, key, probe, look.target.as_ref(), look.writable)
                    .await?;
            }
            Step::Exclude => {
                work.selection.set(Rule {
                    pattern: exact_pattern(&probe.path),
                    cutoff: Cutoff::MinusInfinity,
                })?;
                self.state.set_selection(&work.selection)?;
                self.state.update_index([(key, None)])?;
            }
        }
        Ok(())
    }

    /// Makes the disk show `target`, fetching its content first if needed.
    async fn materialize(
        self: &Arc<Self>,
        work: &mut Work,
        key: &PathKey,
        probe: &Probe,
        target: Option<&Version>,
        writable: bool,
    ) -> Result<()> {
        let root = &self.config.root;
        let Some((version, content)) =
            target.and_then(|version| version.content.map(|content| (version, content)))
        else {
            if probe.stat.is_some() {
                disk::remove(root, &probe.location)?;
            }
            let deleted = target.map(|version| IndexEntry {
                path: version.path.clone(),
                seen: None,
                synced: Some(version.stamp),
            });
            self.state.update_index([(key, deleted.as_ref())])?;
            return Ok(());
        };
        if !self.blobs.has(&content.hash).await? {
            self.fetch(work, content.hash, version.stamp.machine, key.clone());
            return Ok(());
        }
        let location = fs_path(root, &version.path);
        if probe.stat.is_some() && probe.location != location {
            disk::remove(root, &probe.location)?;
        }
        self.blobs
            .export(&content.hash, &location, content.executable, writable)
            .await?;
        let seen = file_stat(&location).map(|stat| Seen { stat, content });
        let entry = IndexEntry {
            path: version.path.clone(),
            seen,
            synced: Some(version.stamp),
        };
        self.state.update_index([(key, Some(&entry))])?;
        work.protect_due = true;
        Ok(())
    }

    /// Fetches a blob from its author's machine or any peer, then wakes the
    /// paths waiting for it; after a failure it waits before trying again.
    /// The blob is protected from the start, so that garbage collection
    /// never takes it before the paths waiting for it protect it.
    pub(crate) fn fetch(
        self: &Arc<Self>,
        work: &mut Work,
        hash: ContentHash,
        author: MachineId,
        key: PathKey,
    ) {
        let waiting = work.fetching.entry(hash).or_default();
        let started = waiting.is_empty();
        waiting.insert(key);
        if !started {
            return;
        }
        self.blobs
            .protect_also(pigeon_store::blobs::blob_hash(&hash));
        let mut providers = vec![author];
        providers.extend(self.node.peers().into_iter().filter(|peer| *peer != author));
        providers.retain(|provider| *provider != self.me());
        let inner = Arc::downgrade(self);
        tokio::spawn(async move {
            let Some(engine) = inner.upgrade() else {
                return;
            };
            let fetched = engine
                .node
                .fetch(pigeon_store::blobs::blob_hash(&hash), providers)
                .await;
            if let Err(error) = &fetched {
                engine.report(format!("{error:#}"));
                drop(engine);
                tokio::time::sleep(RETRY).await;
                let Some(again) = inner.upgrade() else {
                    return;
                };
                return again.release(hash).await;
            }
            engine.release(hash).await;
        });
    }

    async fn release(&self, hash: ContentHash) {
        let keys = self.work.lock().await.fetching.remove(&hash);
        if let Some(keys) = keys {
            let _ = self.wake.send(Wake::Keys(keys.into_iter().collect()));
        }
    }
}

/// How long to wait before fetching again a blob that no peer delivered.
const RETRY: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exact_pattern_matches_only_its_path() {
        let path = GroupPath::parse("a/[b] c.txt").unwrap();
        let pattern = exact_pattern(&path);
        assert_eq!(pattern, "/a/\\[b\\] c.txt");
        let matcher = pigeon_core::selection::compile(&pattern).unwrap();
        assert!(pigeon_core::selection::matches(&matcher, &path));
        let other = GroupPath::parse("a/b c.txt").unwrap();
        assert!(!pigeon_core::selection::matches(&matcher, &other));
    }
}
