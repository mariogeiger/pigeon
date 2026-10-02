//! Patches: sets of paths, each with its new content or nothing and the
//! version it continues when it moved, signed by the machine that made
//! them in one of the formats pigeon has signed in.

use std::collections::BTreeMap;
use std::fmt;

use iroh_base::{SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::clock::Stamp;
use crate::identity::{GroupId, MachineCert};
use crate::patch_v1;
use crate::path::{GroupPath, PathKey};

/// A BLAKE3 hash of a file's bytes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentHash(pub [u8; 32]);

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self.to_string()[..12])
    }
}

/// What a file holds: its bytes, by hash, and its executable bit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Content {
    pub hash: ContentHash,
    pub size: u64,
    pub executable: bool,
}

/// One version of one path: the path and the stamp of the patch that
/// wrote it, which at most one change of that patch touches.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct VersionRef {
    pub path: GroupPath,
    pub stamp: Stamp,
}

/// A version as text: its path, `@`, its stamp's time in hex, `-`, and its
/// machine's key in hex.
impl fmt::Display for VersionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}@{:016x}-{}",
            self.path, self.stamp.time, self.stamp.machine
        )
    }
}

impl std::str::FromStr for VersionRef {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let wrong = || format!("{text:?} names no version: it reads <path>@<time>-<machine>");
        let (path, stamp) = text.rsplit_once('@').ok_or_else(wrong)?;
        let (time, machine) = stamp.split_once('-').ok_or_else(wrong)?;
        Ok(Self {
            path: GroupPath::parse(path).map_err(|error| format!("{text:?}: {error}"))?,
            stamp: Stamp {
                time: u64::from_str_radix(time, 16).map_err(|_| wrong())?,
                machine: machine.parse().map_err(|_| wrong())?,
            },
        })
    }
}

/// One path of a patch: its new content, or `None` to delete it, the
/// version it replaces as its author last saw it, and, when the file
/// moved here, the version it continues, whose history becomes its own.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Change {
    pub path: GroupPath,
    pub content: Option<Content>,
    pub replaces: Option<Stamp>,
    pub continues: Option<VersionRef>,
}

/// A set of changes with at most one per path key, as one atomic step.
pub type Changes = BTreeMap<PathKey, Change>;

/// Collects changes by key, keeping the last change of each key.
#[must_use]
pub fn changes(list: impl IntoIterator<Item = Change>) -> Changes {
    list.into_iter()
        .map(|change| (change.path.key(), change))
        .collect()
}

/// A patch as its machine made it, before signing.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Patch {
    pub stamp: Stamp,
    pub changes: Vec<Change>,
}

/// The format a patch was signed in: the first, of pigeon until 0.6, whose
/// patches named the request they applied and continued no version, or
/// the second.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Format {
    V1 { applies: Option<GroupPath> },
    V2,
}

/// A patch with the certificate of its machine, that machine's signature,
/// and the format the signature covers.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SignedPatch {
    pub patch: Patch,
    pub cert: MachineCert,
    pub signature: Signature,
    pub signed_as: Format,
}

/// The bytes a machine signs for `patch` in `format`, or `None` when the
/// format cannot express the patch.
fn signed_message(group: &GroupId, patch: &Patch, format: &Format) -> Option<Vec<u8>> {
    match format {
        Format::V1 { applies } => patch_v1::signed_message(group, patch, applies.clone()),
        Format::V2 => {
            let mut message = b"pigeon patch 2 ".to_vec();
            message.extend_from_slice(&group.0);
            message.extend(postcard::to_stdvec(patch).ok()?);
            Some(message)
        }
    }
}

/// Why a signed patch is not what it claims to be.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureError {
    #[error("the machine certificate is not signed by its member key")]
    Cert,
    #[error("the patch is stamped by another machine than its certificate names")]
    Machine,
    #[error("the patch is not signed by its machine")]
    Patch,
    #[error("the patch touches {0} twice")]
    Duplicate(GroupPath),
}

impl SignedPatch {
    /// Signs `patch` in the current format with the machine key that
    /// `cert` vouches for.
    ///
    /// # Panics
    /// Never: the current format expresses every patch.
    #[must_use]
    pub fn sign(group: &GroupId, patch: Patch, cert: MachineCert, machine: &SecretKey) -> Self {
        let message = signed_message(group, &patch, &Format::V2)
            .expect("the current format expresses every patch");
        Self {
            signature: machine.sign(&message),
            patch,
            cert,
            signed_as: Format::V2,
        }
    }

