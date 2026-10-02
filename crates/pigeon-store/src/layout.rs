//! Keeping a folder of the root at its destination: moving what the folder
//! holds into the destination and leaving at its place a link, a junction
//! on Windows, which needs no administrator, and the reverse. A move never
//! overwrites a file, and a destination that is missing, such as a disk
//! not plugged in, leaves every file where it is.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Result, StoreError};

/// Where the link at `location` leads, if a link is there.
#[must_use]
pub fn link_target(location: &Path) -> Option<PathBuf> {
    let metadata = fs::symlink_metadata(location).ok()?;
    if !metadata.file_type().is_symlink() {
        return None;
    }
    #[cfg(windows)]
    if let Ok(target) = junction::get_target(location) {
        return Some(target);
    }
    fs::read_link(location).ok()
}

/// Whether `a` and `b` name one folder, as given or as the system resolves
/// them.
fn same_folder(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .is_ok_and(|a| b.canonicalize().is_ok_and(|b| a == b))
}

/// Whether `location` is a link to the folder `destination`, and that
/// folder is there.
#[must_use]
pub fn is_in_place(location: &Path, destination: &Path) -> bool {
    link_target(location).is_some_and(|target| same_folder(&target, destination))
        && destination.is_dir()
}

fn make_link(destination: &Path, location: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(destination, location);
    #[cfg(windows)]
    return junction::create(destination, location);
}

fn remove_link(location: &Path) -> io::Result<()> {
    if cfg!(windows) {
        fs::remove_dir(location)
    } else {
        fs::remove_file(location)
    }
}

fn unavailable(destination: &Path) -> StoreError {
    StoreError::Invalid(format!(
        "{} is not there: is its disk plugged in?",
        destination.display()
    ))
}

/// Moves the folder at `location` to `destination`, then links it there.
///
/// # Errors
///
/// Fails if the destination's parent folder is missing, another link or a
/// file is at `location`, or a file of the folder cannot move, for want of
/// rights or because the destination already holds one by its name.
pub fn place(location: &Path, destination: &Path) -> Result<()> {
    if let Some(target) = link_target(location)
        && !same_folder(&target, destination)
    {
        return Err(StoreError::Invalid(format!(
            "{} is a link to {}",
            location.display(),
            target.display()
        )));
    }
    if !destination.parent().is_some_and(Path::is_dir) {
        return Err(unavailable(destination));
    }
    match fs::create_dir(destination) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
            return Err(StoreError::io(destination)(error));
        }
        _ if !destination.is_dir() => return Err(unavailable(destination)),
        _ => {}
    }
    if link_target(location).is_some() {
        return Ok(());
    }
    if location.is_dir() {
        move_tree(location, destination)?;
        fs::remove_dir(location).map_err(StoreError::io(location))?;
    } else if location.exists() {
        return Err(StoreError::Invalid(format!(
            "{} is a file, not a folder",
            location.display()
        )));
    } else if let Some(parent) = location.parent() {
        fs::create_dir_all(parent).map_err(StoreError::io(parent))?;
    }
    make_link(destination, location).map_err(StoreError::io(location))
}

/// Moves the folder back from `destination` to `location`, in the root.
///
/// # Errors
///
/// Fails if the destination is missing, or a file cannot move back.
pub fn unplace(location: &Path, destination: &Path) -> Result<()> {
    if !destination.is_dir() {
        return Err(unavailable(destination));
    }
    if link_target(location).is_some() {
        remove_link(location).map_err(StoreError::io(location))?;
    }
    fs::create_dir_all(location).map_err(StoreError::io(location))?;
    move_tree(destination, location)?;
    let _ = fs::remove_dir(destination);
    Ok(())
}

/// Moves everything inside `from` into `to`, merging folders, then removes
/// the folders it emptied.
///
/// # Errors
///
/// Fails, after moving all it could, if an item could not move.
pub fn move_tree(from: &Path, to: &Path) -> Result<()> {
    let mut left = Vec::new();
    move_entries(from, to, &mut left)?;
    match left.first() {
        None => Ok(()),
        Some((location, reason)) => Err(StoreError::Invalid(format!(
            "{} stays where it is: {reason}{}",
            location.display(),
            match left.len() {
                1 => String::new(),
                more => format!(", with {} other items", more - 1),
            }
        ))),
    }
}

