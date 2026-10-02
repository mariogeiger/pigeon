//! The state database of one group on one machine: every patch received,
//! the disk index, the suggestions whose content the disk keeps, the
//! folders the disk holds at other destinations, and the selection the
//! disk was last brought to, in one redb file whose transactions keep them
//! consistent across crashes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use pigeon_core::clock::Stamp;
use pigeon_core::identity::GroupId;
use pigeon_core::ledger::Ledger;
use pigeon_core::patch::{Content, SignedPatch};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::places::{Place, Places};
use pigeon_core::selection::Rule;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::error::{Result, StoreError};
use crate::index::IndexEntry;

pub(crate) const PATCHES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("signed patches");
const INDEX: TableDefinition<&str, &[u8]> = TableDefinition::new("index");
const KEPT: TableDefinition<&str, &[u8]> = TableDefinition::new("kept suggestions");
const PLACED: TableDefinition<&str, &str> = TableDefinition::new("placed");
const APPLIED: TableDefinition<&str, &[u8]> = TableDefinition::new("applied");
const SELECTION: &str = "selection";

/// Content that the disk keeps at a path while the suggestion this
/// machine made of it waits: `None` for a deletion, the disk then showing
/// no file there.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Kept {
    pub statement: GroupPath,
    pub content: Option<Content>,
}

/// A patch's key, which sorts patches in stamp order.
pub(crate) fn stamp_key(stamp: &Stamp) -> [u8; 40] {
    let mut key = [0; 40];
    key[..8].copy_from_slice(&stamp.time.to_be_bytes());
    key[8..].copy_from_slice(stamp.machine.as_bytes());
    key
}

/// The state database.
pub struct State {
    database: Database,
    revision: AtomicU64,
}

