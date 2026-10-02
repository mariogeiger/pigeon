//! pigeon's local state for one group on one machine: the group's folders,
//! its configuration and secrets, the group key, the state database of
//! patches, disk index, kept suggestions and placed folders, the blob
//! store, walking, writing and laying out the root, and reading the group's
//! files and state as older versions of pigeon wrote them.

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
pub mod state_v1;

pub use error::{Result, StoreError};
