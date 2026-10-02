//! Bringing disk and ledger into agreement: how each path's disk compares
//! with the version it last matched, with the one the selection holds and
//! with what a suggestion of this machine keeps there, and carrying out the
//! step `reconcile` returns.

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
use pigeon_store::index::{IndexEntry, Seen, now_nanos, observe};
use pigeon_store::probe::{Probe, Prober};
use pigeon_store::state::Kept;

use crate::engine::{Inner, Pending, Work};
use crate::reconcile::{Disk, KeptSuggestion, Step, View, reconcile};
use crate::watch::Rescan;

/// Where a path is on disk, as the disk spells it, and whether a file is
/// there, as a probe told it.
#[derive(Clone, Debug)]
pub(crate) struct Probed {
    pub path: GroupPath,
    pub location: PathBuf,
    /// The file's metadata, `None` when no file is there.
    pub stat: Option<Stat>,
}

/// What a probe of `sought` under `root` tells pigeon to act on: a file
/// or none; nothing at an ignored path; and why nothing can be told
/// otherwise.
pub(crate) fn probed(
    root: &Path,
    sought: &GroupPath,
    probe: Probe,
) -> Result<Option<Probed>, String> {
    match probe {
        Probe::Present(found) => Ok(Some(Probed {
            path: found.path,
            location: found.location,
            stat: Some(found.stat),
        })),
        Probe::Absent => Ok(Some(Probed {
            path: sought.clone(),
            location: fs_path(root, sought),
            stat: None,
        })),
        Probe::Ignored => Ok(None),
        Probe::Unknown(reason) => Err(reason),
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

/// The version the disk should show at `key` under `cutoff`: the head
/// when followed, the version current at the time when pinned, and when
/// freed, the head only where the disk holds the path (`held`).
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
/// holds when known. A file whose metadata changed is hashed only when
/// `hash` asks for its content; one whose metadata did not is hashed again
/// only while it was last read too soon after its change to be sure.
fn compare_disk(
    probe: &Probed,
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
    let unchanged = previous.is_some_and(|seen| seen.matches(&stat));
    let seen = if hash || unchanged {
        Some(observe(&probe.location, stat, previous.as_ref())?)
    } else {
        None
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
    fell: Option<Reason>,
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
        let fell = synced_stamp
            .filter(|stamp| stamp.machine == self.me() && Some(*stamp) != target_stamp)
            .and_then(|stamp| match ledger.outcome(&stamp) {
                Some(Err(rejection)) => Some(Reason::Rejected(rejection.to_string())),
                _ if ledger
                    .unseen_versions(key)
                    .iter()
                    .any(|version| version.stamp == stamp) =>
                {
                    Some(Reason::Superseded)
                }
                _ => None,
            });
        Look {
            target,
            synced,
            fell,
        }
    }

    /// A look at the disk through the placed folders in place.
    pub(crate) fn prober(&self, work: &Work) -> Prober {
        let placed: Vec<GroupPath> = work
            .in_place()
            .into_iter()
            .map(|place| place.folder)
            .collect();
        Prober::new(&self.root, &placed)
    }

    /// Compares every path the scan, the index, and, for the whole root,
    /// the ledger know with the ledger.
    pub(crate) async fn refresh(self: &Arc<Self>, work: &mut Work, rescan: &Rescan) {
        if !work.join.syncs() {
            return;
        }
        self.lay_out(work);
        if work.root_problem.is_some() {
            return;
        }
        let under = match rescan {
            Rescan::Under(path) => Some(path.clone()),
            Rescan::All => None,
        };
        let mut prober = self.prober(work);
        let found = prober.scan(under.as_ref());
        for error in &found.errors {
            self.report(error);
        }
        let mut sought: BTreeMap<PathKey, Option<(GroupPath, Probe)>> = found
            .files
            .into_iter()
            .map(|(key, file)| (key, Some((file.path.clone(), Probe::Present(file)))))
            .collect();
        match self.state.index(under.as_ref()) {
            Ok(entries) => {
                for entry in entries {
                    sought.entry(entry.path.key()).or_insert(None);
                }
            }
            Err(error) => self.report(error),
        }
        for (key, pending) in &work.pending {
            if under
                .as_ref()
                .is_none_or(|under| pending.path.is_within(under))
            {
                sought.entry(key.clone()).or_insert(None);
            }
        }
        if under.is_none() {
            let ledger = self.ledger.lock();
            for key in ledger.keys() {
                sought.entry(key.clone()).or_insert(None);
            }
        }
        for unportable in found.unportable {
            if let Err(error) = self
                .suggest_unportable(work, &unportable.location, unportable.reason)
                .await
            {
                self.report(error);
            }
        }
        for (key, probe) in sought {
            if let Err(error) = self.sync_key(work, &mut prober, &key, probe).await {
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
        if work.root_problem.is_some() {
            return;
        }
        let mut prober = self.prober(work);
        for key in keys {
            if let Err(error) = self.sync_key(work, &mut prober, key, None).await {
                self.report(format!("{}: {error:#}", key.as_str()));
            }
        }
    }

    /// Brings one path into agreement, given the path probed there and what
    /// the probe found, or else probing the path the index, the ledger or
    /// a pending edit knows; unless it lies in a folder out of place, whose
    /// files are not where pigeon can see them. Nothing is done at an
    /// ignored path, nor where nothing can be told, which is reported.
    pub(crate) async fn sync_key(
        self: &Arc<Self>,
        work: &mut Work,
        prober: &mut Prober,
        key: &PathKey,
        probe: Option<(GroupPath, Probe)>,
    ) -> Result<()> {
        let entry = self.state.index_entry(key)?;
        let Some(probe) = self.probe_key(work, prober, key, entry.as_ref(), probe) else {
            return Ok(());
        };
        let look = self.look(work, key, &probe.path, entry.as_ref());
        let synced_stamp = entry.as_ref().and_then(|entry| entry.synced);
        let target_stamp = look.target.as_ref().map(|version| version.stamp);
        let moved = target_stamp != synced_stamp;
        let synced_content = look.synced.as_ref().and_then(|change| change.content);
        let previous = entry.as_ref().and_then(|entry| entry.seen);
        let record = self.state.kept_at(probe.path.as_str())?;
        let hash = moved || record.is_some();
        let (disk, seen) = compare_disk(&probe, previous, synced_content, hash)?;
        let renamed = probe.stat.is_some()
            && entry
                .as_ref()
                .is_some_and(|entry| entry.seen.is_some() && entry.path != probe.path);
        let disk = if renamed { Disk::Changed } else { disk };
        let disk_content = match (probe.stat, seen) {
            (None, _) => Some(None),
            (Some(_), Some(seen)) => Some(Some(seen.content)),
            (Some(_), None) => None,
        };
        let in_place = match (&look.target, probe.stat) {
            (Some(version), Some(_)) => prober.locate(&version.path).1 == probe.location,
            _ => true,
        };
        if moved
            && in_place
            && disk_content == Some(look.target.as_ref().and_then(|version| version.content))
        {
            work.pending.remove(key);
            let adopted = look.target.map(|version| IndexEntry {
                path: if probe.stat.is_some() {
                    probe.path.clone()
                } else {
                    version.path
                },
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
            _ => KeptSuggestion::No,
        };
        let view = View {
            synced: synced_stamp,
            target: target_stamp,
            kept,
            statement: is_statement(&probe.path.key()),
            fell: look.fell.clone(),
        };
        let Some(step) = reconcile(disk, &view) else {
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
        };
        if step != Step::Settle {
            work.pending.remove(key);
        }
        self.carry_out(work, prober, key, &probe, step, &look).await
    }

    /// What the disk holds at `key`: what `probe` found at the path
    /// probed, or else what a probe finds at the path the index, the
    /// ledger or a pending edit knows. Nothing is told in a folder out of
    /// place, at an ignored path or where nothing can be told, which is
    /// reported; no edit waits at such a path.
    fn probe_key(
        &self,
        work: &mut Work,
        prober: &mut Prober,
        key: &PathKey,
        entry: Option<&IndexEntry>,
        probe: Option<(GroupPath, Probe)>,
    ) -> Option<Probed> {
        let (sought, probe) = if let Some(probe) = probe {
            probe
        } else {
            let path = match entry {
                Some(entry) => entry.path.clone(),
                None => match (self.ledger.lock().head(key), work.pending.get(key)) {
                    (Some(head), _) => head.path.clone(),
                    (None, Some(pending)) => pending.path.clone(),
                    (None, None) => return None,
                },
            };
            if work.is_out_of_place(&path) {
                return None;
            }
            let probe = prober.probe(&path);
            (path, probe)
        };
        if work.is_out_of_place(&sought) {
            return None;
        }
        match probed(&self.root, &sought, probe) {
            Ok(Some(probe)) => Some(probe),
            Ok(None) => {
                work.pending.remove(key);
                None
            }
            Err(reason) => {
                work.pending.remove(key);
                self.report(reason);
                None
            }
        }
    }

    /// Whether the disk at `path`, holding `disk_content`, shows what
    /// `record` says a suggestion of this machine keeps there, and whether
    /// the group decided it; a record the disk no longer shows is
    /// forgotten, as the disk moved on.
    fn kept(
        &self,
        path: &GroupPath,
        record: &Kept,
        disk_content: Option<Content>,
    ) -> Result<KeptSuggestion> {
        if disk_content != record.content {
            self.state.unkeep(path.as_str())?;
            return Ok(KeptSuggestion::No);
        }
        let waiting = self
            .ledger
            .lock()
            .head(&record.statement.key())
            .is_some_and(Version::is_live);
        Ok(if waiting {
            KeptSuggestion::Waiting
        } else {
            KeptSuggestion::Decided
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

    /// Carries out the step of `reconcile` at `key`.
    async fn carry_out(
        self: &Arc<Self>,
        work: &mut Work,
        prober: &mut Prober,
        key: &PathKey,
        probe: &Probed,
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
                    self.suggest(work, vec![change.into()], reason).await?;
                }
            }
            Step::Materialize => {
                self.materialize(work, prober, key, probe, look.target.as_ref())
                    .await?;
            }
        }
        Ok(())
    }

    /// Makes the disk show `target`, fetching its content first if needed,
    /// into the folders the disk holds under any spelling of their names,
    /// and forgets what a suggestion of this machine kept there.
    async fn materialize(
        self: &Arc<Self>,
        work: &mut Work,
        prober: &mut Prober,
        key: &PathKey,
        probe: &Probed,
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
        let (path, location, grows) = prober.locate(&version.path);
        if probe.stat.is_some() && probe.location != location {
            disk::remove(root, &probe.location)?;
        }
        self.blobs
            .export(&content.hash, &location, content.executable)
            .await?;
        if let Some(folder) = grows {
            prober.forget(&folder);
        }
        let seen = file_stat(&location).map(|stat| Seen::read(stat, content, now_nanos()));
        let entry = IndexEntry {
            path,
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
    /// trying again, twice as long after each failure in a row, but no
    /// longer than until a session opens, and reports only the first.
    ///
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
        let failures = work.fetch_failures.get(&hash).copied().unwrap_or(0);
        let mut opened = self.node.sessions_opened();
        let inner = Arc::downgrade(self);
        while work.fetches.try_join_next().is_some() {}
        work.fetches.spawn(async move {
            let Some(engine) = inner.upgrade() else {
                return;
            };
            let fetched = engine
                .node
                .fetch(pigeon_store::blobs::blob_hash(&hash), providers)
                .await;
            if let Err(error) = &fetched {
                if failures == 0 {
                    engine.report(format!("{error:#}"));
                }
                drop(engine);
                let wait = RETRY.saturating_mul(1 << failures.min(16)).min(MAX_RETRY);
                tokio::select! {
                    () = tokio::time::sleep(wait) => {}
                    _ = opened.changed() => {}
                }
                let Some(again) = inner.upgrade() else {
                    return;
                };
                return again.release(hash, false).await;
            }
            engine.release(hash, true).await;
        });
    }

    /// Wakes the paths waiting for a blob, counting a failed fetch of it.
    async fn release(&self, hash: ContentHash, fetched: bool) {
        let mut work = self.work.lock().await;
        if fetched {
            work.fetch_failures.remove(&hash);
        } else {
            *work.fetch_failures.entry(hash).or_default() += 1;
        }
        let keys = work.fetching.remove(&hash);
        drop(work);
        if let Some(keys) = keys {
            let _ = self.wake.send(keys.into_iter().collect());
        }
    }
}

/// How long to wait before fetching again a blob that no peer delivered
/// the first time.
const RETRY: Duration = Duration::from_secs(5);
/// The longest wait before fetching again a blob that no peer delivered.
const MAX_RETRY: Duration = Duration::from_secs(600);
