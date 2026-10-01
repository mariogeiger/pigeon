//! A group's data directory: its configuration, the machine's secret key,
//! and where the state database and blob store live.

use std::fs;
use std::path::{Path, PathBuf};

use iroh_base::SecretKey;
use pigeon_core::identity::{MachineCert, member_key};
use pigeon_core::name::MemberName;
use serde::{Deserialize, Serialize};

use crate::error::{Result, StoreError};
use crate::group_key::GroupKey;

/// What a machine knows about a group it joined. The password is never
/// stored: it served once to sign the machine's certificate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupConfig {
    pub key: GroupKey,
    pub member: MemberName,
    pub root: PathBuf,
    pub cert: MachineCert,
}

impl GroupConfig {
    /// Joins `member` to the group from this machine: the name and password
    /// derive the member key, which vouches for the machine key.
    #[must_use]
    pub fn join(
        key: GroupKey,
        member: MemberName,
        password: &str,
        root: PathBuf,
        machine: &SecretKey,
    ) -> Self {
        let group = key.secret.id();
        let member_secret = member_key(&group, &member, password);
        let cert = MachineCert::issue(&group, member.clone(), &member_secret, machine.public());
        Self {
            key,
            member,
            root,
            cert,
        }
    }
}

/// The directory holding one group's local state.
#[derive(Clone, Debug)]
pub struct DataDir {
    path: PathBuf,
}

impl DataDir {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.path.join("config.json")
    }

    #[must_use]
    pub fn machine_key_path(&self) -> PathBuf {
        self.path.join("machine.key")
    }

    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        self.path.join("state.redb")
    }

    #[must_use]
    pub fn blobs_path(&self) -> PathBuf {
        self.path.join("blobs")
    }

    /// Reads the group configuration.
    ///
    /// # Errors
    ///
    /// Fails if the file is missing or malformed.
    pub fn load_config(&self) -> Result<GroupConfig> {
        let path = self.config_path();
        let text = fs::read(&path).map_err(StoreError::io(&path))?;
        serde_json::from_slice(&text).map_err(|source| StoreError::Json { path, source })
    }

    /// Writes the group configuration, replacing any previous one at once.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be written.
    ///
    /// # Panics
    ///
    /// Panics if the configuration does not serialize, which cannot happen.
    pub fn save_config(&self, config: &GroupConfig) -> Result<()> {
        let text = serde_json::to_vec_pretty(config).expect("a configuration serializes");
        write_private(&self.config_path(), &text)
    }

    /// The machine's secret key, created on first use.
    ///
    /// # Errors
    ///
    /// Fails if the key file cannot be read or created, or is malformed.
    pub fn machine_key(&self) -> Result<SecretKey> {
        let path = self.machine_key_path();
        match fs::read(&path) {
            Ok(bytes) => {
                let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
                    StoreError::Invalid(format!("{} does not hold 32 bytes", path.display()))
                })?;
                Ok(SecretKey::from_bytes(&bytes))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let key = SecretKey::generate();
                write_private(&path, &key.to_bytes())?;
                Ok(key)
            }
            Err(error) => Err(StoreError::io(path)(error)),
        }
    }
}

/// Writes a file readable only by its owner, through a temporary file so
/// that a crash never leaves it half written.
///
/// # Errors
///
/// Fails if the file cannot be written.
///
/// # Panics
///
/// Panics if `path` names no file in a folder.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().expect("a data file has a parent");
    fs::create_dir_all(parent).map_err(StoreError::io(parent))?;
    let temporary = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(&temporary)
        .map_err(StoreError::io(&temporary))?;
    std::io::Write::write_all(&mut file, bytes).map_err(StoreError::io(&temporary))?;
    file.sync_all().map_err(StoreError::io(&temporary))?;
    fs::rename(&temporary, path).map_err(StoreError::io(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_key_is_created_once() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::new(dir.path().join("g"));
        let first = data.machine_key().unwrap();
        assert_eq!(data.machine_key().unwrap().public(), first.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(data.machine_key_path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn config_round_trips_and_its_cert_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::new(dir.path());
        let machine = data.machine_key().unwrap();
        let key = GroupKey::generate(MemberName::parse("cheapmo").unwrap(), Vec::new());
        let member = MemberName::parse("mario").unwrap();
        let config = GroupConfig::join(key, member, "pw", "/cheapmo".into(), &machine);
        data.save_config(&config).unwrap();
        let loaded = data.load_config().unwrap();
        assert_eq!(loaded, config);
        assert!(loaded.cert.is_valid(&loaded.key.secret.id()));
        assert_eq!(loaded.cert.machine, machine.public());
    }

    #[test]
    fn same_name_and_password_give_the_same_member_on_two_machines() {
        let key = GroupKey::generate(MemberName::parse("g").unwrap(), Vec::new());
        let member = MemberName::parse("mario").unwrap();
        let a = GroupConfig::join(
            key.clone(),
            member.clone(),
            "pw",
            "/g".into(),
            &SecretKey::generate(),
        );
        let b = GroupConfig::join(
            key.clone(),
            member.clone(),
            "pw",
            "/g".into(),
            &SecretKey::generate(),
        );
        let c = GroupConfig::join(key, member, "other", "/g".into(), &SecretKey::generate());
        assert_eq!(a.cert.member, b.cert.member);
        assert_ne!(a.cert.member, c.cert.member);
    }
}
