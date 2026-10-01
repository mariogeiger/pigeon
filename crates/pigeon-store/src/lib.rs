//! pigeon's local state for one group on one machine: the data directory
//! and group key, the state database of patches, disk index, set-aside
//! list, and settings, the blob store, and walking, writing and laying out
//! the root.

pub mod aside;
pub mod blobs;
pub mod config;
pub mod disk;
pub mod error;
pub mod group_key;
pub mod index;
pub mod layout;
pub mod scan;
pub mod state;

pub use error::{Result, StoreError};
