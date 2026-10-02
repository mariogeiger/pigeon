//! Files sent to the daemon: each arrives in chunks and is written to a
//! file beside the data it is going to, so that the memory it takes does
//! not depend on its size, and is deleted once the call that carries it is
//! done.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use tempfile::{NamedTempFile, TempPath};
use tokio::io::AsyncWriteExt;

/// A file received, deleted when dropped.
pub struct Upload {
    path: TempPath,
}

impl Upload {
    /// Receives a file in the folder `folder`, which it makes if needed,
    /// from `chunks`.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be written or `chunks` fails.
    pub async fn receive<E: std::fmt::Display>(
        folder: &Path,
        mut chunks: impl Stream<Item = Result<Bytes, E>> + Unpin,
    ) -> Result<Self> {
        std::fs::create_dir_all(folder)
            .with_context(|| format!("creating {}", folder.display()))?;
        let (file, path) = NamedTempFile::new_in(folder)
            .with_context(|| format!("creating a file in {}", folder.display()))?
            .into_parts();
        let mut file = tokio::fs::File::from_std(file);
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|error| anyhow!("the upload broke off: {error}"))?;
            file.write_all(&chunk).await.context("storing the upload")?;
        }
        file.flush().await.context("storing the upload")?;
        Ok(Self { path })
    }

    /// Where the file is, until this is dropped.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Removes the files left by a daemon that stopped during an upload.
///
/// # Errors
///
/// Fails if the folder exists and cannot be removed.
pub fn clear(folder: &Path) -> Result<()> {
    match std::fs::remove_dir_all(folder) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("clearing {}", folder.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chunks_make_one_file_that_goes_with_the_upload() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("uploads");
        let chunks = futures_util::stream::iter(
            [Bytes::from("hel"), Bytes::from("lo")].map(Ok::<_, String>),
        );
        let upload = Upload::receive(&folder, chunks).await.unwrap();
        let path = upload.path().to_path_buf();
        assert!(path.starts_with(&folder));
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        drop(upload);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_transfer_that_breaks_off_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let chunks = futures_util::stream::iter([Err::<Bytes, _>("reset")]);
        let error = Upload::receive(dir.path(), chunks).await.err().unwrap();
        assert!(error.to_string().contains("broke off: reset"), "{error}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn clearing_a_folder_that_is_not_there_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        clear(&dir.path().join("none")).unwrap();
        std::fs::write(dir.path().join("left"), "x").unwrap();
        clear(dir.path()).unwrap();
        assert!(!dir.path().exists());
    }
}
