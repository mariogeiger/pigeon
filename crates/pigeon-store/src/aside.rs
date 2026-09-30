//! The set-aside list's items: content the disk held that pigeon may not
//! publish as it is, kept on this machine only, each a patch that nobody
//! signed.

use pigeon_core::clock::Stamp;
use pigeon_core::patch::Content;
use serde::{Deserialize, Serialize};

/// Why pigeon set content aside.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Reason {
    /// An edit to a file this member cannot write.
    NotWritable,
    /// A name no portable path can hold, or one that collides by case.
    Unportable(String),
    /// A patch the ledger rejected, such as a lost claim.
    Rejected(String),
    /// The losing side of concurrent changes by one member's machines.
    Superseded,
}

/// One item of the set-aside list.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AsideItem {
    /// Where the content was found, relative to the root; not always a
    /// valid group path.
    pub path: String,
    /// The content, stored in the blob store, or `None` for a deletion.
    pub content: Option<Content>,
    /// The version the content was based on, if any.
    pub replaces: Option<Stamp>,
    pub reason: Reason,
    /// When pigeon set it aside, in NTP64 time.
    pub time: u64,
}
