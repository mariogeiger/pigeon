//! A group's data directory: where its configuration, its secrets, the
//! state database and the blob store live, and the one way pigeon writes
//! its files there, whole and readable only by their owner.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Result, StoreError};

/// The directory holding one group's local state.
#[derive(Clone, Debug)]
pub struct DataDir {
    path: PathBuf,
}

impl DataDir {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The group's configuration, which the user may edit.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.path.join("config.toml")
    }

    /// The group key and the machine's secret key.
    #[must_use]
    pub fn secrets_path(&self) -> PathBuf {
        self.path.join("secrets.toml")
    }

    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        self.path.join("state.redb")
    }

    #[must_use]
    pub fn blobs_path(&self) -> PathBuf {
        self.path.join("blobs")
    }
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
