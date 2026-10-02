//! The disk index as pigeon 0.8 kept it, before it knew a file's inode and
//! whether it read the file within its time's resolution: read once, to
//! move it to the current form, which trusts what 0.8 saw as 0.8 did.

use pigeon_core::clock::Stamp;
use pigeon_core::patch::Content;
use pigeon_core::path::GroupPath;
use serde::Deserialize;

use crate::disk::Stat;
use crate::index::{IndexEntry, Seen};

#[derive(Deserialize)]
struct StatV1 {
    size: u64,
    modified: i128,
    executable: Option<bool>,
}

#[derive(Deserialize)]
struct SeenV1 {
    stat: StatV1,
    content: Content,
}

/// An index entry as 0.8 wrote it.
#[derive(Deserialize)]
pub(crate) struct IndexEntryV1 {
    path: GroupPath,
    seen: Option<SeenV1>,
    synced: Option<Stamp>,
}

impl From<IndexEntryV1> for IndexEntry {
    fn from(entry: IndexEntryV1) -> Self {
        Self {
            path: entry.path,
            seen: entry.seen.map(|seen| Seen {
                stat: Stat {
                    size: seen.stat.size,
                    modified: seen.stat.modified,
                    executable: seen.stat.executable,
                    inode: None,
                },
                content: seen.content,
                racy: false,
            }),
            synced: entry.synced,
        }
    }
}
