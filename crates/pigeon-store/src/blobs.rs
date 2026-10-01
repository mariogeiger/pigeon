//! The blob store: file contents by BLAKE3 hash, in iroh-blobs' file store,
//! whose garbage collector keeps exactly the hashes pigeon protects, and
//! everything until pigeon first says which, on a disk whose size bounds
//! the history.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh_blobs::Hash;
use iroh_blobs::api::TempTag;
use iroh_blobs::api::proto::BlobStatus;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::fs::options::Options;
use iroh_blobs::store::{GcConfig, ProtectOutcome};
use pigeon_core::patch::ContentHash;

use crate::disk;
use crate::error::{Result, StoreError};

fn blob_error(error: impl std::fmt::Display) -> StoreError {
    StoreError::Blobs(error.to_string())
}

/// The iroh-blobs hash of a content hash; both are BLAKE3.
#[must_use]
pub fn blob_hash(hash: &ContentHash) -> Hash {
    Hash::from_bytes(hash.0)
}

/// The hashes garbage collection keeps, once pigeon computed them.
#[derive(Default)]
struct Protected {
    hashes: HashSet<Hash>,
    computed: bool,
}

/// One group's blob store.
#[derive(Clone)]
pub struct Blobs {
    store: FsStore,
    protected: Arc<Mutex<Protected>>,
    path: PathBuf,
}

impl Blobs {
    /// Opens the store in `path`, collecting garbage every `gc_interval`
    /// once [`Blobs::protect`] said what to keep.
    ///
    /// # Errors
    ///
    /// Fails if the store cannot be opened.
    ///
    /// # Panics
    ///
    /// Collection panics if a thread panicked while holding the protected
    /// set.
    pub async fn open(path: &Path, gc_interval: Duration) -> Result<Self> {
        let protected = Arc::new(Mutex::new(Protected::default()));
        let shared = protected.clone();
        let options = Options {
            gc: Some(GcConfig {
                interval: gc_interval,
                add_protected: Some(Arc::new(move |live: &mut HashSet<Hash>| {
                    let protected = shared.lock().expect("no panic holds the lock");
                    live.extend(protected.hashes.iter());
                    let outcome = if protected.computed {
                        ProtectOutcome::Continue
                    } else {
                        ProtectOutcome::Abort
                    };
                    Box::pin(async move { outcome })
                })),
            }),
            ..Options::new(path)
        };
        let store = FsStore::load_with_opts(path.join("blobs.db"), options)
            .await
            .map_err(blob_error)?;
        Ok(Self {
            store,
            protected,
            path: path.to_path_buf(),
        })
    }

    /// The size in bytes of the disk holding the store.
    ///
    /// # Errors
    ///
    /// Fails if the system cannot tell.
    pub fn disk_size(&self) -> Result<u64> {
        fs4::total_space(&self.path).map_err(StoreError::io(&self.path))
    }

    /// The underlying store, which the network serves and fills.
    #[must_use]
    pub fn store(&self) -> &FsStore {
        &self.store
    }

    /// Replaces the set of hashes garbage collection keeps, letting it
    /// collect the others.
    ///
    /// # Panics
    ///
    /// Panics if a thread panicked while holding the set.
    pub fn protect(&self, hashes: HashSet<Hash>) {
        *self.protected.lock().expect("no panic holds the lock") = Protected {
            hashes,
            computed: true,
        };
    }

    /// Adds `hash` to the set garbage collection keeps, until the next
    /// [`Blobs::protect`] replaces it.
    ///
    /// # Panics
    ///
    /// Panics if a thread panicked while holding the set.
    pub fn protect_also(&self, hash: Hash) {
        self.protected
            .lock()
            .expect("no panic holds the lock")
            .hashes
            .insert(hash);
    }

    /// Copies the file at `path` into the store and returns its size. The
    /// returned tag keeps it from garbage collection until it is protected
    /// or dropped.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read.
    pub async fn import(&self, path: &Path) -> Result<(TempTag, u64)> {
        let tag = self
            .store
            .blobs()
            .add_path(path)
            .temp_tag()
            .await
            .map_err(blob_error)?;
        match self
            .store
            .blobs()
            .status(tag.hash())
            .await
            .map_err(blob_error)?
        {
            BlobStatus::Complete { size } => Ok((tag, size)),
            _ => Err(StoreError::Blobs(format!(
                "{} was not stored whole",
                path.display()
            ))),
        }
    }