impl State {
    /// Opens the database at `path`, creating it and its tables if needed.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be opened or is not a state database.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(StoreError::io(parent))?;
        }
        let database = Database::create(path)?;
        let transaction = database.begin_write()?;
        transaction.open_table(PATCHES)?;
        crate::state_v1::upgrade_patches(&transaction)?;
        transaction.open_table(INDEX)?;
        transaction.open_table(KEPT)?;
        transaction.open_table(PLACED)?;
        transaction.open_table(APPLIED)?;
        transaction.commit()?;
        Ok(Self {
            database,
            revision: AtomicU64::new(0),
        })
    }

    /// The database itself, where older versions of pigeon kept more.
    pub(crate) fn database(&self) -> &Database {
        &self.database
    }

    /// How many writes this handle committed: a new value means the state
    /// may read differently.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// Records a patch; recording one twice changes nothing.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn add_patch(&self, patch: &SignedPatch) -> Result<()> {
        let bytes = postcard::to_stdvec(patch)?;
        let transaction = self.database.begin_write()?;
        transaction
            .open_table(PATCHES)?
            .insert(stamp_key(&patch.stamp()).as_slice(), bytes.as_slice())?;
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Every recorded patch, in stamp order.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn patches(&self) -> Result<Vec<SignedPatch>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(PATCHES)?;
        let mut patches = Vec::new();
        for entry in table.iter()? {
            let (_, value) = entry?;
            patches.push(postcard::from_bytes(value.value())?);
        }
        Ok(patches)
    }

    /// The ledger folded from every recorded patch. A patch whose signature
    /// fails is dropped, as it would have been on receipt.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn ledger(&self, group: GroupId) -> Result<Ledger> {
        let mut ledger = Ledger::new(group);
        for patch in self.patches()? {
            let _ = ledger.insert(patch);
        }
        Ok(ledger)
    }

    /// The index entry of one path.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn index_entry(&self, key: &PathKey) -> Result<Option<IndexEntry>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(INDEX)?;
        let value = table.get(key.as_str())?;
        Ok(value
            .map(|value| postcard::from_bytes(value.value()))
            .transpose()?)
    }

    /// The index entries at `under` and inside it, or every entry, by path
    /// key.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn index(&self, under: Option<&GroupPath>) -> Result<Vec<IndexEntry>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(INDEX)?;
        let start = under.map(GroupPath::key);
        let range = match &start {
            Some(key) => table.range(key.as_str()..)?,
            None => table.range::<&str>(..)?,
        };
        let mut entries = Vec::new();
        for entry in range {
            let (key, value) = entry?;
            let key = key.value();
            if let Some(start) = &start {
                let Some(rest) = key.strip_prefix(start.as_str()) else {
                    break;
                };
                if !rest.is_empty() && !rest.starts_with('/') {
                    continue;
                }
            }
            entries.push(postcard::from_bytes(value.value())?);
        }
        Ok(entries)
    }

    /// Replaces or removes index entries in one transaction: `Some` sets
    /// the entry of its path, `None` removes the entry at the key.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn update_index<'a>(
        &self,
        updates: impl IntoIterator<Item = (&'a PathKey, Option<&'a IndexEntry>)>,
    ) -> Result<()> {
        let transaction = self.database.begin_write()?;
        {
            let mut table = transaction.open_table(INDEX)?;
            for (key, entry) in updates {
                match entry {
                    Some(entry) => {
                        let bytes = postcard::to_stdvec(entry)?;
                        table.insert(key.as_str(), bytes.as_slice())?;
                    }
                    None => {
                        table.remove(key.as_str())?;
                    }
                }
            }
        }
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Records that the disk keeps `kept` at `path`, a path relative to
    /// the root that no portable path may hold.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn keep(&self, path: &str, kept: &Kept) -> Result<()> {
        let bytes = postcard::to_stdvec(kept)?;
        let transaction = self.database.begin_write()?;
        transaction
            .open_table(KEPT)?
            .insert(path, bytes.as_slice())?;
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Forgets what the disk keeps at `path`.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn unkeep(&self, path: &str) -> Result<()> {
        let transaction = self.database.begin_write()?;
        transaction.open_table(KEPT)?.remove(path)?;
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// What the disk keeps at `path` for a suggestion, if anything.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn kept_at(&self, path: &str) -> Result<Option<Kept>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(KEPT)?;
        let value = table.get(path)?;
        Ok(value
            .map(|value| postcard::from_bytes(value.value()))
            .transpose()?)
    }

    /// What the disk keeps for suggestions, by path relative to the root.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn kept(&self) -> Result<BTreeMap<String, Kept>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(KEPT)?;
        let mut kept = BTreeMap::new();
        for entry in table.iter()? {
            let (path, value) = entry?;
            kept.insert(
                path.value().to_owned(),
                postcard::from_bytes(value.value())?,
            );
        }
        Ok(kept)
    }

    /// The rules of the selection the disk was last brought to, if any was.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn applied_selection(&self) -> Result<Option<Vec<Rule>>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(APPLIED)?;
        let value = table.get(SELECTION)?;
        Ok(value
            .map(|value| postcard::from_bytes(value.value()))
            .transpose()?)
    }

    /// Records the rules of the selection the disk was brought to.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn set_applied_selection(&self, rules: &[Rule]) -> Result<()> {
        let bytes = postcard::to_stdvec(rules)?;
        let transaction = self.database.begin_write()?;
        transaction
            .open_table(APPLIED)?
            .insert(SELECTION, bytes.as_slice())?;
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// The folders the disk holds at other destinations, as last moved.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read or names an invalid folder.
    pub fn placed(&self) -> Result<Places> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(PLACED)?;
        let mut list = Vec::new();
        for entry in table.iter()? {
            let (folder, destination) = entry?;
            let folder = GroupPath::parse(folder.value()).map_err(|error| {
                StoreError::Invalid(format!(
                    "the state database holds a placed folder that is no path: {error}"
                ))
            })?;
            list.push(Place {
                folder,
                destination: destination.value().into(),
            });
        }
        Ok(Places::new(list))
    }

    /// Records the folders the disk holds at other destinations.
    ///
    /// # Errors
    ///
    /// Fails if a destination is not valid Unicode or the database cannot
    /// be written.
    pub fn set_placed(&self, placed: &Places) -> Result<()> {
        let transaction = self.database.begin_write()?;
        transaction.delete_table(PLACED)?;
        {
            let mut table = transaction.open_table(PLACED)?;
            for place in placed.iter() {
                let destination = place.destination.to_str().ok_or_else(|| {
                    StoreError::Invalid(format!(
                        "{} is not valid Unicode",
                        place.destination.display()
                    ))
                })?;
                table.insert(place.folder.as_str(), destination)?;
            }
        }
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
