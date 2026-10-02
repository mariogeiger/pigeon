//! A group's secrets on one machine, `secrets.toml`, readable only by its
//! owner: the machine's secret key, made on first use, and the group key
//! once the machine joins.

use data_encoding::HEXLOWER;
use iroh_base::SecretKey;
use serde::{Deserialize, Serialize};

use crate::error::{Result, StoreError};
use crate::group_dirs::{GroupDirs, read_if_present, write_private};
use crate::group_key::GroupKey;

/// What `secrets.toml` starts with.
pub const HEADER: &str = "\
# The secrets of this machine in this group: whoever holds them speaks for
# it. pigeon writes this file; share it with nobody.
";

/// A group's secrets on one machine.
#[derive(Clone, Debug)]
pub struct Secrets {
    /// The key the machine signs and connects with.
    pub machine: SecretKey,
    /// The group key, none while the machine only hears the group.
    pub key: Option<GroupKey>,
}

/// The secrets as the file spells them.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spelled {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<GroupKey>,
    machine: String,
}

impl Spelled {
    fn of(secrets: &Secrets) -> Self {
        Self {
            key: secrets.key.clone(),
            machine: HEXLOWER.encode(&secrets.machine.to_bytes()),
        }
    }

    fn secrets(self) -> Result<Secrets, String> {
        let bytes = HEXLOWER
            .decode(self.machine.as_bytes())
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .ok_or("machine is not 64 lowercase hexadecimal digits")?;
        Ok(Secrets {
            machine: SecretKey::from_bytes(&bytes),
            key: self.key,
        })
    }
}

impl GroupDirs {
    /// The group's secrets on this machine, made with a new machine key on
    /// first use.
    ///
    /// # Errors
    ///
    /// Fails, naming the file, if it cannot be read or written, or is
    /// invalid.
    pub fn secrets(&self) -> Result<Secrets> {
        let path = self.secrets_path();
        let Some(text) = read_if_present(&path)? else {
            let secrets = Secrets {
                machine: SecretKey::generate(),
                key: None,
            };
            self.save_secrets(&secrets)?;
            return Ok(secrets);
        };
        toml::from_str::<Spelled>(&text)
            .map_err(|error| error.to_string())
            .and_then(Spelled::secrets)
            .map_err(|reason| StoreError::Invalid(format!("{}: {reason}", path.display())))
    }

    /// Writes the group's secrets on this machine, replacing any at once.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be written.
    ///
    /// # Panics
    ///
    /// Panics if the secrets do not serialize, which cannot happen.
    pub fn save_secrets(&self, secrets: &Secrets) -> Result<()> {
        let body = toml::to_string_pretty(&Spelled::of(secrets)).expect("secrets serialize");
        write_private(&self.secrets_path(), format!("{HEADER}\n{body}").as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use pigeon_core::name::MemberName;

    use super::*;

    #[test]
    fn the_machine_key_is_made_once_and_kept_private() {
        let dir = tempfile::tempdir().unwrap();
        let group = GroupDirs::new(dir.path().join("g"), dir.path().join("g"));
        let first = group.secrets().unwrap();
        assert!(first.key.is_none());
        assert_eq!(
            group.secrets().unwrap().machine.public(),
            first.machine.public()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(group.secrets_path()).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn secrets_read_back_exactly_as_written_and_nothing_else_reads() {
        let dir = tempfile::tempdir().unwrap();
        let group = GroupDirs::new(dir.path(), dir.path());
        let mut secrets = group.secrets().unwrap();
        let by = SecretKey::generate().public();
        secrets.key = Some(GroupKey::generate(
            MemberName::parse("g").unwrap(),
            vec![by],
        ));
        group.save_secrets(&secrets).unwrap();
        let text = std::fs::read_to_string(group.secrets_path()).unwrap();
        assert!(text.starts_with(HEADER), "{text}");
        let read = group.secrets().unwrap();
        assert_eq!(read.machine.to_bytes(), secrets.machine.to_bytes());
        assert_eq!(read.key, secrets.key);
        std::fs::write(
            group.secrets_path(),
            format!("{text}\n[cert]\nname = \"mario\"\n"),
        )
        .unwrap();
        let error = group.secrets().unwrap_err().to_string();
        assert!(
            error.contains("secrets.toml") && error.contains("cert"),
            "{error}"
        );
        std::fs::write(group.secrets_path(), "machine = \"00\"\n").unwrap();
        let error = group.secrets().unwrap_err().to_string();
        assert!(error.contains("hexadecimal"), "{error}");
    }
}