    /// Whether the store holds the whole content.
    ///
    /// # Errors
    ///
    /// Fails if the store cannot answer.
    pub async fn has(&self, hash: &ContentHash) -> Result<bool> {
        self.store
            .blobs()
            .has(blob_hash(hash))
            .await
            .map_err(blob_error)
    }

    /// Writes the content to `target` at once, creating its folders and
    /// giving it its bits.
    ///
    /// # Errors
    ///
    /// Fails if the store lacks the content or the file cannot be written.
    ///
    /// # Panics
    ///
    /// Panics if `target` has no parent folder.
    pub async fn export(
        &self,
        hash: &ContentHash,
        target: &Path,
        executable: bool,
        writable: bool,
    ) -> Result<()> {
        let parent = target.parent().expect("a file has a parent");
        std::fs::create_dir_all(parent).map_err(StoreError::io(parent))?;
        let temporary = disk::temporary_path(target);
        self.store
            .blobs()
            .export(blob_hash(hash), &temporary)
            .await
            .map_err(blob_error)?;
        disk::install(&temporary, target, executable, writable)
    }

    /// Reads a whole content into memory, for statements and small files.
    ///
    /// # Errors
    ///
    /// Fails if the store lacks the content.
    pub async fn read(&self, hash: &ContentHash) -> Result<Vec<u8>> {
        let bytes = self
            .store
            .blobs()
            .get_bytes(blob_hash(hash))
            .await
            .map_err(blob_error)?;
        Ok(bytes.to_vec())
    }

    /// Stores bytes, such as a statement pigeon writes itself.
    ///
    /// # Errors
    ///
    /// Fails if the store cannot be written.
    pub async fn add_bytes(&self, bytes: Vec<u8>) -> Result<TempTag> {
        self.store
            .blobs()
            .add_bytes(bytes)
            .temp_tag()
            .await
            .map_err(blob_error)
    }

    /// Stops the store, flushing what it holds.
    ///
    /// # Errors
    ///
    /// Fails if the store does not shut down cleanly.
    pub async fn shutdown(&self) -> Result<()> {
        self.store.shutdown().await.map_err(blob_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::hash_file;

    #[tokio::test]
    async fn imports_and_exports_by_blake3_hash() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(&dir.path().join("blobs"), Duration::from_secs(3600))
            .await
            .unwrap();
        let source = dir.path().join("source");
        std::fs::write(&source, vec![7u8; 100_000]).unwrap();
        let (tag, size) = blobs.import(&source).await.unwrap();
        assert_eq!(size, 100_000);
        let hash = hash_file(&source).unwrap();
        assert_eq!(tag.hash(), blob_hash(&hash));
        assert!(blobs.has(&hash).await.unwrap());
        let target = dir.path().join("root/a/b");
        blobs.export(&hash, &target, false, false).await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), vec![7u8; 100_000]);
        assert!(std::fs::metadata(&target).unwrap().permissions().readonly());
        blobs.export(&hash, &target, false, true).await.unwrap();
        assert!(!std::fs::metadata(&target).unwrap().permissions().readonly());
        assert!(!disk::temporary_path(&target).exists());
        blobs.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn garbage_collection_keeps_everything_until_told_what_to_keep() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(&dir.path().join("blobs"), Duration::from_millis(20))
            .await
            .unwrap();
        let tag = blobs
            .add_bytes(b"published offline".to_vec())
            .await
            .unwrap();
        let hash = ContentHash(*tag.hash().as_bytes());
        drop(tag);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(blobs.has(&hash).await.unwrap());
        blobs.protect(HashSet::new());
        for _ in 0..100 {
            if !blobs.has(&hash).await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!blobs.has(&hash).await.unwrap());
        blobs.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn garbage_collection_keeps_exactly_the_protected_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(&dir.path().join("blobs"), Duration::from_millis(50))
            .await
            .unwrap();
        let kept = blobs.add_bytes(b"kept".to_vec()).await.unwrap();
        let dropped = blobs.add_bytes(b"dropped".to_vec()).await.unwrap();
        let (kept_hash, dropped_hash) = (kept.hash(), dropped.hash());
        blobs.protect(HashSet::from([kept_hash]));
        drop((kept, dropped));
        let content = |hash: Hash| ContentHash(*hash.as_bytes());
        for _ in 0..100 {
            if !blobs.has(&content(dropped_hash)).await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!blobs.has(&content(dropped_hash)).await.unwrap());
        assert!(blobs.has(&content(kept_hash)).await.unwrap());
        assert_eq!(blobs.read(&content(kept_hash)).await.unwrap(), b"kept");
        blobs.shutdown().await.unwrap();
    }
}
