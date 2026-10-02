//! Publishing the edits that settled: importing their files, pairing each
//! file that vanished with one of the same content that appeared, as its
//! move, publishing as one patch what the rules let this machine publish by
//! itself, and suggesting the rest to the group.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use pigeon_core::clock::Stamp;
use pigeon_core::ledger::Version;
use pigeon_core::patch::{Change, Content, ContentHash, VersionRef};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::selection::Cutoff;
use pigeon_core::statement::{Reason, STATEMENTS, SuggestedChange};
use pigeon_store::disk::Stat;
use pigeon_store::index::{IndexEntry, Seen, hash_file};

use crate::disk_sync::{change_at, file_stat, stat_time};
use crate::engine::{Inner, JoinState, Pending, Work, now};

/// An edit that settled: the change it makes, the index entry it leaves,
/// when the disk changed, in NTP64, and what it does to the content the
/// disk last matched.
struct Settled {
    key: PathKey,
    change: Change,
    entry: IndexEntry,
    edited: u64,
    shape: Shape,
}

/// What an edit does to the content the disk last matched.
enum Shape {
    /// The file of this version vanished, with this content.
    Vanished(VersionRef, Content),
    /// A file appeared where the disk held none.
    Appeared,
    Other,
}

impl Shape {
    /// The shape of an edit leaving a file or not, `present`, where the
    /// disk last matched `synced`, the change of the patch stamped so.
    fn of(synced: Option<(Stamp, Change)>, present: bool) -> Self {
        let synced = synced.and_then(|(stamp, change)| {
            change.content.map(|content| {
                let from = VersionRef {
                    path: change.path,
                    stamp,
                };
                (from, content)
            })
        });
        match (synced, present) {
            (Some((from, content)), false) => Self::Vanished(from, content),
            (None, true) => Self::Appeared,
            _ => Self::Other,
        }
    }

