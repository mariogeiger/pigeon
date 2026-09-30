//! Which blobs garbage collection must keep: the versions retention keeps
//! of the member's files, what the disk holds, the statements, the
//! set-aside list, and the contents that requests not yet applied carry.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use iroh_blobs::Hash;
use pigeon_core::patch::Content;
use pigeon_core::retention::Dated;
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
        let mut kept: HashSet<Hash> = HashSet::new();
        let mut keep = |content: Option<Content>| {
            if let Some(content) = content {
                kept.insert(blob_hash(&content.hash));
            }
        };
        let mut requests = Vec::new();
        {
            let ledger = self.ledger.lock();
            let folder = requests_folder();
            for key in ledger.keys() {
                let versions = ledger.versions(key);
                let Some(head) = versions.last() else {
                    continue;
                };
                if head.owner == self.config.member {
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
                    for (version, kept) in versions.iter().zip(retention.keep(&dated, now)) {
                        if kept {
                            keep(version.content);
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
        self.blobs.protect(kept);
        work.tags.clear();
        work.protect_due = false;
        Ok(())
    }
}
