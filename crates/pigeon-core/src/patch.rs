//! Patches: sets of paths, each with its new content or nothing, composed by
//! override, and signed by the machine that made them.

use std::collections::BTreeMap;
use std::fmt;

use iroh_base::{SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::clock::Stamp;
use crate::identity::{GroupId, MachineCert};
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

/// One path of a patch: its new content, or `None` to delete it, and the
/// version it replaces as its author last saw it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Change {
    pub path: GroupPath,
    pub content: Option<Content>,
    pub replaces: Option<Stamp>,
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

/// Applies `later` after `earlier`: `later` wins wherever both touch a path,
/// and the result replaces what `earlier` replaced. Composition is
/// associative, so pruning may compose any run of consecutive patches.
#[must_use]
pub fn compose(earlier: &Changes, later: &Changes) -> Changes {
    let mut composed = earlier.clone();
    for (key, change) in later {
        let replaces = earlier
            .get(key)
            .map_or(change.replaces, |first| first.replaces);
        composed.insert(
            key.clone(),
            Change {
                replaces,
                ..change.clone()
            },
        );
    }
    composed
}

/// A patch as its machine made it, before signing.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Patch {
    pub stamp: Stamp,
    pub changes: Vec<Change>,
    /// The request this patch applies, which lets it change frozen files.
    pub applies: Option<GroupPath>,
}

/// A patch with the certificate of its machine and that machine's signature.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SignedPatch {
    pub patch: Patch,
    pub cert: MachineCert,
    pub signature: Signature,
}

fn signed_message(group: &GroupId, patch: &Patch) -> Vec<u8> {
    let mut message = b"pigeon patch ".to_vec();
    message.extend_from_slice(&group.0);
    message.extend(postcard::to_stdvec(patch).unwrap_or_default());
    message
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
    /// Signs `patch` with the machine key that `cert` vouches for.
    #[must_use]
    pub fn sign(group: &GroupId, patch: Patch, cert: MachineCert, machine: &SecretKey) -> Self {
        let signature = machine.sign(&signed_message(group, &patch));
        Self {
            patch,
            cert,
            signature,
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
        self.patch
            .stamp
            .machine
            .verify(&signed_message(group, &self.patch), &self.signature)
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

    fn content(byte: u8) -> Content {
        Content {
            hash: ContentHash([byte; 32]),
            size: u64::from(byte),
            executable: false,
        }
    }

    fn change(path: &str, byte: Option<u8>) -> Change {
        Change {
            path: GroupPath::parse(path).unwrap(),
            content: byte.map(content),
            replaces: None,
        }
    }

    #[test]
    fn later_wins_and_composition_is_associative() {
        let p = changes([change("a", Some(1)), change("b", Some(2))]);
        let q = changes([change("b", None), change("c", Some(3))]);
        let r = changes([change("a", Some(4)), change("c", None)]);
        let pq = compose(&p, &q);
        assert_eq!(pq[&GroupPath::parse("b").unwrap().key()].content, None);
        assert_eq!(compose(&pq, &r), compose(&p, &compose(&q, &r)));
    }
}