    /// Checks every signature and that no path appears twice.
    ///
    /// # Errors
    /// Returns the first check that fails.
    pub fn verify(&self, group: &GroupId) -> Result<(), SignatureError> {
        if !self.cert.is_valid(group) {
            return Err(SignatureError::Cert);
        }
        if self.cert.machine != self.patch.stamp.machine {
            return Err(SignatureError::Machine);
        }
        let message =
            signed_message(group, &self.patch, &self.signed_as).ok_or(SignatureError::Patch)?;
        self.patch
            .stamp
            .machine
            .verify(&message, &self.signature)
            .map_err(|_| SignatureError::Patch)?;
        let mut keys = std::collections::BTreeSet::new();
        for change in &self.patch.changes {
            if !keys.insert(change.path.key()) {
                return Err(SignatureError::Duplicate(change.path.clone()));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn stamp(&self) -> Stamp {
        self.patch.stamp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{GroupSecret, member_key};
    use crate::name::MemberName;

    fn content(byte: u8) -> Content {
        Content {
            hash: ContentHash([byte; 32]),
            size: u64::from(byte),
            executable: false,
        }
    }

    fn signed(continues: Option<VersionRef>) -> (GroupId, SignedPatch, SecretKey) {
        let group = GroupSecret([5; 32]).id();
        let name = MemberName::parse("alice").unwrap();
        let machine = SecretKey::from_bytes(&[3; 32]);
        let cert = MachineCert::issue(
            &group,
            name.clone(),
            &member_key(&group, &name),
            machine.public(),
        );
        let stamp = Stamp {
            time: 9,
            machine: machine.public(),
        };
        let patch = Patch {
            stamp,
            changes: vec![Change {
                path: GroupPath::parse("b").unwrap(),
                content: Some(content(1)),
                replaces: None,
                continues,
            }],
        };
        let signed = SignedPatch::sign(&group, patch, cert, &machine);
        (group, signed, machine)
    }

    #[test]
    fn a_signature_covers_the_version_a_change_continues() {
        let from = VersionRef {
            path: GroupPath::parse("a").unwrap(),
            stamp: Stamp {
                time: 1,
                machine: SecretKey::from_bytes(&[4; 32]).public(),
            },
        };
        let (group, mut signed, _) = signed(Some(from));
        assert_eq!(signed.verify(&group), Ok(()));
        signed.patch.changes[0].continues = None;
        assert_eq!(signed.verify(&group), Err(SignatureError::Patch));
    }

    #[test]
    fn a_first_format_patch_verifies_and_cannot_continue_a_version() {
        let (group, signed, machine) = signed(None);
        let applies = Some(GroupPath::parse(".pigeon/requests/x.json").unwrap());
        let message = patch_v1::signed_message(&group, &signed.patch, applies.clone()).unwrap();
        let mut first = SignedPatch {
            signature: machine.sign(&message),
            signed_as: Format::V1 { applies },
            ..signed
        };
        assert_eq!(first.verify(&group), Ok(()));
        first.signed_as = Format::V1 { applies: None };
        assert_eq!(first.verify(&group), Err(SignatureError::Patch));
        first.patch.changes[0].continues = Some(VersionRef {
            path: GroupPath::parse("a").unwrap(),
            stamp: first.patch.stamp,
        });
        assert_eq!(patch_v1::signed_message(&group, &first.patch, None), None);
    }

    #[test]
    fn a_version_reads_back_from_its_text() {
        let stamp = Stamp {
            time: 0x1234_5678_9abc_def0,
            machine: SecretKey::from_bytes(&[3; 32]).public(),
        };
        let version = VersionRef {
            path: GroupPath::parse("docs/a@b - c.txt").unwrap(),
            stamp,
        };
        let text = version.to_string();
        assert!(text.starts_with("docs/a@b - c.txt@123456789abcdef0-"));
        assert_eq!(text.parse::<VersionRef>(), Ok(version));
        for wrong in ["docs/a.txt", "a@xyz-m", "a@12", "a?b@12-00", "a@12-zz"] {
            assert!(wrong.parse::<VersionRef>().is_err(), "{wrong}");
        }
    }
}
