//! Files in a group's root: where a group path lives on disk, what a file's
//! metadata says, and how pigeon replaces, moves or removes a file at once
//! while the disk shows what pigeon saw there, the new file flushed to the
//! disk before it takes the place of the old, keeping the permissions it
//! finds and setting only whether the file may be executed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// The device and inode numbers, where the system gives them: a file
    /// copied over another, with its size and time, is another file.
    pub inode: Option<(u64, u64)>,
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
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            Some((metadata.dev(), metadata.ino()))
        };
        #[cfg(not(unix))]
        let inode = None;
        let modified = metadata.modified().map_or(0, Self::nanos);
        Self {
            size: metadata.len(),
            modified,
            executable,
            inode,
        }
    }

    /// `time` in nanoseconds since the Unix epoch, negative before it.
    #[must_use]
    pub fn nanos(time: std::time::SystemTime) -> i128 {
        match time.duration_since(UNIX_EPOCH) {
            Ok(after) => i128::try_from(after.as_nanos()).unwrap_or(i128::MAX),
            Err(before) => -i128::try_from(before.duration().as_nanos()).unwrap_or(i128::MAX),
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

/// A new temporary file next to `target`, into which its new content goes:
/// no other write of this process uses it, and its name is short whatever
/// the target's.
///
/// # Panics
///
/// Panics if `target` has no file name.
#[must_use]
pub fn temporary_path(target: &Path) -> PathBuf {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    assert!(target.file_name().is_some(), "a file has a name");
    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!("{TEMPORARY_PREFIX}{}-{write}", std::process::id()))
}

/// Flushes the names `folder` holds to the disk, so that a file moved into
/// it stays there after a power loss. Windows journals its moves and opens
/// no folder as a file.
///
/// # Errors
///
/// Fails if the folder cannot be flushed.
pub fn sync_folder(folder: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    fs::File::open(folder)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = folder;
    Ok(())
}

/// Flushes the content of the written file at `path` to the disk. Unix
/// flushes a file opened only to read it, and so a read-only one.
fn flush(path: &Path) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(not(unix))]
    options.write(true);
    options
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(StoreError::io(path))
}

/// Moves the flushed file `temporary` onto `target`, then flushes the
/// folder.
fn move_into_place(temporary: &Path, target: &Path) -> Result<()> {
    fs::rename(temporary, target).map_err(StoreError::io(target))?;
    target.parent().map_or(Ok(()), |folder| {
        sync_folder(folder).map_err(StoreError::io(folder))
    })
}

/// `result`, removing the file `temporary` unless it moved into place.
fn discarding<T>(temporary: &Path, result: Result<Option<T>>) -> Result<Option<T>> {
    if !matches!(result, Ok(Some(_))) {
        let _ = fs::remove_file(temporary);
    }
    result
}

/// Moves the written file `temporary` onto `target` at once, after
/// flushing its content, then flushes the folder, so that a crash leaves
/// the old file or the new one whole. The temporary file goes on failure.
///
/// # Errors
///
/// Fails if the file cannot be flushed or moved.
pub fn replace(temporary: &Path, target: &Path) -> Result<()> {
    let moved = flush(temporary).and_then(|()| move_into_place(temporary, target));
    discarding(temporary, moved.map(Some)).map(|_| ())
}

/// Whether the disk at `target` still shows what pigeon saw there: the
/// file of metadata `expected`, or no file when `None`.
fn shows(target: &Path, expected: Option<Stat>) -> Result<bool> {
    match fs::symlink_metadata(target) {
        Ok(metadata) => Ok(expected == Some(Stat::of(&metadata))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(expected.is_none()),
        Err(error) => Err(StoreError::io(target)(error)),
    }
}

/// Moves `temporary` onto `target` at once, as [`replace`] does, if the
/// disk at `target` still shows `expected`: the file pigeon saw there, or
/// no file. The file takes the permissions of the file it replaces, or
/// else those it was written with, which the umask decided, and may be
/// executed as `executable` says. Returns the metadata the file has, read
/// before it moved into place, so that a write that follows shows; or
/// `None`, leaving the disk as it is, when `target` changed since pigeon
/// saw it. The temporary file goes unless it moved.
///
/// # Errors
///
/// Fails if the file cannot be flushed, moved or its permissions set.
pub fn install(
    temporary: &Path,
    target: &Path,
    executable: bool,
    expected: Option<Stat>,
) -> Result<Option<Stat>> {
    discarding(
        temporary,
        install_kept(temporary, target, executable, expected),
    )
}

/// What [`install`] does, but for removing the temporary file.
fn install_kept(
    temporary: &Path,
    target: &Path,
    executable: bool,
    expected: Option<Stat>,
) -> Result<Option<Stat>> {
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
    flush(temporary)?;
    let stat = Stat::read(temporary)?;
    if !shows(target, expected)? {
        return Ok(None);
    }
    move_into_place(temporary, target)?;
    Ok(Some(stat))
}

/// Removes the file at `target` if it still shows `expected`, the
/// metadata pigeon saw, then every folder it leaves empty, as [`prune`]
/// does. Says whether no file is left there: `false` when the file
/// changed since, which it keeps.
///
/// # Errors
///
/// Fails if the file cannot be looked at or removed.
pub fn remove(root: &Path, target: &Path, expected: Stat) -> Result<bool> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if Stat::of(&metadata) != expected => return Ok(false),
        Ok(_) => match fs::remove_file(target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(StoreError::io(target)(error)),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(StoreError::io(target)(error)),
    }
    prune(root, target);
    Ok(true)
}

