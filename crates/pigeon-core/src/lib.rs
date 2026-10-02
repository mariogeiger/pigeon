//! pigeon's model, free of I/O: member names, portable paths, clocks, keys,
//! patches, announced drafts, ownership, statements, selections, places,
//! retention, the ledger that folds every patch into one tree, and the
//! restoring of files to a past time.

pub mod clock;
pub mod draft;
pub mod identity;
pub mod ledger;
pub mod name;
pub mod ownership;
pub mod patch;
pub mod patch_v1;
pub mod path;
pub mod places;
pub mod restore;
pub mod retention;
pub mod selection;
pub mod statement;
#[cfg(any(test, feature = "test-support"))]
pub mod test_machines;
