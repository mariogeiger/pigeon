//! The folders of one group on one machine: its configuration's, which
//! the user may edit, and its data's, which holds its secrets, the state
//! database and the blob store; one folder may be both. And the one way
//! pigeon writes its files there, whole and readable only by their owner.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Result, StoreError};

/// Where one group keeps its files on this machine.
#[derive(Clone, Debug)]
pub struct GroupDirs {
    config: PathBuf,
    data: PathBuf,
}

impl GroupDirs {
    /// The group whose configuration lives in `config` and whose data
    /// lives in `data`.
    #[must_use]
    pub fn new(config: impl Into<PathBuf>, data: impl Into<PathBuf>) -> Self {
        Self {
            config: config.into(),
            data: data.into(),
        }
    }

    /// The folder of the group's configuration.
    #[must_use]
    pub fn config(&self) -> &Path {
        &self.config
    }

    /// The folder of the group's data.
    #[must_use]
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// The group's configuration, which the user may edit.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    /// The group key and the machine's secret key.
    #[must_use]
    pub fn secrets_path(&self) -> PathBuf {
        self.data.join("secrets.toml")
    }

    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        self.data.join("state.redb")
    }

    #[must_use]
    pub fn blobs_path(&self) -> PathBuf {
        self.data.join("blobs")
    }

    /// Removes both folders and all they hold.
    ///
    /// # Errors
    ///
    /// Fails if a folder that exists cannot be removed.
    pub fn remove(&self) -> Result<()> {
        for folder in [&self.config, &self.data] {
            match fs::remove_dir_all(folder) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(StoreError::io(folder)(error));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Moves the file or folder `from` to `to`, unless `to` exists or `from`
/// does not; says whether it moved.
///
/// # Errors
///
/// Fails, saying to move it by hand, if it cannot move, as from one disk to
/// another.
pub fn move_into_place(from: &Path, to: &Path) -> Result<bool> {
    if from == to || !from.exists() || to.exists() {
        return Ok(false);
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(StoreError::io(parent))?;
    }
    fs::rename(from, to).map_err(|error| {
        StoreError::Invalid(format!(
            "moving {} to {}: {error}; move it there by hand",
            from.display(),
            to.display()
        ))
    })?;
    Ok(true)
}

/// Reads the text file at `path`, none if it does not exist.
///
/// # Errors
///
/// Fails if the file exists but cannot be read as text.
pub fn read_if_present(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(StoreError::io(path)(error)),
    }
}

/// Writes a file readable only by its owner, through a temporary file so
/// that a crash never leaves it half written.
///
/// # Errors
///
/// Fails if the file cannot be written.
///
/// # Panics
///
/// Panics if `path` names no file in a folder.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().expect("a data file has a parent");
    fs::create_dir_all(parent).map_err(StoreError::io(parent))?;
    let temporary = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(&temporary)
        .map_err(StoreError::io(&temporary))?;
    std::io::Write::write_all(&mut file, bytes).map_err(StoreError::io(&temporary))?;
    file.sync_all().map_err(StoreError::io(&temporary))?;
    fs::rename(&temporary, path).map_err(StoreError::io(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_moves_into_place_once_and_never_over_another() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a"), dir.path().join("b").join("a"));
        assert!(!move_into_place(&from, &to).unwrap());
        fs::write(&from, "one").unwrap();
        assert!(move_into_place(&from, &to).unwrap());
        assert_eq!(fs::read_to_string(&to).unwrap(), "one");
        fs::write(&from, "two").unwrap();
        assert!(!move_into_place(&from, &to).unwrap());
        assert_eq!(fs::read_to_string(&to).unwrap(), "one");
        assert!(!move_into_place(&to, &to).unwrap());
        let group = GroupDirs::new(dir.path().join("config"), dir.path().join("data"));
        write_private(&group.config_path(), b"").unwrap();
        write_private(&group.secrets_path(), b"").unwrap();
        group.remove().unwrap();
        assert!(!group.config().exists() && !group.data().exists());
        group.remove().unwrap();
    }

    #[test]
    fn a_private_file_is_written_whole_for_its_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g").join("secrets.toml");
        assert_eq!(read_if_present(&path).unwrap(), None);
        write_private(&path, b"one").unwrap();
        write_private(&path, b"two").unwrap();
        assert_eq!(read_if_present(&path).unwrap().as_deref(), Some("two"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
