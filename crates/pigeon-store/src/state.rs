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
use redb::{
    AccessGuard, Database, ReadableDatabase, ReadableTable, TableDefinition, WriteTransaction,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Result, StoreError};
use crate::index::IndexEntry;
use crate::index_v1::IndexEntryV1;

const PATCHES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("signed patches");
const INDEX: TableDefinition<&str, &[u8]> = TableDefinition::new("index 2");
/// The index as 0.8 kept it, moved to [`INDEX`] on opening.
const INDEX_V1: TableDefinition<&str, &[u8]> = TableDefinition::new("index");
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

/// Decodes a value the database holds.
fn decode<T: DeserializeOwned>(value: &AccessGuard<'_, &[u8]>) -> Result<T> {
    Ok(postcard::from_bytes(value.value())?)
}

/// Moves the entries of the index 0.8 kept to the current index, then
/// drops the old table.
fn move_index_v1(transaction: &WriteTransaction) -> Result<()> {
    let mut moved = Vec::new();
    {
        let old = transaction.open_table(INDEX_V1)?;
        for entry in old.iter()? {
            let (key, value) = entry?;
            let entry: IndexEntryV1 = postcard::from_bytes(value.value())?;
            moved.push((key.value().to_owned(), IndexEntry::from(entry)));
        }
    }
    let mut index = transaction.open_table(INDEX)?;
    for (key, entry) in moved {
        let bytes = postcard::to_stdvec(&entry)?;
        index.insert(key.as_str(), bytes.as_slice())?;
    }
    drop(index);
    transaction.delete_table(INDEX_V1)?;
    Ok(())
}

/// A patch's key, which sorts patches in stamp order.
fn stamp_key(stamp: &Stamp) -> [u8; 40] {
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
        transaction.open_table(INDEX)?;
        transaction.open_table(KEPT)?;
        transaction.open_table(PLACED)?;
        transaction.open_table(APPLIED)?;
        move_index_v1(&transaction)?;
        transaction.commit()?;
        Ok(Self {
            database,
            revision: AtomicU64::new(0),
        })
    }

    /// How many writes this handle committed: a new value means the state
    /// may read differently.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// Runs `change` in one write transaction and commits it, a new
    /// revision.
    fn write(&self, change: impl FnOnce(&WriteTransaction) -> Result<()>) -> Result<()> {
        let transaction = self.database.begin_write()?;
        change(&transaction)?;
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// The value at `key` in `table`, decoded.
    fn value<T: DeserializeOwned>(
        &self,
        table: TableDefinition<'static, &'static str, &'static [u8]>,
        key: &str,
    ) -> Result<Option<T>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(table)?;
        let value = table.get(key)?;
        value.as_ref().map(decode).transpose()
    }

    /// Stores `value` at `key` in `table`.
    fn set_value<T: Serialize + ?Sized>(
        &self,
        table: TableDefinition<'static, &'static str, &'static [u8]>,
        key: &str,
        value: &T,
    ) -> Result<()> {
        let bytes = postcard::to_stdvec(value)?;
        self.write(|transaction| {
            transaction
                .open_table(table)?
                .insert(key, bytes.as_slice())?;
            Ok(())
        })
    }

    /// Records a patch; recording one twice changes nothing.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn add_patch(&self, patch: &SignedPatch) -> Result<()> {
        self.add_patches(std::slice::from_ref(patch))
    }

    /// Records patches in one transaction, all or none; recording one
    /// twice changes nothing.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn add_patches(&self, patches: &[SignedPatch]) -> Result<()> {
        let encoded = patches
            .iter()
            .map(|patch| Ok((stamp_key(&patch.stamp()), postcard::to_stdvec(patch)?)))
            .collect::<Result<Vec<_>>>()?;
        self.write(|transaction| {
            let mut table = transaction.open_table(PATCHES)?;
            for (key, bytes) in &encoded {
                table.insert(key.as_slice(), bytes.as_slice())?;
            }
            Ok(())
        })
    }

    /// Every recorded patch, in stamp order.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn patches(&self) -> Result<Vec<SignedPatch>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(PATCHES)?;
        table.iter()?.map(|entry| decode(&entry?.1)).collect()
    }

    /// The ledger folded from every recorded patch. A patch whose signature
    /// fails is dropped, as it would have been on receipt.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn ledger(&self, group: GroupId) -> Result<Ledger> {
        let mut ledger = Ledger::new(group);
        ledger.extend(self.patches()?);
        Ok(ledger)
    }

    /// The index entry of one path.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn index_entry(&self, key: &PathKey) -> Result<Option<IndexEntry>> {
        self.value(INDEX, key.as_str())
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
        let Some(folder) = under else {
            return table.iter()?.map(|entry| decode(&entry?.1)).collect();
        };
        let start = folder.key();
        let mut entries = Vec::new();
        for entry in table.range(start.as_str()..)? {
            let (key, value) = entry?;
            if !key.value().starts_with(start.as_str()) {
                break;
            }
            let entry: IndexEntry = decode(&value)?;
            if entry.path.is_within(folder) {
                entries.push(entry);
            }
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
        self.write(|transaction| {
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
            Ok(())
        })
    }

    /// Records that the disk keeps `kept` at `path`, a path relative to
    /// the root that no portable path may hold.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn keep(&self, path: &str, kept: &Kept) -> Result<()> {
        self.set_value(KEPT, path, kept)
    }

    /// Forgets what the disk keeps at `path`.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn unkeep(&self, path: &str) -> Result<()> {
        self.write(|transaction| {
            transaction.open_table(KEPT)?.remove(path)?;
            Ok(())
        })
    }

    /// What the disk keeps at `path` for a suggestion, if anything.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn kept_at(&self, path: &str) -> Result<Option<Kept>> {
        self.value(KEPT, path)
    }

    /// What the disk keeps for suggestions, by path relative to the root.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn kept(&self) -> Result<BTreeMap<String, Kept>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(KEPT)?;
        table
            .iter()?
            .map(|entry| {
                let (path, value) = entry?;
                Ok((path.value().to_owned(), decode(&value)?))
            })
            .collect()
    }

    /// The rules of the selection the disk was last brought to, if any was.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn applied_selection(&self) -> Result<Option<Vec<Rule>>> {
        self.value(APPLIED, SELECTION)
    }

    /// Records the rules of the selection the disk was brought to.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn set_applied_selection(&self, rules: &[Rule]) -> Result<()> {
        self.set_value(APPLIED, SELECTION, rules)
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
        self.write(|transaction| {
            transaction.delete_table(PLACED)?;
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
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
