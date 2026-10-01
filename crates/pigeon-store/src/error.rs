//! The one error type of local storage: every failure to read or write the
//! group's folders, the state database, the blob store, or the group root.

use std::path::PathBuf;

/// A failure of local storage.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("state database: {0}")]
    Database(#[from] redb::Error),
    #[error("state database holds an unreadable record: {0}")]
    Record(#[from] postcard::Error),
    #[error("blob store: {0}")]
    Blobs(String),
    #[error("{0}")]
    Invalid(String),
}

impl StoreError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }
}

macro_rules! database_error {
    ($($kind:ty),*) => {$(
        impl From<$kind> for StoreError {
            fn from(error: $kind) -> Self {
                Self::Database(error.into())
            }
        }
    )*};
}

database_error!(
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError
);

pub type Result<T, E = StoreError> = std::result::Result<T, E>;