fn move_entries(from: &Path, to: &Path, left: &mut Vec<(PathBuf, String)>) -> Result<()> {
    for entry in fs::read_dir(from).map_err(StoreError::io(from))? {
        let entry = entry.map_err(StoreError::io(from))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().map_err(StoreError::io(&source))?;
        if kind.is_dir() {
            fs::create_dir_all(&target).map_err(StoreError::io(&target))?;
            move_entries(&source, &target, left)?;
            let _ = fs::remove_dir(&source);
        } else if fs::symlink_metadata(&target).is_ok() {
            left.push((source, format!("{} exists", target.display())));
        } else if let Err(error) = move_file(&source, &target, kind.is_file()) {
            left.push((source, error.to_string()));
        }
    }
    Ok(())
}

/// Renames `source` to `target`, or, across disks, copies a regular file
/// with its time and bits and then removes the source.
fn move_file(source: &Path, target: &Path, regular: bool) -> io::Result<()> {
    let Err(error) = fs::rename(source, target) else {
        return Ok(());
    };
    if !regular {
        return Err(error);
    }
    if let Err(error) = copy_file(source, target) {
        let _ = fs::remove_file(target);
        return Err(error);
    }
    fs::remove_file(source)
}

/// Copies the regular file `source` to the new file `target`, with its
/// modification time and bits.
fn copy_file(source: &Path, target: &Path) -> io::Result<()> {
    let metadata = fs::metadata(source)?;
    let mut writer = fs::File::create_new(target)?;
    io::copy(&mut fs::File::open(source)?, &mut writer)?;
    writer.set_modified(metadata.modified()?)?;
    drop(writer);
    fs::set_permissions(target, metadata.permissions())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read_only(path: &Path, executable: bool) {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o555 } else { 0o444 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_folder_moves_to_its_destination_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let location = dir.path().join("root/videos");
        let destination = dir.path().join("disk/videos");
        fs::create_dir(dir.path().join("disk")).unwrap();
        write(&location.join("2026/a.mp4"), "a");
        read_only(&location.join("2026/a.mp4"), false);
        place(&location, &destination).unwrap();
        assert!(is_in_place(&location, &destination));
        assert_eq!(
            fs::read_to_string(destination.join("2026/a.mp4")).unwrap(),
            "a"
        );
        assert_eq!(
            fs::read_to_string(location.join("2026/a.mp4")).unwrap(),
            "a"
        );
        place(&location, &destination).unwrap();
        unplace(&location, &destination).unwrap();
        assert!(link_target(&location).is_none());
        assert_eq!(
            fs::read_to_string(location.join("2026/a.mp4")).unwrap(),
            "a"
        );
        assert!(!destination.exists());
    }

    #[test]
    fn a_missing_folder_is_linked_to_its_destination() {
        let dir = tempfile::tempdir().unwrap();
        let location = dir.path().join("root/a/videos");
        let destination = dir.path().join("videos");
        place(&location, &destination).unwrap();
        assert!(is_in_place(&location, &destination));
    }

    #[test]
    fn a_missing_disk_moves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let location = dir.path().join("root/videos");
        write(&location.join("a.mp4"), "a");
        let destination = dir.path().join("unplugged/videos");
        assert!(place(&location, &destination).is_err());
        assert!(!dir.path().join("unplugged").exists());
        assert_eq!(fs::read_to_string(location.join("a.mp4")).unwrap(), "a");
        assert!(unplace(&location, &destination).is_err());
        assert_eq!(fs::read_to_string(location.join("a.mp4")).unwrap(), "a");
    }

    #[test]
    fn a_move_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let location = dir.path().join("root/videos");
        let destination = dir.path().join("videos");
        write(&location.join("a.mp4"), "root");
        write(&location.join("b.mp4"), "b");
        write(&destination.join("a.mp4"), "disk");
        let error = place(&location, &destination).unwrap_err().to_string();
        assert!(error.contains("a.mp4 stays where it is"), "{error}");
        assert_eq!(fs::read_to_string(location.join("a.mp4")).unwrap(), "root");
        assert_eq!(
            fs::read_to_string(destination.join("a.mp4")).unwrap(),
            "disk"
        );
        assert_eq!(fs::read_to_string(destination.join("b.mp4")).unwrap(), "b");
        assert!(link_target(&location).is_none());
    }

    #[test]
    fn a_file_copied_across_disks_keeps_its_time_and_bits() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a");
        write(&source, "a");
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(7);
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(time)
            .unwrap();
        read_only(&source, true);
        let before = crate::disk::Stat::read(&source).unwrap();
        let target = dir.path().join("b");
        copy_file(&source, &target).unwrap();
        assert!(copy_file(&source, &target).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "a");
        assert_eq!(crate::disk::Stat::read(&target).unwrap(), before);
    }
}
