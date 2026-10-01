//! The state database of one group on one machine: every patch received,
//! the disk index, the set-aside list and the files that show its items to
//! the group, the folders the disk holds at other destinations, and the
//! selection the disk was last brought to, in one redb file whose
//! transactions keep them consistent across crashes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use pigeon_core::clock::Stamp;
use pigeon_core::identity::GroupId;
use pigeon_core::ledger::Ledger;
use pigeon_core::patch::SignedPatch;
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::places::{Place, Places};
use pigeon_core::selection::Rule;
use pigeon_core::statement::AsideItem;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use crate::error::{Result, StoreError};
use crate::index::IndexEntry;

const PATCHES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("patches");
const INDEX: TableDefinition<&str, &[u8]> = TableDefinition::new("index");
const ASIDE: TableDefinition<u64, &[u8]> = TableDefinition::new("aside");
const ASIDE_FILES: TableDefinition<u64, &str> = TableDefinition::new("aside files");
const PLACED: TableDefinition<&str, &str> = TableDefinition::new("placed");
const APPLIED: TableDefinition<&str, &[u8]> = TableDefinition::new("applied");
const SELECTION: &str = "selection";

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
        transaction.open_table(ASIDE)?;
        transaction.open_table(ASIDE_FILES)?;
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

    /// Adds an item to the set-aside list and returns its number.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn set_aside(&self, item: &AsideItem) -> Result<u64> {
        let bytes = postcard::to_stdvec(item)?;
        let transaction = self.database.begin_write()?;
        let id = {
            let mut table = transaction.open_table(ASIDE)?;
            let id = table.last()?.map_or(1, |(key, _)| key.value() + 1);
            table.insert(id, bytes.as_slice())?;
            id
        };
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    /// The set-aside list, oldest first.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn aside(&self) -> Result<Vec<(u64, AsideItem)>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(ASIDE)?;
        let mut items = Vec::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            items.push((key.value(), postcard::from_bytes(value.value())?));
        }
        Ok(items)
    }

    /// Records that the file at `file` shows the group item `id`.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn set_aside_file(&self, id: u64, file: &GroupPath) -> Result<()> {
        let transaction = self.database.begin_write()?;
        transaction
            .open_table(ASIDE_FILES)?
            .insert(id, file.as_str())?;
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// The files that show the group the set-aside items, by item.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read or names an invalid file.
    pub fn aside_files(&self) -> Result<BTreeMap<u64, GroupPath>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(ASIDE_FILES)?;
        let mut files = BTreeMap::new();
        for entry in table.iter()? {
            let (id, file) = entry?;
            let file = GroupPath::parse(file.value()).map_err(|error| {
                StoreError::Invalid(format!(
                    "the state database holds a set-aside file that is no path: {error}"
                ))
            })?;
            files.insert(id.value(), file);
        }
        Ok(files)
    }

    /// Removes an item from the set-aside list, with the record of its
    /// file, and returns it.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn take_aside(&self, id: u64) -> Result<Option<AsideItem>> {
        let transaction = self.database.begin_write()?;
        let item = {
            transaction.open_table(ASIDE_FILES)?.remove(id)?;
            let mut table = transaction.open_table(ASIDE)?;
            let removed = table.remove(id)?;
            removed
                .map(|value| postcard::from_bytes(value.value()))
                .transpose()?
        };
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(item)
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
