//! Files in a group's root: where a group path lives on disk, what a file's
//! metadata says, and how pigeon replaces or removes a file at once,
//! keeping the permissions it finds and setting only whether the file may
//! be executed.

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

/// `mode` with execution allowed wherever reading is when `executable`,
/// and nowhere otherwise.
#[cfg(unix)]
fn with_execution(mode: u32, executable: bool) -> u32 {
    if executable {
        mode | (mode & 0o444) >> 2
    } else {
        mode & !0o111
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

/// Moves `temporary` onto `target` at once. The file takes the
/// permissions of the file it replaces, or else those it was written with,
/// which the umask decided, and may be executed as `executable` says.
///
/// # Errors
///
/// Fails if the file cannot be moved or its permissions set.
pub fn install(temporary: &Path, target: &Path, executable: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let written = fs::symlink_metadata(temporary)
            .map_err(StoreError::io(temporary))?
            .permissions()
            .mode()
            & 0o777;
        let found = fs::symlink_metadata(target)
            .ok()
            .filter(fs::Metadata::is_file)
            .map_or(written, |metadata| metadata.permissions().mode() & 0o777);
        let mode = with_execution(found, executable);
        if mode != written {
            fs::set_permissions(temporary, fs::Permissions::from_mode(mode))
                .map_err(StoreError::io(temporary))?;
        }
    }
    #[cfg(not(unix))]
    let _ = executable;
    fs::rename(temporary, target).map_err(StoreError::io(target))
}

/// Removes the file at `target`, then every folder it leaves empty up to,
/// but not including, `root` or a link to a placed folder.
///
/// # Errors
///
/// Fails if the file exists and cannot be removed.
pub fn remove(root: &Path, target: &Path) -> Result<()> {
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

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn written(target: &Path, mode: u32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let temporary = temporary_path(target);
        fs::write(&temporary, "new").unwrap();
        fs::set_permissions(&temporary, fs::Permissions::from_mode(mode)).unwrap();
        temporary
    }

    #[cfg(unix)]
    #[test]
    fn a_new_file_keeps_the_mode_the_umask_gave_it_and_executes_where_it_reads() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("run.sh");
        let temporary = written(&target, 0o640);
        assert_eq!(temporary.file_name().unwrap(), ".~pigeon-run.sh");
        install(&temporary, &target, true).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(mode(&target), 0o750);
        assert!(!temporary.exists());
        let plain = dir.path().join("notes.txt");
        install(&written(&plain, 0o600), &plain, false).unwrap();
        assert_eq!(mode(&plain), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_file_keeps_its_mode_but_for_execution() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a");
        fs::write(&target, "old").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o751)).unwrap();
        install(&written(&target, 0o644), &target, false).unwrap();
        assert_eq!(mode(&target), 0o640);
        fs::set_permissions(&target, fs::Permissions::from_mode(0o444)).unwrap();
        install(&written(&target, 0o644), &target, true).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(mode(&target), 0o555);
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
