//! Changing the selection: one rule at a time, or every rule at once after
//! a preview of what the new rules would download, free and freeze on this
//! machine. Held copies the change stops holding go, unless modified; those
//! no rule held, such as files created here, stay.

use std::collections::HashSet;

use anyhow::{Result, bail};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::selection::{Cutoff, Rule, Selection};
use pigeon_core::statement::is_statement;
use pigeon_store::config::Config;
use pigeon_store::disk::{self, fs_path};
use pigeon_store::index::IndexEntry;
use serde::Serialize;

use crate::disk_sync::{file_stat, target};
use crate::engine::{Engine, Inner, Work};
use crate::watch::Rescan;

/// How many files of each delta a preview lists, the largest first.
pub const LISTED: usize = 100;

/// How many files, and how many bytes they fill.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize)]
pub struct Amount {
    pub files: u64,
    pub bytes: u64,
}

impl Amount {
    fn add(&mut self, size: u64) {
        self.files += 1;
        self.bytes += size;
    }
}

/// What one draft rule does among the group's files.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize)]
pub struct RuleEffect {
    /// The files it matches.
    pub matches: u64,
    /// The files it decides, being their last matching rule.
    pub decides: Amount,
}

/// How a file's copy on this machine changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Delta {
    /// A version comes here.
    Download,
    /// The copy goes.
    Free,
    /// The copy stays, frozen at a time.
    Freeze,
}

/// One file whose copy changes, with the draft rule that decides it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Changed {
    pub path: GroupPath,
    pub size: u64,
    pub rule: Option<usize>,
}

/// Every file of one delta counted, the largest listed.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct DeltaFiles {
    pub delta: Delta,
    pub total: Amount,
    /// The `LISTED` largest, largest first.
    pub largest: Vec<Changed>,
}

/// Files of this member's own folder that a draft rule frees.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub struct OwnFreed {
    /// The deciding rule, none when no rule matches.
    pub rule: Option<usize>,
    pub total: Amount,
}

/// What replacing the selection with draft rules would change here.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Preview {
    /// The version of the current selection, which `set_selection` expects.
    pub version: String,
    /// What the current selection holds.
    pub now: Amount,
    /// What the draft would hold.
    pub after: Amount,
    /// One effect per draft rule.
    pub rules: Vec<RuleEffect>,
    /// Downloads, frees and freezes, in that order.
    pub deltas: Vec<DeltaFiles>,
    pub own_freed: Vec<OwnFreed>,
}

/// The delta between the content held now and after, if any.
fn delta(now: Option<u64>, after: Option<(u64, bool)>, same: bool) -> Option<(Delta, u64)> {
    match (now, after) {
        (None, Some((size, _))) => Some((Delta::Download, size)),
        (Some(size), None) => Some((Delta::Free, size)),
        (Some(_), Some((size, true))) => Some((Delta::Freeze, size)),
        (Some(_), Some((size, false))) if !same => Some((Delta::Download, size)),
        _ => None,
    }
}

impl Inner {
    /// The keys whose held copies a cutoff of $-\infty$ keeps: those
    /// modified on disk, or whose edits wait.
    fn modified<'a>(
        &self,
        work: &Work,
        entries: impl Iterator<Item = &'a IndexEntry>,
    ) -> HashSet<PathKey> {
        entries
            .filter(|entry| {
                let stat = file_stat(&fs_path(&self.root, &entry.path));
                entry.seen.map(|seen| seen.stat) != stat
                    || work.pending.contains_key(&entry.path.key())
            })
            .map(|entry| entry.path.key())
            .collect()
    }

    /// Removes the held copies nobody modified that the change from
    /// `before` to the current selection stops holding: those whose cutoff
    /// becomes $-\infty$. Then records the current selection as the one the
    /// disk was brought to.
    pub(crate) fn free_unselected(&self, work: &Work, before: &Selection) -> Result<()> {
        let after = &work.config.selection;
        let mut entries = self.state.index(None)?;
        entries.retain(|entry| {
            after.cutoff(&entry.path) == Cutoff::MinusInfinity
                && before.cutoff(&entry.path) != Cutoff::MinusInfinity
        });
        let modified = self.modified(work, entries.iter());
        for entry in entries {
            let key = entry.path.key();
            if modified.contains(&key) {
                continue;
            }
            let location = fs_path(&self.root, &entry.path);
            if file_stat(&location).is_some() {
                disk::remove(&self.root, &location)?;
            }
            self.state.update_index([(&key, None)])?;
        }
        let rules: Vec<Rule> = after.rules().cloned().collect();
        self.state.set_applied_selection(&rules)?;
        Ok(())
    }

    /// The selection the disk was last brought to, or the current one if
    /// none was recorded.
    pub(crate) fn applied_selection(&self, work: &Work) -> Result<Selection> {
        Ok(match self.state.applied_selection()? {
            Some(rules) => Selection::exactly(rules)?,
            None => work.config.selection.clone(),
        })
    }
}

/// Sorts `files` largest first and keeps the `LISTED` largest.
fn largest(mut files: Vec<Changed>) -> Vec<Changed> {
    files.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    files.truncate(LISTED);
    files
}

