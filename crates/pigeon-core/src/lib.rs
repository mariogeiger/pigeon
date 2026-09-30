//! pigeon's model, free of I/O: member names, portable paths, clocks, keys,
//! patches, folder rules, statements, selections, retention, and the ledger
//! that folds every patch into one tree.

pub mod clock;
pub mod folder;
pub mod identity;
pub mod ledger;
pub mod name;
pub mod patch;
pub mod path;
pub mod retention;
pub mod selection;
pub mod statement;
