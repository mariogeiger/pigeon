//! A group's data directory as pigeon wrote it before `config.toml`:
//! `config.json` held the key, the member and the root, `machine.key` the
//! machine's secret key, and a settings table of the state database the
//! selection, the retention in seconds, and the places wanted and placed.
//! Upgrading writes them in today's files, then removes the old ones.

use std::path::{Path, PathBuf};

use iroh_base::SecretKey;
use pigeon_core::identity::Renewal;
use pigeon_core::name::MemberName;
use pigeon_core::places::{Place, Places};
use pigeon_core::retention::{DAY, Retention};
use pigeon_core::selection::{Rule, Selection};
use redb::{ReadableDatabase, TableDefinition, TableError};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::config::{Config, ConfigFile};
use crate::data_dir::{DataDir, read_if_present};
use crate::error::{Result, StoreError};
use crate::group_key::GroupKey;
use crate::secrets::Secrets;
use crate::state::State;

const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");

/// What `config.json` held; its certificate now derives from the rest.
#[derive(Deserialize)]
struct OldConfig {
    key: GroupKey,
    #[serde(default)]
    renewal: Option<Renewal>,
    member: MemberName,
    root: PathBuf,
}

/// The retention as the settings table held it, in seconds.
#[derive(Deserialize)]
struct OldRetention {
    every: Option<u64>,
    daily: Option<u64>,
    weekly: Option<u64>,
    before_deletion: Option<u64>,
    quota_percent: Option<u8>,
    everything: Option<bool>,
}

impl From<OldRetention> for Retention {
    fn from(old: OldRetention) -> Self {
        let defaults = Self::default();
        let days = |seconds: Option<u64>, default| seconds.map_or(default, |seconds| seconds / DAY);
        Self {
            every: days(old.every, defaults.every),
            daily: days(old.daily, defaults.daily),
            weekly: days(old.weekly, defaults.weekly),
            before_deletion: days(old.before_deletion, defaults.before_deletion),
            quota_percent: old.quota_percent.unwrap_or(defaults.quota_percent),
            everything: old.everything.unwrap_or(defaults.everything),
        }
    }
}

/// The setting `name` of the settings table, if the table holds it.
fn setting<T: DeserializeOwned>(state: &State, name: &str) -> Result<Option<T>> {
    let transaction = state.database().begin_read()?;
    let table = match transaction.open_table(SETTINGS) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Some(value) = table.get(name)? else {
        return Ok(None);
    };
    serde_json::from_slice(value.value())
        .map(Some)
        .map_err(|source| StoreError::Json {
            path: PathBuf::from(format!("the setting {name}")),
            source,
        })
}

fn remove(path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(StoreError::io(path))
}

/// Upgrades `data` if an older pigeon wrote it, and says whether it did.
/// The old configuration goes last but for leftovers, so that a crash
/// before leaves it to upgrade again.
///
/// # Errors
///
/// Fails, naming what is wrong, if an old file cannot be read or a new one
/// written.
pub fn upgrade(data: &DataDir) -> Result<bool> {
    let config_path = data.path().join("config.json");
    let Some(text) = read_if_present(&config_path)? else {
        return Ok(false);
    };
    let old: OldConfig = serde_json::from_str(&text).map_err(|source| StoreError::Json {
        path: config_path.clone(),
        source,
    })?;
    let key_path = data.path().join("machine.key");
    let bytes = std::fs::read(&key_path).map_err(StoreError::io(&key_path))?;
    let machine: [u8; 32] = bytes.try_into().map_err(|_| {
        StoreError::Invalid(format!("{} does not hold 32 bytes", key_path.display()))
    })?;
    let state = State::open(&data.state_path())?;
    let rules: Vec<Rule> = setting(&state, "selection")?.unwrap_or_default();
    let selection =
        Selection::exactly(rules).map_err(|error| StoreError::Invalid(error.to_string()))?;
    let retention = setting::<OldRetention>(&state, "retention")?
        .map_or_else(Retention::default, Retention::from);
    let wanted: Vec<Place> = setting(&state, "places")?.unwrap_or_default();
    let laid_out: Vec<Place> = setting(&state, "placed")?.unwrap_or_default();
    let config = Config {
        member: old.member,
        root: old.root,
        selection,
        retention,
        places: Places::new(wanted),
    };
    ConfigFile::create(data, config)?;
    data.save_secrets(&Secrets {
        machine: SecretKey::from_bytes(&machine),
        key: Some(old.key),
        renewal: old.renewal,
    })?;
    state.set_placed(&Places::new(laid_out))?;
    remove(&config_path)?;
    remove(&key_path)?;
    let transaction = state.database().begin_write()?;
    transaction.delete_table(SETTINGS)?;
    transaction.commit()?;
    Ok(true)
}

#[cfg(test)]
#[path = "legacy_tests.rs"]
mod tests;
