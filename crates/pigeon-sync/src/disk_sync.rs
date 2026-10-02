//! Bringing disk and ledger into agreement: how each path's disk compares
//! with the version it last matched, with the one the selection holds and
//! with what a suggestion of this machine keeps there, and carrying out the
//! steps `reconcile` returns.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::Result;
use pigeon_core::clock::{MachineId, Stamp, ntp_time};
use pigeon_core::ledger::{Ledger, Version};
use pigeon_core::patch::{Change, Content, ContentHash};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::selection::Cutoff;
use pigeon_core::statement::{Reason, SuggestedChange, is_statement};
use pigeon_store::disk::{self, Stat, fs_path};
use pigeon_store::index::{IndexEntry, Seen, observe};
use pigeon_store::scan::scan;
use pigeon_store::state::Kept as KeptRecord;

use crate::engine::{Inner, Pending, Wake, Work};
use crate::reconcile::{Disk, Kept, Lost, Step, View, reconcile};
use crate::watch::Rescan;

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
pub(crate) fn stat_time(stat: &Stat) -> u64 {
    let nanos = u64::try_from(stat.modified.max(0)).unwrap_or(u64::MAX);
    ntp_time(UNIX_EPOCH + Duration::from_nanos(nanos))
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
pub(crate) fn change_at(ledger: &Ledger, stamp: &Stamp, key: &PathKey) -> Option<Change> {
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
            Disk::Removed
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
        Disk::Changed
    };
    Ok((disk, seen))
}

/// What the ledger says of one path, read under its lock.
struct Look {
    target: Option<Version>,
    synced: Option<Change>,
    lost: Option<Lost>,
}

impl Inner {
    fn look(
        &self,
        work: &Work,
        key: &PathKey,
        path: &GroupPath,
        entry: Option<&IndexEntry>,
    ) -> Look {
        let ledger = self.ledger.lock();
        let cutoff = work.config.selection.cutoff(path);
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
        Look {
            target,
            synced,
            lost,
        }
    }

    /// Compares every path the scan, the index, and, for the whole root,
    /// the ledger know with the ledger.
    pub(crate) async fn refresh(self: &Arc<Self>, work: &mut Work, rescan: &Rescan) {
        if !work.join.syncs() {
            return;
        }
        self.lay_out(work);
        let root = self.root.clone();
        let under = match rescan {
            Rescan::Under(path) => Some(path.clone()),
            Rescan::All => None,
        };
        let placed: Vec<GroupPath> = work
            .in_place()
            .into_iter()
            .map(|place| place.folder)
            .collect();
        let found = scan(&root, under.as_ref(), &placed);
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
        for (key, pending) in &work.pending {
            if under
                .as_ref()
                .is_none_or(|under| pending.path.is_within(under))
            {
                probes.entry(key.clone()).or_insert(None);
            }
        }
        if under.is_none() {
            let ledger = self.ledger.lock();
            for key in ledger.keys() {
                probes.entry(key.clone()).or_insert(None);
            }
        }
        for skipped in found.skipped {
            if let Err(error) = self
                .suggest_unportable(work, &skipped.location, skipped.reason)
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
        self.lay_out(work);
        for key in keys {
            if let Err(error) = self.sync_key(work, key, None).await {
                self.report(format!("{}: {error:#}", key.as_str()));
            }
        }
    }

    /// Brings one path into agreement, unless it lies in a folder out of
    /// place, whose files are not where pigeon can see them.
    pub(crate) async fn sync_key(
        self: &Arc<Self>,
        work: &mut Work,
        key: &PathKey,
        probe: Option<Probe>,
    ) -> Result<()> {
        let entry = self.state.index_entry(key)?;
        let root = self.root.clone();
        let probe = if let Some(probe) = probe {
            probe
        } else {
            let path = match &entry {
                Some(entry) => entry.path.clone(),
                None => match (self.ledger.lock().head(key), work.pending.get(key)) {
                    (Some(head), _) => head.path.clone(),
                    (None, Some(pending)) => pending.path.clone(),
                    (None, None) => return Ok(()),
                },
            };
            Probe::at(&root, path)
        };
        if work.is_out_of_place(&probe.path) {
            return Ok(());
        }
        let look = self.look(work, key, &probe.path, entry.as_ref());
        let synced_stamp = entry.as_ref().and_then(|entry| entry.synced);
        let target_stamp = look.target.as_ref().map(|version| version.stamp);
        let moved = target_stamp != synced_stamp;
        let synced_content = look.synced.as_ref().and_then(|change| change.content);
        let previous = entry.as_ref().and_then(|entry| entry.seen);
        let record = self.state.kept_at(probe.path.as_str())?;
        let hash = moved || record.is_some();
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
            if record.is_some() {
                self.state.unkeep(probe.path.as_str())?;
            }
            return Ok(());
        }
        let kept = match (record, disk_content) {
            (Some(record), Some(content)) => self.kept(&probe.path, &record, content)?,
            _ => Kept::No,
        };
        let view = View {
            synced: synced_stamp,
            target: target_stamp,
            kept,
            statement: is_statement(&probe.path.key()),
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
            self.carry_out(work, key, &probe, step, &look).await?;
        }
        Ok(())
    }

