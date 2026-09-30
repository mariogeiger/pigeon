//! The disk index: for each path, what the disk held when pigeon last
//! looked and which published version that was, so that a file whose size
//! and modification time have not changed is never hashed again.

use std::path::Path;

use pigeon_core::clock::Stamp;
use pigeon_core::patch::{Content, ContentHash};
use pigeon_core::path::GroupPath;
use serde::{Deserialize, Serialize};

use crate::disk::Stat;
use crate::error::{Result, StoreError};

/// What pigeon last saw at one path of the disk.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Seen {
    pub stat: Stat,
    pub content: Content,
}

/// One path of the index: the disk state last seen there, and the version
/// of the ledger that the disk last agreed with.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct IndexEntry {
    pub path: GroupPath,
    /// The file on disk, or `None` once pigeon saw it missing.
    pub seen: Option<Seen>,
    /// The version the disk last matched, or `None` for a file never
    /// published.
    pub synced: Option<Stamp>,
}

/// The BLAKE3 hash of a file, which is also its blob hash.
///
/// # Errors
///
/// Fails if the file cannot be read.
pub fn hash_file(path: &Path) -> Result<ContentHash> {
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_mmap_rayon(path)
        .map_err(StoreError::io(path))?;
    Ok(ContentHash(*hasher.finalize().as_bytes()))
}

/// What the file at `location` holds, reusing `previous` unless its size,
/// time, or executable bit changed. A system without an executable bit
/// keeps the previous one.
///
/// # Errors
///
/// Fails if the file must be hashed and cannot be read.
pub fn observe(location: &Path, stat: Stat, previous: Option<&Seen>) -> Result<Seen> {
    let stat = Stat {
        executable: stat
            .executable
            .or(previous.and_then(|seen| seen.stat.executable)),
        ..stat
    };
    if let Some(previous) = previous.filter(|seen| seen.stat == stat) {
        return Ok(*previous);
    }
    let hash = hash_file(location)?;
    Ok(Seen {
        stat,
        content: Content {
            hash,
            size: stat.size,
            executable: stat.executable.unwrap_or(false),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    fn set_time(path: &Path, seconds: u64) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn hash_is_blake3_of_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "hello").unwrap();
        assert_eq!(
            hash_file(&file).unwrap().0,
            *blake3::hash(b"hello").as_bytes()
        );
    }

    #[test]
    fn an_unchanged_stat_is_never_hashed_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "one").unwrap();
        set_time(&file, 100);
        let first = observe(&file, Stat::read(&file).unwrap(), None).unwrap();
        fs::write(&file, "two").unwrap();
        set_time(&file, 100);
        let same = observe(&file, Stat::read(&file).unwrap(), Some(&first)).unwrap();
        assert_eq!(same, first);
        set_time(&file, 101);
        let changed = observe(&file, Stat::read(&file).unwrap(), Some(&first)).unwrap();
        assert_eq!(changed.content.hash.0, *blake3::hash(b"two").as_bytes());
    }

    #[test]
    fn a_system_without_executable_bits_keeps_the_previous_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "x").unwrap();
        let stat = Stat {
            executable: Some(true),
            ..Stat::read(&file).unwrap()
        };
        let first = observe(&file, stat, None).unwrap();
        assert!(first.content.executable);
        let unknown = Stat {
            executable: None,
            ..stat
        };
        assert_eq!(observe(&file, unknown, Some(&first)).unwrap(), first);
    }
}