impl Engine {
    /// Makes `rule` the last rule of the selection. A rule that stops
    /// holding files also removes the held copies nobody modified.
    ///
    /// # Errors
    ///
    /// Fails if the pattern is invalid or the configuration or the state
    /// cannot be written.
    pub async fn set_rule(&self, rule: Rule) -> Result<()> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        let before = work.config.selection.clone();
        let mut config = Config::clone(&work.config);
        config.selection.set(rule)?;
        work.config.save(config)?;
        inner.free_unselected(&work, &before)?;
        inner.refresh(&mut work, &Rescan::All).await;
        Ok(())
    }

    /// Replaces the whole selection with `rules`, kept as given, if it is
    /// still at `version` when one is given. The held copies nobody
    /// modified that the new rules stop holding go.
    ///
    /// # Errors
    ///
    /// Fails if the selection changed since `version`, a pattern is
    /// invalid, or the configuration or the state cannot be written.
    pub async fn set_selection(&self, rules: Vec<Rule>, version: Option<&str>) -> Result<()> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        let current = work.config.selection.version();
        if let Some(version) = version.filter(|version| *version != current) {
            bail!("the selection changed since version {version}: it is now at {current}");
        }
        let before = work.config.selection.clone();
        let mut config = Config::clone(&work.config);
        config.selection = Selection::exactly(rules)?;
        work.config.save(config)?;
        inner.free_unselected(&work, &before)?;
        inner.refresh(&mut work, &Rescan::All).await;
        Ok(())
    }

    /// What replacing the selection with `rules` would change here, counted
    /// in one pass over the ledger.
    ///
    /// # Errors
    ///
    /// Fails if a pattern is invalid or the index cannot be read.
    pub async fn preview(&self, rules: Vec<Rule>) -> Result<Preview> {
        let inner = &self.inner;
        let draft = Selection::exactly(rules)?;
        let entries = inner.state.index(None)?;
        let work = inner.work.lock().await;
        let freed = entries
            .iter()
            .filter(|entry| draft.cutoff(&entry.path) == Cutoff::MinusInfinity);
        let modified = inner.modified(&work, freed);
        let indexed: HashSet<PathKey> = entries.iter().map(|entry| entry.path.key()).collect();
        let own = inner.member.own_folder();
        let mut tally = Tally::new(work.config.selection.version(), draft.rules().count());
        let ledger = inner.ledger.lock();
        for key in ledger.keys() {
            let Some(head) = ledger.head(key) else {
                continue;
            };
            let path = &head.path;
            if is_statement(&path.key()) {
                continue;
            }
            let cutoff = draft.cutoff(path);
            let rule = draft.decider(path);
            if let Some(content) = head.content {
                tally.rule_effects(draft.matching(path), rule, content.size);
            }
            let current = work.config.selection.cutoff(path);
            let now = target(&ledger, key, current, indexed.contains(key))
                .and_then(|version| version.content);
            let kept = modified.contains(key)
                || (indexed.contains(key) && current == Cutoff::MinusInfinity);
            let after = target(&ledger, key, cutoff, kept).and_then(|version| version.content);
            let freezes = matches!(cutoff, Cutoff::At(_)) && !matches!(current, Cutoff::At(_));
            let same = now.map(|content| content.hash) == after.map(|content| content.hash);
            let now = now.map(|content| content.size);
            let after = after.map(|content| (content.size, freezes));
            tally.totals(now, after.map(|(size, _)| size));
            if let Some((delta, size)) = delta(now, after, same) {
                let file = Changed {
                    path: path.clone(),
                    size,
                    rule,
                };
                tally.change(delta, file, path.is_within(&own));
            }
        }
        Ok(tally.preview())
    }
}

/// A preview being counted, file by file.
struct Tally {
    preview: Preview,
    changed: [Vec<Changed>; 3],
    totals: [Amount; 3],
}

impl Tally {
    fn new(version: String, rules: usize) -> Self {
        Self {
            preview: Preview {
                version,
                now: Amount::default(),
                after: Amount::default(),
                rules: vec![RuleEffect::default(); rules],
                deltas: Vec::new(),
                own_freed: Vec::new(),
            },
            changed: Default::default(),
            totals: [Amount::default(); 3],
        }
    }

    /// Counts a live file of `size` that the rules at `matching` match and
    /// the rule `decider` decides.
    fn rule_effects(
        &mut self,
        matching: impl Iterator<Item = usize>,
        decider: Option<usize>,
        size: u64,
    ) {
        for index in matching {
            self.preview.rules[index].matches += 1;
        }
        if let Some(index) = decider {
            self.preview.rules[index].decides.add(size);
        }
    }

    /// Counts the sizes of a file held now and after, if held.
    fn totals(&mut self, now: Option<u64>, after: Option<u64>) {
        if let Some(size) = now {
            self.preview.now.add(size);
        }
        if let Some(size) = after {
            self.preview.after.add(size);
        }
    }

    /// Counts a file that `delta` changes, one of this member's own when
    /// `own`.
    fn change(&mut self, delta: Delta, file: Changed, own: bool) {
        let slot = delta as usize;
        self.totals[slot].add(file.size);
        if delta == Delta::Free && own {
            let freed = &mut self.preview.own_freed;
            if let Some(known) = freed.iter_mut().find(|known| known.rule == file.rule) {
                known.total.add(file.size);
            } else {
                let mut total = Amount::default();
                total.add(file.size);
                freed.push(OwnFreed {
                    rule: file.rule,
                    total,
                });
            }
        }
        self.changed[slot].push(file);
    }

    fn preview(self) -> Preview {
        let mut preview = self.preview;
        preview.own_freed.sort_by_key(|freed| freed.rule);
        preview.deltas = [Delta::Download, Delta::Free, Delta::Freeze]
            .into_iter()
            .zip(self.totals)
            .zip(self.changed)
            .map(|((delta, total), files)| DeltaFiles {
                delta,
                total,
                largest: largest(files),
            })
            .collect();
        preview
    }
}
