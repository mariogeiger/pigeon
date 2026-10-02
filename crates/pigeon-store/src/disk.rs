//! Files in a group's root: where a group path lives on disk, what a file's
//! metadata says, and how pigeon replaces or removes a file at once, with
//! the executable bit it should carry; every file pigeon writes may be
//! written.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use pigeon_core::path::GroupPath;

use crate::error::{Result, StoreError};

/// The prefix of files pigeon writes before moving them into place; scans
/// skip them.
pub const TEMPORARY_PREFIX: &str = ".~pigeon-";

/// Where a group path lives under a root.
#[must_use]
pub fn fs_path(root: &Path, path: &GroupPath) -> PathBuf {
    let mut result = root.to_path_buf();
    result.extend(path.names());
    result
}

/// What a scan records of a file, enough to tell that it changed without
/// reading it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Stat {
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    pub modified: i128,
    /// The executable bit, where the system has one.
    pub executable: Option<bool>,
}

impl Stat {
    #[must_use]
    pub fn of(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            Some(metadata.permissions().mode() & 0o111 != 0)
        };
        #[cfg(not(unix))]
        let executable = None;
        let modified =
            metadata
                .modified()
                .map_or(0, |time| match time.duration_since(UNIX_EPOCH) {
                    Ok(after) => i128::try_from(after.as_nanos()).unwrap_or(i128::MAX),
                    Err(before) => {
                        -i128::try_from(before.duration().as_nanos()).unwrap_or(i128::MAX)
                    }
                });
        Self {
            size: metadata.len(),
            modified,
            executable,
        }
    }

    /// Reads the metadata of the file at `path`, without following a link.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read.
    pub fn read(path: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(path).map_err(StoreError::io(path))?;
        Ok(Self::of(&metadata))
    }
}

/// Lets a file be written, and executed when `executable`.
///
/// # Errors
///
/// Fails if the permissions cannot be changed.
pub fn set_permissions(path: &Path, executable: bool) -> Result<()> {
    let mut permissions = fs::metadata(path)
        .map_err(StoreError::io(path))?
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let execute = if executable { 0o111 } else { 0 };
        permissions.set_mode(0o644 | execute);
    }
    #[cfg(not(unix))]
    {
        let _ = executable;
        permissions.set_readonly(false);
    }
    fs::set_permissions(path, permissions).map_err(StoreError::io(path))
}

/// Lets the file at `path` be written if it is read-only, as pigeon left
/// published drop files until 0.6, keeping its executable bit.
///
/// # Errors
///
/// Fails if the file exists and its permissions cannot be changed.
pub fn unfreeze(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.permissions().readonly() => {
            set_permissions(path, Stat::of(&metadata).executable.unwrap_or(false))
        }
        _ => Ok(()),
    }
}

/// The temporary file next to `target` into which its new content goes.
///
/// # Panics
///
/// Panics if `target` has no file name.
#[must_use]
pub fn temporary_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .expect("a file has a name")
        .to_string_lossy();
    target.with_file_name(format!("{TEMPORARY_PREFIX}{name}"))
}

/// Moves `temporary` onto `target` at once, after giving it its bits.
///
/// # Errors
///
/// Fails if the file cannot be moved or its permissions set.
pub fn install(temporary: &Path, target: &Path, executable: bool) -> Result<()> {
    set_permissions(temporary, executable)?;
    if cfg!(windows) {
        unfreeze(target)?;
    }
    fs::rename(temporary, target).map_err(StoreError::io(target))
}

/// Removes the file at `target`, then every folder it leaves empty up to,
/// but not including, `root` or a link to a placed folder.
///
/// # Errors
///
/// Fails if the file exists and cannot be removed.
pub fn remove(root: &Path, target: &Path) -> Result<()> {
    if cfg!(windows) {
        unfreeze(target)?;
    }
    match fs::remove_file(target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(StoreError::io(target)(error)),
    }
    let mut folder = target.parent();
    while let Some(current) = folder {
        if current == root
            || !current.starts_with(root)
            || fs::symlink_metadata(current).is_ok_and(|metadata| metadata.is_symlink())
            || fs::remove_dir(current).is_err()
        {
            break;
        }
        folder = current.parent();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_replaces_a_read_only_file_with_a_writable_one_and_sets_its_bits() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.txt");
        fs::write(&target, "old").unwrap();
        let mut frozen = fs::metadata(&target).unwrap().permissions();
        frozen.set_readonly(true);
        fs::set_permissions(&target, frozen).unwrap();
        let temporary = temporary_path(&target);
        assert_eq!(temporary.file_name().unwrap(), ".~pigeon-a.txt");
        fs::write(&temporary, "new").unwrap();
        install(&temporary, &target, true).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        let metadata = fs::metadata(&target).unwrap();
        assert!(!metadata.permissions().readonly());
        if cfg!(unix) {
            assert_eq!(Stat::of(&metadata).executable, Some(true));
        }
        assert!(!temporary.exists());
    }

    #[test]
    fn remove_prunes_empty_folders_but_keeps_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("g");
        let path = GroupPath::parse("a/b/c.txt").unwrap();
        let target = fs_path(&root, &path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(root.join("a/keep.txt"), "").unwrap();
        fs::write(&target, "").unwrap();
        remove(&root, &target).unwrap();
        assert!(!root.join("a/b").exists());
        assert!(root.join("a/keep.txt").exists());
        fs::remove_file(root.join("a/keep.txt")).unwrap();
        remove(&root, &root.join("a/keep.txt")).unwrap();
        assert!(root.exists());
    }

    #[test]
    fn stat_notices_size_and_time() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "abc").unwrap();
        let before = Stat::read(&file).unwrap();
        assert_eq!(before.size, 3);
        let file_handle = fs::File::options().write(true).open(&file).unwrap();
        file_handle
            .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1))
            .unwrap();
        let after = Stat::read(&file).unwrap();
        assert_eq!(after.modified, 1_000_000_000);
        assert_ne!(before, after);
    }
}
