//! A group's secrets on one machine, `secrets.toml`, readable only by its
//! owner: the machine's secret key, made on first use, once the machine
//! joins the group key and the renewal that made its secret, and the
//! certificate of a member whose key does not derive from their name.
//! Times read as RFC 3339, which holds them exactly.

use data_encoding::HEXLOWER;
use iroh_base::{PublicKey, SecretKey, Signature};
use pigeon_core::clock::{MachineId, Stamp, parse_rfc3339, rfc3339};
use pigeon_core::identity::{GroupId, MachineCert, Renewal, RenewedSecret};
use pigeon_core::name::MemberName;
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
    /// The renewal that made the key's secret, none for the first secret.
    pub renewal: Option<Renewal>,
    /// The certificate of this machine by a member whose key does not
    /// derive from their name, as for members of groups founded before
    /// keys did; none when the name gives the key.
    pub cert: Option<MachineCert>,
}

impl Secrets {
    /// The certificate by which `member` vouches for this machine in
    /// `group`: the one kept for them, or the one their name derives.
    ///
    /// # Errors
    ///
    /// Returns why the certificate kept for them is invalid.
    pub fn cert_of(&self, group: &GroupId, member: &MemberName) -> Result<MachineCert, String> {
        match self.cert.as_ref().filter(|cert| cert.name == *member) {
            Some(cert) if cert.is_valid(group) => Ok(cert.clone()),
            Some(_) => Err(format!(
                "the certificate of {member} does not vouch for this machine in this group"
            )),
            None => Ok(MachineCert::derive(
                group,
                member.clone(),
                self.machine.public(),
            )),
        }
    }

    /// The group secret this machine holds and its renewal.
    #[must_use]
    pub fn secret(&self) -> Option<RenewedSecret> {
        self.key.as_ref().map(|key| RenewedSecret {
            secret: key.secret.clone(),
            renewal: self.renewal,
        })
    }

    /// Adopts `secret` as the group's current secret.
    pub fn renew(&mut self, secret: RenewedSecret) {
        if let Some(key) = &mut self.key {
            key.secret = secret.secret;
            self.renewal = secret.renewal;
        }
    }
}

/// The secrets as the file spells them.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spelled {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<GroupKey>,
    machine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renewal: Option<SpelledRenewal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cert: Option<SpelledCert>,
}

/// A certificate of this machine, without the machine.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpelledCert {
    name: MemberName,
    member: PublicKey,
    signature: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpelledRenewal {
    by: MachineId,
    after: SpelledStamp,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpelledStamp {
    time: String,
    machine: MachineId,
}

impl Spelled {
    fn of(secrets: &Secrets) -> Self {
        Self {
            key: secrets.key.clone(),
            machine: HEXLOWER.encode(&secrets.machine.to_bytes()),
            renewal: secrets.renewal.map(|renewal| SpelledRenewal {
                by: renewal.by,
                after: SpelledStamp {
                    time: rfc3339(renewal.after.time),
                    machine: renewal.after.machine,
                },
            }),
            cert: secrets.cert.as_ref().map(|cert| SpelledCert {
                name: cert.name.clone(),
                member: cert.member,
                signature: HEXLOWER.encode(&cert.signature.to_bytes()),
            }),
        }
    }

    fn secrets(self) -> Result<Secrets, String> {
        let bytes = HEXLOWER
            .decode(self.machine.as_bytes())
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .ok_or("machine is not 64 lowercase hexadecimal digits")?;
        let renewal = match self.renewal {
            Some(renewal) => Some(Renewal {
                after: Stamp {
                    time: parse_rfc3339(&renewal.after.time)?,
                    machine: renewal.after.machine,
                },
                by: renewal.by,
            }),
            None => None,
        };
        let machine = SecretKey::from_bytes(&bytes);
        let cert = match self.cert {
            Some(cert) => {
                let signature = HEXLOWER
                    .decode(cert.signature.as_bytes())
                    .ok()
                    .and_then(|bytes| <[u8; 64]>::try_from(bytes).ok())
                    .ok_or("the certificate's signature is not 128 lowercase hexadecimal digits")?;
                Some(MachineCert {
                    name: cert.name,
                    member: cert.member,
                    machine: machine.public(),
                    signature: Signature::from_bytes(&signature),
                })
            }
            None => None,
        };
        Ok(Secrets {
            machine,
            key: self.key,
            renewal,
            cert,
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
                renewal: None,
                cert: None,
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
        assert!(first.key.is_none() && first.secret().is_none());
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
    fn secrets_read_back_exactly_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let group = GroupDirs::new(dir.path(), dir.path());
        let mut secrets = group.secrets().unwrap();
        let by = SecretKey::generate().public();
        secrets.key = Some(GroupKey::generate(
            MemberName::parse("g").unwrap(),
            vec![by],
        ));
        let first = secrets.secret().unwrap();
        let renewed = RenewedSecret {
            secret: crate::group_key::random_secret(),
            renewal: Some(Renewal {
                after: Stamp {
                    time: (1_790_856_000 << 32) + 3,
                    machine: secrets.machine.public(),
                },
                by,
            }),
        };
        secrets.renew(renewed.clone());
        assert_ne!(secrets.secret(), Some(first));
        group.save_secrets(&secrets).unwrap();
        let text = std::fs::read_to_string(group.secrets_path()).unwrap();
        assert!(
            text.starts_with(HEADER) && text.contains("2026-10-01T12:00:00."),
            "{text}"
        );
        let read = group.secrets().unwrap();
        assert_eq!(read.machine.to_bytes(), secrets.machine.to_bytes());
        assert_eq!(read.key, secrets.key);
        assert_eq!(read.secret(), Some(renewed));
        let id = secrets.key.as_ref().unwrap().group;
        let mario = MemberName::parse("mario").unwrap();
        let random = SecretKey::generate();
        let cert = MachineCert::issue(&id, mario.clone(), &random, read.machine.public());
        let kept = Secrets {
            cert: Some(cert.clone()),
            ..read
        };
        group.save_secrets(&kept).unwrap();
        let read = group.secrets().unwrap();
        assert_eq!(read.cert, Some(cert.clone()));
        assert_eq!(read.cert_of(&id, &mario), Ok(cert));
        let other = GroupKey::generate(MemberName::parse("h").unwrap(), Vec::new()).group;
        assert!(
            read.cert_of(&other, &mario)
                .unwrap_err()
                .contains("does not vouch")
        );
        std::fs::write(group.secrets_path(), "machine = \"00\"\n").unwrap();
        let error = group.secrets().unwrap_err().to_string();
        assert!(error.contains("hexadecimal"), "{error}");
    }
}
