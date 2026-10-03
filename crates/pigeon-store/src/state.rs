//! The state database of one group on one machine: every patch received,
//! the disk index, the suggestions whose content the disk keeps, the
//! folders the disk holds at other destinations, and the selection the
//! disk was last brought to, in one redb file whose transactions keep them
//! consistent across crashes. Each write waits for the disk to keep it,
//! save a refresh of what the index already says, whose loss only makes
//! pigeon read the files again; recording a value already held, or
//! forgetting what nothing keeps, writes nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pigeon_core::clock::Stamp;
use pigeon_core::identity::GroupId;
use pigeon_core::ledger::Ledger;
use pigeon_core::patch::{Content, SignedPatch};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::places::{Place, Places};
use pigeon_core::selection::Rule;
use redb::{
    AccessGuard, Database, Durability, ReadableDatabase, ReadableTable, TableDefinition,
    WriteTransaction,
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
/// The changes of this machine whose loss was noted, by stamp and path
/// key; created when the first losses are noted.
const LOSSES: TableDefinition<&[u8], ()> = TableDefinition::new("noted losses");
const SELECTION: &str = "selection";
const ROOT: &str = "root";

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

/// The key of the loss of the change `stamp` made at `key`.
fn loss_key(stamp: &Stamp, key: &PathKey) -> Vec<u8> {
    let mut bytes = stamp_key(stamp).to_vec();
    bytes.extend_from_slice(key.as_str().as_bytes());
    bytes
}

/// The state database.
pub struct State {
    database: Database,
    revision: AtomicU64,
    syncs: AtomicU64,
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
            syncs: AtomicU64::new(0),
        })
    }

    /// How many writes this handle committed: a new value means the state
    /// may read differently.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// How many writes this handle waited for the disk to keep, each one
    /// a sync of the file.
    #[must_use]
    pub fn syncs(&self) -> u64 {
        self.syncs.load(Ordering::Relaxed)
    }

    /// Runs `change` in one write transaction and commits it, a new
    /// revision, once the disk keeps it.
    fn write(&self, change: impl FnOnce(&WriteTransaction) -> Result<()>) -> Result<()> {
        self.commit(Durability::Immediate, |transaction| {
            change(transaction).map(|()| true)
        })
    }

    /// Runs `change` in one write transaction, committed with
    /// `durability` as a new revision when `change` says it wrote
    /// something, and dropped otherwise.
    fn commit(
        &self,
        durability: Durability,
        change: impl FnOnce(&WriteTransaction) -> Result<bool>,
    ) -> Result<()> {
        let mut transaction = self.database.begin_write()?;
        transaction
            .set_durability(durability)
            .map_err(redb::Error::from)?;
        if !change(&transaction)? {
            transaction.abort()?;
            return Ok(());
        }
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        if matches!(durability, Durability::Immediate) {
            self.syncs.fetch_add(1, Ordering::Relaxed);
        }
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

    /// Stores `value` at `key` in `table`, writing nothing when `key`
    /// holds it already.
    fn set_value<T: Serialize + ?Sized>(
        &self,
        table: TableDefinition<'static, &'static str, &'static [u8]>,
        key: &str,
        value: &T,
    ) -> Result<()> {
        let bytes = postcard::to_stdvec(value)?;
        self.commit(Durability::Immediate, |transaction| {
            let mut table = transaction.open_table(table)?;
            if table
                .get(key)?
                .is_some_and(|held| held.value() == bytes.as_slice())
            {
                return Ok(false);
            }
            table.insert(key, bytes.as_slice())?;
            Ok(true)
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
        self.write(|transaction| put_index(transaction, updates))
    }

    /// Replaces index entries as [`State::update_index`] does, without
    /// waiting for the disk to keep them, so that a crash may lose them:
    /// for what the disk holds as the index already says, whose loss costs
    /// only reading the files again.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn refresh_index<'a>(
        &self,
        updates: impl IntoIterator<Item = (&'a PathKey, Option<&'a IndexEntry>)>,
    ) -> Result<()> {
        self.commit(Durability::None, |transaction| {
            put_index(transaction, updates).map(|()| true)
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

    /// Forgets what the disk keeps at `path`, writing nothing where it keeps
    /// nothing.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn unkeep(&self, path: &str) -> Result<()> {
        self.commit(Durability::Immediate, |transaction| {
            Ok(transaction.open_table(KEPT)?.remove(path)?.is_some())
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

    /// The root the disk was last brought to, if one was recorded.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn applied_root(&self) -> Result<Option<PathBuf>> {
        Ok(self.value::<String>(APPLIED, ROOT)?.map(PathBuf::from))
    }

    /// Records the root the disk is brought to.
    ///
    /// # Errors
    ///
    /// Fails if the root is not valid Unicode or the database cannot be
    /// written.
    pub fn set_applied_root(&self, root: &Path) -> Result<()> {
        let text = root.to_str().ok_or_else(|| {
            StoreError::Invalid(format!("{} is not valid Unicode", root.display()))
        })?;
        self.set_value(APPLIED, ROOT, text)
    }

    /// Whether the index holds no path, as before anything was synced.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn index_is_empty(&self) -> Result<bool> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(INDEX)?;
        Ok(table.iter()?.next().is_none())
    }

    /// Forgets all the state records of the disk, as a fresh start in
    /// another root does: the index, the kept suggestions, the placed
    /// folders and what the disk was brought to.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn forget_disk(&self) -> Result<()> {
        self.write(|transaction| {
            for table in [INDEX, KEPT, APPLIED] {
                transaction.delete_table(table)?;
                transaction.open_table(table)?;
            }
            transaction.delete_table(PLACED)?;
            transaction.open_table(PLACED)?;
            Ok(())
        })
    }

    /// Whether this state notes the losses of its machine's changes, as it
    /// does from the first start of an engine of this pigeon on, and never
    /// did under an older one.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn notes_losses(&self) -> Result<bool> {
        let transaction = self.database.begin_read()?;
        match transaction.open_table(LOSSES) {
            Ok(_) => Ok(true),
            Err(redb::TableError::TableDoesNotExist(_)) => Ok(false),
            Err(error) => Err(redb::Error::from(error).into()),
        }
    }

    /// Whether the loss of the change the patch `stamp` made at `key` was
    /// noted.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn loss_noted(&self, stamp: &Stamp, key: &PathKey) -> Result<bool> {
        let transaction = self.database.begin_read()?;
        let table = match transaction.open_table(LOSSES) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(false),
            Err(error) => return Err(redb::Error::from(error).into()),
        };
        Ok(table.get(loss_key(stamp, key).as_slice())?.is_some())
    }

    /// Notes, in one transaction, the loss of the change each patch made at
    /// its key.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn note_losses<'a>(
        &self,
        losses: impl IntoIterator<Item = (&'a Stamp, &'a PathKey)>,
    ) -> Result<()> {
        self.write(|transaction| {
            let mut table = transaction.open_table(LOSSES)?;
            for (stamp, key) in losses {
                table.insert(loss_key(stamp, key).as_slice(), ())?;
            }
            Ok(())
        })
    }
}

/// Sets or removes each index entry of `updates` in `transaction`.
fn put_index<'a>(
    transaction: &WriteTransaction,
    updates: impl IntoIterator<Item = (&'a PathKey, Option<&'a IndexEntry>)>,
) -> Result<()> {
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
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