/// Moves the file at `from` to `to`, creating the folders it needs,
/// unless another file is at `to`: the file itself is there, under
/// another spelling of its name, on a disk that tells no case apart. Then
/// flushes both folders and prunes those `from` leaves empty, as
/// [`prune`] does. Says whether it moved.
///
/// # Errors
///
/// Fails if a folder cannot be made or the file cannot be moved.
pub fn rename(root: &Path, from: &Path, to: &Path) -> Result<bool> {
    match fs::symlink_metadata(to) {
        Ok(_) if !same_file(from, to) => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(StoreError::io(to)(error)),
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(StoreError::io(parent))?;
    }
    fs::rename(from, to).map_err(StoreError::io(to))?;
    for folder in [to.parent(), from.parent()].into_iter().flatten() {
        sync_folder(folder).map_err(StoreError::io(folder))?;
    }
    prune(root, from);
    Ok(true)
}

/// Whether `a` and `b` name one file: by its device and inode where the
/// system tells them, and otherwise by its canonical path.
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        let inode = |path: &Path| Stat::read(path).ok().and_then(|stat| stat.inode);
        inode(a).is_some_and(|known| inode(b) == Some(known))
    }
    #[cfg(not(unix))]
    {
        fs::canonicalize(a).is_ok_and(|canonical| fs::canonicalize(b).ok() == Some(canonical))
    }
}

/// Removes every folder that the removal or move of the file at `target`
/// left empty, up to, but not including, `root` or a link to a placed
/// folder.
fn prune(root: &Path, target: &Path) {
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
}

/// Whether the temporary file at `path` is one another process wrote,
/// which this one never moves into place: one left by a run that stopped
/// before it moved it.
#[must_use]
pub fn is_left_over(path: &Path) -> bool {
    let own = format!("{TEMPORARY_PREFIX}{}-", std::process::id());
    path.file_name()
        .and_then(|name| name.to_str())
        .is_none_or(|name| !name.starts_with(&own))
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
        let name = temporary.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(TEMPORARY_PREFIX));
        assert_ne!(temporary, temporary_path(&target));
        let installed = install(&temporary, &target, true, None).unwrap();
        assert_eq!(installed, Some(Stat::read(&target).unwrap()));
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(mode(&target), 0o750);
        assert!(!temporary.exists());
        let plain = dir.path().join("notes.txt");
        install(&written(&plain, 0o600), &plain, false, None).unwrap();
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
        let seen = Some(Stat::read(&target).unwrap());
        install(&written(&target, 0o644), &target, false, seen).unwrap();
        assert_eq!(mode(&target), 0o640);
        fs::set_permissions(&target, fs::Permissions::from_mode(0o444)).unwrap();
        let seen = Some(Stat::read(&target).unwrap());
        install(&written(&target, 0o644), &target, true, seen).unwrap();
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
        assert!(remove(&root, &target, Stat::read(&target).unwrap()).unwrap());
        assert!(!root.join("a/b").exists());
        assert!(root.join("a/keep.txt").exists());
        let kept = Stat::read(&root.join("a/keep.txt")).unwrap();
        fs::remove_file(root.join("a/keep.txt")).unwrap();
        assert!(remove(&root, &root.join("a/keep.txt"), kept).unwrap());
        assert!(root.exists());
    }

    #[test]
    fn a_file_changed_since_pigeon_saw_it_is_neither_replaced_nor_removed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a");
        fs::write(&target, "old").unwrap();
        let seen = Stat::read(&target).unwrap();
        fs::write(&target, "the user's").unwrap();
        let temporary = temporary_path(&target);
        fs::write(&temporary, "new").unwrap();
        assert_eq!(
            install(&temporary, &target, false, Some(seen)).unwrap(),
            None
        );
        assert!(!temporary.exists());
        assert!(!remove(dir.path(), &target, seen).unwrap());
        assert_eq!(fs::read_to_string(&target).unwrap(), "the user's");
        let appeared = dir.path().join("b");
        fs::write(&appeared, "the user's").unwrap();
        fs::write(&temporary, "new").unwrap();
        assert_eq!(install(&temporary, &appeared, false, None).unwrap(), None);
        assert_eq!(fs::read_to_string(&appeared).unwrap(), "the user's");
    }

    #[test]
    fn a_rename_takes_no_other_file_s_place_and_prunes_what_it_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("old")).unwrap();
        fs::write(root.join("old/a"), "a").unwrap();
        fs::write(root.join("b"), "b").unwrap();
        assert!(!rename(root, &root.join("old/a"), &root.join("b")).unwrap());
        assert_eq!(fs::read_to_string(root.join("b")).unwrap(), "b");
        assert!(rename(root, &root.join("old/a"), &root.join("new/A")).unwrap());
        assert_eq!(fs::read_to_string(root.join("new/A")).unwrap(), "a");
        assert!(!root.join("old").exists());
        assert!(rename(root, &root.join("new/A"), &root.join("new/a")).unwrap());
        assert_eq!(fs::read_to_string(root.join("new/a")).unwrap(), "a");
    }

    #[test]
    fn only_another_process_s_temporary_files_are_left_over() {
        let target = Path::new("/root/a");
        assert!(!is_left_over(&temporary_path(target)));
        let other = std::process::id().wrapping_add(1);
        assert!(is_left_over(
            &target.with_file_name(format!("{TEMPORARY_PREFIX}{other}-0"))
        ));
        assert!(is_left_over(
            &target.with_file_name(format!("{TEMPORARY_PREFIX}a.txt"))
        ));
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
