//! pigeon's local state for one group on one machine: the data directory,
//! its configuration and secrets, the group key, the state database of
//! patches, disk index, set-aside list and placed folders, the blob store,
//! walking, writing and laying out the root, and reading the data directory
//! as older versions of pigeon wrote it.

pub mod aside;
pub mod blobs;
pub mod config;
pub mod data_dir;
pub mod disk;
pub mod error;
pub mod group_key;
pub mod index;
pub mod layout;
pub mod legacy;
pub mod scan;
pub mod secrets;
pub mod state;

pub use error::{Result, StoreError};