    fn vanished(&self) -> Option<(&VersionRef, &Content)> {
        match self {
            Self::Vanished(from, content) => Some((from, content)),
            Self::Appeared | Self::Other => None,
        }
    }
}

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

    /// Whether a file at `path` would be a new drop: outside the statements
    /// folder, in no member's personal path, where no file lives.
    pub(crate) fn is_new_drop(&self, path: &GroupPath) -> bool {
        let ledger = self.ledger.lock();
        !path.is_inside(STATEMENTS)
            && ledger.owner(path).is_none()
            && !ledger.head(&path.key()).is_some_and(Version::is_live)
    }

    /// How long an edit at `path` must stay unchanged before it is
    /// published: longer for a new drop, a draft until then.
    pub(crate) fn settle_time(&self, path: &GroupPath) -> Duration {
        if self.is_new_drop(path) {
            self.options.settle_drop
        } else {
            self.options.settle_personal
        }
    }

    /// Publishes the edits that stayed unchanged long enough, or every
    /// edit at `at_once` without waiting, once the member has joined, and
    /// suggests those the rules leave to the group.
    pub(crate) async fn publish_settled(&self, work: &mut Work, at_once: &[PathKey]) {
        if work.join != JoinState::Joined {
            return;
        }
        let due = self.due(work, at_once);
        let settled = self.take_settled(work, &due).await;
        let mut automatic = Vec::new();
        for unit in pair_moves(settled) {
            match self.reason(work, &unit) {
                Some(reason) => self.suggest_settled(work, unit, reason).await,
                None => automatic.push(unit),
            }
        }
        let (accepted, refused) = self.keep_acceptable(automatic);
        for (unit, rejection) in refused {
            self.suggest_settled(work, unit, Reason::Rejected(rejection))
                .await;
        }
        if accepted.is_empty() {
            return;
        }
        let stamp = self.clock.stamp();
        let changes = accepted
            .iter()
            .map(|settled| settled.change.clone())
            .collect();
        if let Err(error) = self.publish_at(stamp, changes) {
            self.report(format!("publishing: {error:#}"));
            return;
        }
        let entries: Vec<(PathKey, IndexEntry)> = accepted
            .into_iter()
            .map(|settled| {
                let synced = Some(stamp);
                (
                    settled.key,
                    IndexEntry {
                        synced,
                        ..settled.entry
                    },
                )
            })
            .collect();
        if let Err(error) = self
            .state
            .update_index(entries.iter().map(|(key, entry)| (key, Some(entry))))
        {
            self.report(error);
        }
    }

    /// The stamp and change of the version the disk last matched at `key`.
    fn synced_at(&self, key: &PathKey) -> Result<Option<(Stamp, Change)>> {
        let synced = self.state.index_entry(key)?.and_then(|entry| entry.synced);
        Ok(synced.and_then(|stamp| {
            change_at(&self.ledger.lock(), &stamp, key).map(|change| (stamp, change))
        }))
    }

    /// The pending edits due now: those that stayed unchanged long enough
    /// or are asked for at once, with the other half of each move one of
    /// them makes, a vanished file and a file of the same content that
    /// appeared.
    fn due(&self, work: &Work, at_once: &[PathKey]) -> BTreeSet<PathKey> {
        let mut due: BTreeSet<PathKey> = work
            .pending
            .iter()
            .filter(|(key, pending)| {
                at_once.contains(key) || pending.since.elapsed() >= self.settle_time(&pending.path)
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut vanished = Vec::new();
        let mut appeared = Vec::new();
        for (key, pending) in &work.pending {
            let synced = self.synced_at(key).ok().flatten();
            match Shape::of(synced, pending.stat.is_some()) {
                Shape::Vanished(_, content) => vanished.push((key, content)),
                Shape::Appeared => appeared.push((key, pending)),
                Shape::Other => {}
            }
        }
        let mut paired = BTreeSet::new();
        for (from, content) in vanished {
            let Some((to, _)) = appeared.iter().find(|(to, pending)| {
                !paired.contains(*to)
                    && (due.contains(from) || due.contains(*to))
                    && pending.stat.is_some_and(|stat| stat.size == content.size)
                    && hash_file(&pending.location).is_ok_and(|hash| hash == content.hash)
            }) else {
                continue;
            };
            paired.insert((*to).clone());
            due.insert(from.clone());
            due.insert((*to).clone());
        }
        due
    }

    /// Takes the pending edits at `due`, with the changes they make. An
    /// edit that changed again waits anew.
    async fn take_settled(&self, work: &mut Work, due: &BTreeSet<PathKey>) -> Vec<Settled> {
        let mut settled = Vec::new();
        for key in due {
            let Some(pending) = work.pending.get(key).cloned() else {
                continue;
            };
            if work.is_out_of_place(&pending.path) {
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
                work.pending.insert(key.clone(), again);
                continue;
            }
            work.pending.remove(key);
            match self.prepare(work, key, &pending).await {
                Ok(Some(edit)) => settled.push(edit),
                Ok(None) => {}
                Err(error) => self.report(format!("{}: {error:#}", key.as_str())),
            }
        }
        settled
    }

    /// Why the rules leave `unit` to the group, if they do: an edit made
    /// before a version this machine had not seen yet lost to it, and
    /// this machine publishes by itself only its member's personal files
    /// and new drops, outside the statements folder and pinned files.
    fn reason(&self, work: &Work, unit: &[Settled]) -> Option<Reason> {
        let ledger = self.ledger.lock();
        let superseded = unit.iter().any(|settled| {
            ledger.head(&settled.key).is_some_and(|head| {
                Some(head.stamp) != settled.entry.synced && settled.edited <= head.stamp.time
            })
        });
        if superseded {
            return Some(Reason::Superseded);
        }
        let automatic = unit.iter().all(|settled| {
            let path = &settled.change.path;
            let own = match ledger.owner(path) {
                Some(owner) => owner == self.member,
                None => {
                    settled.change.content.is_some()
                        && !ledger.head(&settled.key).is_some_and(Version::is_live)
                }
            };
            own && !path.is_inside(STATEMENTS)
                && !matches!(work.config.selection.cutoff(path), Cutoff::At(_))
        });
        (!automatic).then_some(Reason::OutsideRules)
    }

    /// Suggests `unit` for `reason`; the disk keeps what it holds.
    async fn suggest_settled(&self, work: &mut Work, unit: Vec<Settled>, reason: Reason) {
        let changes = unit
            .iter()
            .map(|settled| SuggestedChange {
                path: settled.change.path.as_str().to_owned(),
                content: settled.change.content,
                replaces: settled.change.replaces,
                continues: settled.change.continues.clone(),
            })
            .collect();
        if let Err(error) = self.suggest(work, changes, reason).await {
            self.report(format!("suggesting: {error:#}"));
            return;
        }
        let entries: Vec<(PathKey, IndexEntry)> = unit
            .into_iter()
            .map(|settled| (settled.key, settled.entry))
            .collect();
        if let Err(error) = self
            .state
            .update_index(entries.iter().map(|(key, entry)| (key, Some(entry))))
        {
            self.report(error);
        }
    }

    /// Keeps the units the ledger accepts together, and the others with
    /// why it refuses each.
    fn keep_acceptable(
        &self,
        units: Vec<Vec<Settled>>,
    ) -> (Vec<Settled>, Vec<(Vec<Settled>, String)>) {
        let ledger = self.ledger.lock();
        let check = |units: &[&Vec<Settled>]| {
            let changes: Vec<Change> = units
                .iter()
                .flat_map(|unit| unit.iter().map(|settled| settled.change.clone()))
                .collect();
            ledger.check(&self.member, &self.cert.member, &changes)
        };
        if check(&units.iter().collect::<Vec<_>>()).is_ok() {
            return (units.into_iter().flatten().collect(), Vec::new());
        }
        let mut accepted = Vec::new();
        let mut refused = Vec::new();
        for unit in units {
            match check(&[&unit]) {
                Ok(()) => accepted.extend(unit),
                Err(rejection) => refused.push((unit, rejection.to_string())),
            }
        }
        (accepted, refused)
    }

    /// The change a settled edit makes, with the index entry it leaves, or
    /// `None` when the file only changed its time.
    async fn prepare(
        &self,
        work: &mut Work,
        key: &PathKey,
        pending: &Pending,
    ) -> Result<Option<Settled>> {
        let entry = self.state.index_entry(key)?;
        let synced = entry.as_ref().and_then(|entry| entry.synced);
        let synced_change = self.synced_at(key)?;
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
        if content
            == synced_change
                .as_ref()
                .and_then(|(_, change)| change.content)
        {
            if entry.synced.is_some() || entry.seen.is_some() {
                self.state.update_index([(key, Some(&entry))])?;
            }
            return Ok(None);
        }
        let change = Change {
            path: pending.path.clone(),
            content,
            replaces: synced,
            continues: None,
        };
        Ok(Some(Settled {
            key: key.clone(),
            change,
            entry,
            edited: pending.stat.as_ref().map_or_else(now, stat_time),
            shape: Shape::of(synced_change, content.is_some()),
        }))
    }
}

/// Groups the settled edits into units published or suggested whole: each
/// vanished file with a file of its content that appeared, which continues
/// it, and every other edit alone.
fn pair_moves(settled: Vec<Settled>) -> Vec<Vec<Settled>> {
    let (mut vanished, others): (Vec<Settled>, Vec<Settled>) = settled
        .into_iter()
        .partition(|settled| settled.shape.vanished().is_some());
    let mut units = Vec::new();
    for mut appeared in others {
        let found = match (&appeared.shape, appeared.change.content) {
            (Shape::Appeared, Some(content)) => vanished.iter().position(|gone| {
                gone.shape
                    .vanished()
                    .is_some_and(|(_, held)| held.hash == content.hash)
            }),
            _ => None,
        };
        let Some(index) = found else {
            units.push(vec![appeared]);
            continue;
        };
        let gone = vanished.remove(index);
        appeared.change.continues = gone.shape.vanished().map(|(from, _)| from.clone());
        units.push(vec![gone, appeared]);
    }
    units.extend(vanished.into_iter().map(|gone| vec![gone]));
    units
}
