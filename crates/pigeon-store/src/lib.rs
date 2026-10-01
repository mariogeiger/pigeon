//! pigeon's local state for one group on one machine: the group's folders,
//! its configuration and secrets, the group key, the state database of
//! patches, disk index, set-aside list and placed folders, the blob store,
//! walking, writing and laying out the root, and reading the group's files
//! as older versions of pigeon wrote them.

pub mod aside;
pub mod blobs;
pub mod config;
pub mod disk;
pub mod error;
pub mod group_dirs;
pub mod group_key;
pub mod index;
pub mod layout;
pub mod legacy;
pub mod scan;
pub mod secrets;
pub mod state;

pub use error::{Result, StoreError};
