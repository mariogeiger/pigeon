//! The blob store: file contents by BLAKE3 hash, in iroh-blobs' file store,
//! whose garbage collector keeps exactly the hashes pigeon protects, and
//! everything until pigeon first says which, on a disk whose size bounds
//! the history; a store stops whole, its collector, threads and files
//! gone, before another opens in its folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh_blobs::Hash;
use iroh_blobs::api::TempTag;
use iroh_blobs::api::proto::BlobStatus;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::fs::options::Options;
use iroh_blobs::store::{GcConfig, ProtectOutcome};
use pigeon_core::patch::ContentHash;
use tokio::io::AsyncRead;
use tokio::sync::watch;

use crate::disk;
use crate::error::{Result, StoreError};

/// A store error telling `error` and each cause under it that it does not
/// tell already.
fn blob_error(error: impl std::error::Error) -> StoreError {
    let mut told = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        let text = next.to_string();
        if !told.contains(&text) {
            told = format!("{told}: {text}");
        }
        cause = next.source();
    }
    StoreError::Blobs(told)
}

/// The iroh-blobs hash of a content hash; both are BLAKE3.
#[must_use]
pub fn blob_hash(hash: &ContentHash) -> Hash {
    Hash::from_bytes(hash.0)
}

/// How often iroh's garbage collector asks whether a collection is due:
/// at most how long a store outlives the stop of its database.
const ASKED_EVERY: Duration = Duration::from_millis(50);

/// The hashes garbage collection keeps, once pigeon computed them.
#[derive(Default)]
struct Protected {
    hashes: HashSet<Hash>,
    computed: bool,
}

/// What iroh's garbage collector asks before each collection, held by the
/// store alone, so that it goes, closing `_whole`, once the store stopped
/// whole.
struct Collector {
    protected: Arc<Mutex<Protected>>,
    collections: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
    interval: Duration,
    due: Mutex<Instant>,
    _whole: watch::Sender<()>,
}

impl Collector {
    /// Whether a collection runs now, with the hashes it keeps added to
    /// `live`: once the store's database stopped, so that the collection
    /// fails and the collector ends, and otherwise every `interval`, once
    /// pigeon said what to keep.
    fn ask(&self, live: &mut HashSet<Hash>) -> ProtectOutcome {
        if self.stopping.load(Ordering::Acquire) {
            return ProtectOutcome::Continue;
        }
        let mut due = self.due.lock().expect("no panic holds the lock");
        let protected = self.protected.lock().expect("no panic holds the lock");
        if Instant::now() < *due || !protected.computed {
            return ProtectOutcome::Abort;
        }
        *due = Instant::now() + self.interval;
        live.extend(protected.hashes.iter());
        self.collections.fetch_add(1, Ordering::Relaxed);
        ProtectOutcome::Continue
    }
}

/// One group's blob store.
#[derive(Clone)]
pub struct Blobs {
    store: FsStore,
    protected: Arc<Mutex<Protected>>,
    /// The collections begun, each ending before the next begins.
    collections: Arc<AtomicU64>,
    /// Set once the database stopped, for the collector to end.
    stopping: Arc<AtomicBool>,
    /// Closed once the store stopped whole.
    whole: watch::Receiver<()>,
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
        let collections = Arc::new(AtomicU64::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let (stopped_whole, whole) = watch::channel(());
        let collector = Collector {
            protected: protected.clone(),
            collections: collections.clone(),
            stopping: stopping.clone(),
            interval: gc_interval,
            due: Mutex::new(Instant::now() + gc_interval),
            _whole: stopped_whole,
        };
        let options = Options {
            gc: Some(GcConfig {
                interval: ASKED_EVERY.min(gc_interval),
                add_protected: Some(Arc::new(move |live: &mut HashSet<Hash>| {
                    let outcome = collector.ask(live);
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
            collections,
            stopping,
            whole,
            path: path.to_path_buf(),
        })
    }

    /// How many garbage collections began, each once pigeon said what to
    /// keep, and each ending before the next begins: a collection that
    /// begins after a moment is over once the count grew by two.
    #[must_use]
    pub fn collections(&self) -> u64 {
        self.collections.load(Ordering::Relaxed)
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

    /// Writes the content to `target` at once, flushed to the disk,
    /// creating its folders and giving it its bits, if the disk at
    /// `target` still shows `expected`, the file pigeon saw there or no
    /// file: the metadata of the file written, or `None` when `target`
    /// changed since. Nothing is left of a write that failed.
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
        expected: Option<disk::Stat>,
    ) -> Result<Option<disk::Stat>> {
        let parent = target.parent().expect("a file has a parent");
        std::fs::create_dir_all(parent).map_err(StoreError::io(parent))?;
        let temporary = disk::temporary_path(target);
        if let Err(error) = self.store.blobs().export(blob_hash(hash), &temporary).await {
            let _ = std::fs::remove_file(&temporary);
            return Err(blob_error(error));
        }
        disk::install(&temporary, target, executable, expected)
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

    /// Reads a content as a stream, for files too large to hold in memory.
    #[must_use]
    pub fn stream(&self, hash: &ContentHash) -> impl AsyncRead + Unpin + Send + use<> {
        self.store.blobs().reader(blob_hash(hash))
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

    /// Stops the store, unless it stopped already, once the requests it
    /// was given are over, and waits until it stopped whole: its
    /// collector, its threads and its files gone, once every other copy of
    /// it went too. Stopping a store again fails, finding its database
    /// gone, which is all such a failure tells, so none is reported.
    pub async fn stop_whole(self) {
        let _ = self.store.wait_idle().await;
        let _ = self.store.shutdown().await;
        self.stopping.store(true, Ordering::Release);
        let mut whole = self.whole.clone();
        drop(self);
        while whole.changed().await.is_ok() {}
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
        let written = blobs.export(&hash, &target, false, None).await.unwrap();
        assert_eq!(written, Some(disk::Stat::read(&target).unwrap()));
        assert_eq!(std::fs::read(&target).unwrap(), vec![7u8; 100_000]);
        assert!(!std::fs::metadata(&target).unwrap().permissions().readonly());
        assert!(!disk::temporary_path(&target).exists());
        blobs.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_import_tells_the_cause_under_the_store_error() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(&dir.path().join("blobs"), Duration::from_secs(3600))
            .await
            .unwrap();
        let Err(StoreError::Blobs(told)) = blobs.import(&dir.path().join("missing")).await else {
            panic!("a missing file is imported");
        };
        assert!(told.contains("not a file"), "{told}");
        blobs.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_store_that_collects_seldom_stops_whole_at_once_keeping_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blobs");
        let hour = Duration::from_secs(3600);
        let blobs = Blobs::open(&path, hour).await.unwrap();
        let tag = blobs.add_bytes(b"kept".to_vec()).await.unwrap();
        let hash = ContentHash(*tag.hash().as_bytes());
        drop(tag);
        let copy = blobs.clone();
        let stopped = tokio::time::timeout(Duration::from_secs(60), async {
            drop(blobs);
            copy.stop_whole().await;
        })
        .await;
        assert!(stopped.is_ok(), "the store outlived its stop");
        let reopened = Blobs::open(&path, hour).await.unwrap();
        assert_eq!(reopened.read(&hash).await.unwrap(), b"kept");
        reopened.stop_whole().await;
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
