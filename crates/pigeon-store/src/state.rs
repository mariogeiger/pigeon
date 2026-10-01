//! The state database of one group on one machine: every patch received,
//! the disk index, the set-aside list, and the machine's settings, in one
//! redb file whose transactions keep them consistent across crashes.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use pigeon_core::clock::Stamp;
use pigeon_core::identity::GroupId;
use pigeon_core::ledger::Ledger;
use pigeon_core::patch::SignedPatch;
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::places::{Place, Places};
use pigeon_core::retention::Retention;
use pigeon_core::selection::{Rule, Selection};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::aside::AsideItem;
use crate::error::{Result, StoreError};
use crate::index::IndexEntry;

const PATCHES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("patches");
const INDEX: TableDefinition<&str, &[u8]> = TableDefinition::new("index");
const ASIDE: TableDefinition<u64, &[u8]> = TableDefinition::new("aside");
const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");

const SELECTION: &str = "selection";
const RETENTION: &str = "retention";
const PLACES: &str = "places";
const PLACED: &str = "placed";

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
        transaction.open_table(SETTINGS)?;
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

    /// Removes an item from the set-aside list and returns it.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn take_aside(&self, id: u64) -> Result<Option<AsideItem>> {
        let transaction = self.database.begin_write()?;
        let item = {
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

    fn setting<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(SETTINGS)?;
        let value = table.get(name)?;
        value
            .map(|value| {
                serde_json::from_slice(value.value()).map_err(|source| StoreError::Json {
                    path: name.into(),
                    source,
                })
            })
            .transpose()
    }

    fn set_setting<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value).expect("a setting serializes");
        let transaction = self.database.begin_write()?;
        transaction
            .open_table(SETTINGS)?
            .insert(name, bytes.as_slice())?;
        transaction.commit()?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// The machine's selection, empty until first set.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read or holds an invalid rule.
    pub fn selection(&self) -> Result<Selection> {
        let rules: Vec<Rule> = self.setting(SELECTION)?.unwrap_or_default();
        Selection::new(rules).map_err(|error| StoreError::Invalid(error.to_string()))
    }

    /// Stores the machine's selection.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn set_selection(&self, selection: &Selection) -> Result<()> {
        let rules: Vec<&Rule> = selection.rules().collect();
        self.set_setting(SELECTION, &rules)
    }

    /// The machine's retention, the default until first set.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn retention(&self) -> Result<Retention> {
        Ok(self.setting(RETENTION)?.unwrap_or_default())
    }

    /// Stores the machine's retention.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn set_retention(&self, retention: &Retention) -> Result<()> {
        self.set_setting(RETENTION, retention)
    }

    /// The folders the machine wants at other destinations.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn places(&self) -> Result<Places> {
        let list: Vec<Place> = self.setting(PLACES)?.unwrap_or_default();
        Ok(Places::new(list))
    }

    /// Stores the folders the machine wants at other destinations.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn set_places(&self, places: &Places) -> Result<()> {
        self.set_setting(PLACES, &places.iter().collect::<Vec<_>>())
    }

    /// The folders the disk holds at other destinations, as last moved.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn placed(&self) -> Result<Places> {
        let list: Vec<Place> = self.setting(PLACED)?.unwrap_or_default();
        Ok(Places::new(list))
    }

    /// Records the folders the disk holds at other destinations.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn set_placed(&self, placed: &Places) -> Result<()> {
        self.set_setting(PLACED, &placed.iter().collect::<Vec<_>>())
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