    /// Whether the disk at `path`, holding `disk_content`, shows what
    /// `record` says a suggestion of this machine keeps there, and whether
    /// the group decided it; a record the disk no longer shows is
    /// forgotten, as the disk moved on.
    fn kept(
        &self,
        path: &GroupPath,
        record: &KeptRecord,
        disk_content: Option<Content>,
    ) -> Result<Kept> {
        if disk_content != record.content {
            self.state.unkeep(path.as_str())?;
            return Ok(Kept::No);
        }
        let waiting = self
            .ledger
            .lock()
            .head(&record.statement.key())
            .is_some_and(Version::is_live);
        Ok(if waiting {
            Kept::Waiting
        } else {
            Kept::Decided
        })
    }

    /// Suggests a file the scan could not name, once for each content the
    /// disk shows at its location.
    async fn suggest_unportable(
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
        let Some(stat) = file_stat(location) else {
            return Ok(());
        };
        if let Some(record) = self.state.kept_at(&path)? {
            let unchanged = record.content.is_some_and(|content| {
                pigeon_store::index::hash_file(location).is_ok_and(|hash| hash == content.hash)
            });
            if unchanged {
                return Ok(());
            }
        }
        let content = self.import(work, location, &stat).await?;
        let change = SuggestedChange {
            path,
            content: Some(content),
            replaces: None,
            continues: None,
        };
        self.suggest(work, vec![change], Reason::Unportable(reason))
            .await
    }

    /// Carries out one step of `reconcile` at `key`.
    async fn carry_out(
        self: &Arc<Self>,
        work: &mut Work,
        key: &PathKey,
        probe: &Probe,
        step: Step,
        look: &Look,
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
            Step::SuggestSynced(reason) => {
                if let Some(change) = &look.synced {
                    let suggested = SuggestedChange {
                        path: change.path.as_str().to_owned(),
                        content: change.content,
                        replaces: change.replaces,
                        continues: change.continues.clone(),
                    };
                    self.suggest(work, vec![suggested], reason).await?;
                }
            }
            Step::Materialize => {
                self.materialize(work, key, probe, look.target.as_ref())
                    .await?;
            }
        }
        Ok(())
    }

    /// Makes the disk show `target`, fetching its content first if needed,
    /// and forgets what a suggestion of this machine kept there.
    async fn materialize(
        self: &Arc<Self>,
        work: &mut Work,
        key: &PathKey,
        probe: &Probe,
        target: Option<&Version>,
    ) -> Result<()> {
        let root = &self.root;
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
            self.state.unkeep(probe.path.as_str())?;
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
            .export(&content.hash, &location, content.executable)
            .await?;
        let seen = file_stat(&location).map(|stat| Seen { stat, content });
        let entry = IndexEntry {
            path: version.path.clone(),
            seen,
            synced: Some(version.stamp),
        };
        self.state.update_index([(key, Some(&entry))])?;
        self.state.unkeep(probe.path.as_str())?;
        work.protect_due = true;
        Ok(())
    }

    /// Fetches a blob from its author's machine and every peer at once, then
    /// wakes the paths waiting for it; after a failure it waits before
    /// trying again.
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
