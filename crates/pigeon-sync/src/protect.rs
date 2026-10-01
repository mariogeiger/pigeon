//! Which blobs garbage collection must keep: the current versions of the
//! member's files, what the disk holds, the statements, the set-aside list,
//! the blobs being fetched and the contents that requests not yet applied
//! carry; then, within the quota, the past versions retention keeps of the
//! member's files, or of every file with `everything`.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use iroh_blobs::Hash;
use pigeon_core::patch::Content;
use pigeon_core::patch::ContentHash;
use pigeon_core::retention::{Dated, within_quota};
use pigeon_core::statement::{RequestStatement, STATEMENTS};
use pigeon_store::blobs::blob_hash;

use crate::disk_sync::change_at;
use crate::engine::{Inner, Work};
use crate::statements::requests_folder;

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl Inner {
    /// Recomputes the protected set and lets go of the blobs' temporary
    /// tags, which it covers.
    pub(crate) async fn protect(&self, work: &mut Work) -> Result<()> {
        let retention = self.state.retention()?;
        let now = seconds(SystemTime::now());
        let entries = self.state.index(None)?;
        let mut anyway: HashSet<ContentHash> = HashSet::new();
        let mut keep = |content: Option<Content>| {
            if let Some(content) = content {
                anyway.insert(content.hash);
            }
        };
        let mut history: Vec<(u64, Content)> = Vec::new();
        let mut requests = Vec::new();
        {
            let ledger = self.ledger.lock();
            let folder = requests_folder();
            for key in ledger.keys() {
                let versions = ledger.versions(key);
                let Some((head, past)) = versions.split_last() else {
                    continue;
                };
                let own = head.owner == self.config.member;
                if own {
                    keep(head.content);
                }
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
                        }
                    }
                }
                if head.path.is_inside(STATEMENTS) {
                    keep(head.content);
                    if head.path.is_inside(&folder)
                        && let Some(content) = head.content
                    {
                        requests.push((head.path.clone(), content));
                    }
                }
            }
            for entry in &entries {
                if let Some(stamp) = entry.synced {
                    keep(
                        change_at(&ledger, &stamp, &entry.path.key())
                            .and_then(|change| change.content),
                    );
                }
            }
        }
        let applied = self.applied_requests();
        for (path, content) in requests {
            if applied.contains(&path) {
                continue;
            }
            let Ok(statement) = self.read_statement::<RequestStatement>(&content).await else {
                continue;
            };
            for change in statement.changes {
                keep(change.content);
            }
        }
        for (_, item) in self.state.aside()? {
            keep(item.content);
        }
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
        self.blobs.protect(kept);
        work.tags.clear();
        work.protect_due = false;
        Ok(())
    }
}
