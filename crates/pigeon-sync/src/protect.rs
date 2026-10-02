//! Which blobs garbage collection must keep: the current versions of the
//! member's files and those the member made, which another member's
//! machines may not have fetched yet, what the disk is to hold whether it
//! landed yet or not, statements included, the blobs being fetched and the
//! contents that live suggestions carry; then, within the quota, the past
//! versions retention keeps of the member's files, or of every file with
//! `everything`, with what the past suggestions among them carried. A
//! member's files are those of their personal path, the files of no
//! personal path whose current version they made, and the suggestions they
//! made.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use iroh_blobs::Hash;
use pigeon_core::patch::Content;
use pigeon_core::patch::ContentHash;
use pigeon_core::path::PathKey;
use pigeon_core::retention::{Dated, within_quota};
use pigeon_core::statement::{Suggestion, is_suggestion_path};
use pigeon_store::blobs::blob_hash;

use crate::disk_sync::target;
use crate::engine::{Inner, Work};

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl Inner {
    /// Recomputes the protected set and hands it the blobs' temporary
    /// tags, which it covers.
    pub(crate) async fn protect(&self, work: &mut Work) -> Result<()> {
        let retention = work.config.retention;
        let now = seconds(SystemTime::now());
        let indexed: HashSet<PathKey> = self
            .state
            .index(None)?
            .iter()
            .map(|entry| entry.path.key())
            .collect();
        let mut anyway: HashSet<ContentHash> = HashSet::new();
        let mut keep = |content: Option<Content>| {
            if let Some(content) = content {
                anyway.insert(content.hash);
            }
        };
        let mut history: Vec<(u64, Content)> = Vec::new();
        let mut past_suggestions = Vec::new();
        {
            let ledger = self.ledger.lock();
            for key in ledger.keys() {
                let versions = ledger.versions(key);
                let Some((head, past)) = versions.split_last() else {
                    continue;
                };
                let own = match ledger.owner(&head.path) {
                    Some(owner) => owner == self.member,
                    None if is_suggestion_path(&head.path) => {
                        versions.iter().any(|version| version.author == self.member)
                    }
                    None => head.author == self.member,
                };
                if own || head.author == self.member {
                    keep(head.content);
                }
                let cutoff = work.config.selection.cutoff(&head.path);
                keep(
                    target(&ledger, key, cutoff, indexed.contains(key))
                        .and_then(|version| version.content),
                );
                if own || retention.everything {
                    let dated: Vec<Dated> = versions
                        .iter()
                        .enumerate()
                        .map(|(index, version)| Dated {
                            seconds: seconds(version.stamp.system_time()),
                            deleted_next: versions
                                .get(index + 1)
                                .is_some_and(|next| next.content.is_none()),
                        })
                        .collect();
                    let kept = retention.keep(&dated, now);
                    for ((version, dated), kept) in past.iter().zip(&dated).zip(kept) {
                        if let Some(content) = version.content.filter(|_| kept) {
                            history.push((dated.seconds, content));
                            if is_suggestion_path(&version.path) {
                                past_suggestions.push((dated.seconds, content));
                            }
                        }
                    }
                }
            }
        }
        for live in work.suggestions.values() {
            for change in &live.suggestion.changes {
                keep(change.content);
            }
        }
        for kept in self.state.kept()?.into_values() {
            keep(kept.content);
        }
        history.extend(self.suggested(past_suggestions).await?);
        anyway.extend(work.fetching.keys().copied());
        let mut held = Vec::new();
        for (time, content) in history {
            if !anyway.contains(&content.hash) && self.blobs.has(&content.hash).await? {
                held.push((time, content));
            }
        }
        let quota = self.blobs.disk_size()? / 100 * u64::from(retention.quota_percent);
        let kept: HashSet<Hash> = within_quota(&held, quota, &anyway)
            .iter()
            .chain(&anyway)
            .map(blob_hash)
            .collect();
        self.blobs.protect(kept, std::mem::take(&mut work.tags));
        work.protect_due = false;
        Ok(())
    }

    /// The contents the suggestion statements `statements` carried, each
    /// dated as its statement, as far as this machine holds them.
    async fn suggested(&self, statements: Vec<(u64, Content)>) -> Result<Vec<(u64, Content)>> {
        let mut suggested = Vec::new();
        for (seconds, content) in statements {
            if !self.blobs.has(&content.hash).await? {
                continue;
            }
            let Ok(suggestion) = self.read_statement::<Suggestion>(&content).await else {
                continue;
            };
            suggested.extend(
                suggestion
                    .changes
                    .iter()
                    .filter_map(|change| change.content.map(|content| (seconds, content))),
            );
        }
        Ok(suggested)
    }
}
