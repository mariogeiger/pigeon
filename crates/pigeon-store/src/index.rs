//! The disk index: for each path, what the disk held when pigeon last
//! looked and which published version that was, so that a file whose size
//! and modification time have not changed is never hashed again.

use std::io::Read;
use std::path::Path;
use std::time::SystemTime;

use pigeon_core::clock::Stamp;
use pigeon_core::patch::{Content, ContentHash};
use pigeon_core::path::GroupPath;
use serde::{Deserialize, Serialize};

use crate::disk::Stat;
use crate::error::{Result, StoreError};

/// How long after a change a file's modification time may still read the
/// same: the two seconds FAT and exFAT round it to.
const RESOLUTION: i128 = 2_000_000_000;

/// How much of a file is hashed at a time.
const CHUNK: usize = 1 << 20;

/// What pigeon last saw at one path of the disk.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Seen {
    pub stat: Stat,
    pub content: Content,
    /// Whether the file was read within [`RESOLUTION`] of its modification
    /// time, so that a write may have followed without changing its
    /// metadata: only reading it again then tells what it holds.
    pub racy: bool,
}

impl Seen {
    /// What a file with metadata `stat` held when pigeon read it as
    /// `content`, starting at `read_at`, in nanoseconds since the Unix
    /// epoch.
    #[must_use]
    pub fn read(stat: Stat, content: Content, read_at: i128) -> Self {
        Self {
            stat,
            content,
            racy: stat.modified > read_at - RESOLUTION,
        }
    }

    /// Whether a file whose metadata is now `stat` still holds this content
    /// as far as metadata tells: it [matches](Self::matches), and was read
    /// long enough after its last change.
    #[must_use]
    pub fn holds(&self, stat: &Stat) -> bool {
        !self.racy && self.matches(stat)
    }

    /// Whether `stat` shows the metadata this content was read with: the
    /// same size, time, executable bit where the system has one, and inode
    /// where both know it.
    #[must_use]
    pub fn matches(&self, stat: &Stat) -> bool {
        let known = |now: Option<(u64, u64)>, then: Option<(u64, u64)>| {
            now.zip(then).is_none_or(|(now, then)| now == then)
        };
        self.stat.size == stat.size
            && self.stat.modified == stat.modified
            && stat
                .executable
                .is_none_or(|executable| self.stat.executable == Some(executable))
            && known(stat.inode, self.stat.inode)
    }
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

/// The current time in nanoseconds since the Unix epoch.
#[must_use]
pub fn now_nanos() -> i128 {
    Stat::nanos(SystemTime::now())
}

/// The BLAKE3 hash of a file, which is also its blob hash, read a chunk at
/// a time: a file shrinking meanwhile only ends the read.
///
/// # Errors
///
/// Fails if the file cannot be read.
pub fn hash_file(path: &Path) -> Result<ContentHash> {
    let mut file = std::fs::File::open(path).map_err(StoreError::io(path))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; CHUNK];
    let mut filled = 0;
    loop {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(StoreError::io(path)(error)),
        }
        if filled == CHUNK {
            hasher.update_rayon(&buffer);
            filled = 0;
        }
    }
    hasher.update_rayon(&buffer[..filled]);
    Ok(ContentHash(*hasher.finalize().as_bytes()))
}

/// What the file at `location` holds, reusing `previous` while it
/// [holds](Seen::holds) the file, with the metadata found now. A system
/// without an executable bit keeps the previous one.
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
    if let Some(previous) = previous.filter(|seen| seen.holds(&stat)) {
        return Ok(Seen { stat, ..*previous });
    }
    let read_at = now_nanos();
    let hash = hash_file(location)?;
    let content = Content {
        hash,
        size: stat.size,
        executable: stat.executable.unwrap_or(false),
    };
    Ok(Seen::read(stat, content, read_at))
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
    fn a_file_larger_than_a_chunk_hashes_whole() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        let bytes: Vec<u8> = (0..=250).cycle().take(CHUNK * 2 + 17).collect();
        fs::write(&file, &bytes).unwrap();
        assert_eq!(
            hash_file(&file).unwrap().0,
            *blake3::hash(&bytes).as_bytes()
        );
    }

    #[test]
    fn a_file_read_right_after_its_change_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "one").unwrap();
        let first = observe(&file, Stat::read(&file).unwrap(), None).unwrap();
        assert!(first.racy);
        let modified = fs::metadata(&file).unwrap().modified().unwrap();
        fs::write(&file, "two").unwrap();
        let handle = fs::File::options().write(true).open(&file).unwrap();
        handle.set_modified(modified).unwrap();
        let again = observe(&file, Stat::read(&file).unwrap(), Some(&first)).unwrap();
        assert_eq!(again.content.hash.0, *blake3::hash(b"two").as_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn another_file_with_the_same_size_and_time_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "one").unwrap();
        set_time(&file, 100);
        let first = observe(&file, Stat::read(&file).unwrap(), None).unwrap();
        let copy = dir.path().join("copy");
        fs::write(&copy, "two").unwrap();
        set_time(&copy, 100);
        fs::rename(&copy, &file).unwrap();
        let again = observe(&file, Stat::read(&file).unwrap(), Some(&first)).unwrap();
        assert_eq!(again.content.hash.0, *blake3::hash(b"two").as_bytes());
        let unknown = Seen {
            stat: Stat {
                inode: None,
                ..again.stat
            },
            ..again
        };
        let learned = observe(&file, Stat::read(&file).unwrap(), Some(&unknown)).unwrap();
        assert_eq!(learned, again);
    }

    #[test]
    fn a_system_without_executable_bits_keeps_the_previous_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "x").unwrap();
        set_time(&file, 100);
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
