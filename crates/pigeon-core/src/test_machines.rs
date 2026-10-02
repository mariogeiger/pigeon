//! Machines of a test group that sign patches, and the changes and
//! contents they write, for the tests of every crate.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test's names and paths are valid, or the test fails"
)]

use iroh_base::SecretKey;

use crate::clock::Stamp;
use crate::identity::{GroupId, GroupSecret, MachineCert, member_key};
use crate::ledger::{Ledger, Rejection};
use crate::name::MemberName;
use crate::patch::{Change, Content, ContentHash, Patch, SignedPatch, VersionRef};
use crate::path::{GroupPath, PathKey};
use crate::statement::member_path;

pub struct Machine {
    pub group: GroupId,
    pub cert: MachineCert,
    pub key: SecretKey,
}

#[must_use]
pub fn group() -> GroupId {
    GroupSecret([9; 32]).id()
}

#[must_use]
pub fn machine(name: &str, seed: u8) -> Machine {
    signer(name, SecretKey::from_bytes(&[seed; 32]))
}

/// The machine of key `key` that signs for the member `name`.
#[must_use]
pub fn signer(name: &str, key: SecretKey) -> Machine {
    let member = member_key(&group(), &MemberName::parse(name).unwrap());
    issued(name, &member, key)
}

/// The machine of seed `seed` that signs for the member `name` of key
/// `member`, which may not be the key the name derives.
#[must_use]
pub fn keyed_machine(name: &str, member: &SecretKey, seed: u8) -> Machine {
    issued(name, member, SecretKey::from_bytes(&[seed; 32]))
}

fn issued(name: &str, member: &SecretKey, key: SecretKey) -> Machine {
    let group = group();
    let cert = MachineCert::issue(
        &group,
        MemberName::parse(name).unwrap(),
        member,
        key.public(),
    );
    Machine { group, cert, key }
}

#[must_use]
pub fn content(byte: u8) -> Content {
    Content {
        hash: ContentHash([byte; 32]),
        size: 1,
        executable: false,
    }
}

#[must_use]
pub fn change(path: &str, byte: Option<u8>, replaces: Option<Stamp>) -> Change {
    Change {
        path: GroupPath::parse(path).unwrap(),
        content: byte.map(content),
        replaces,
        continues: None,
    }
}

/// The two changes that move the file at `from`, written at `stamp`, to
/// `to` with the content `byte`.
#[must_use]
pub fn moved(from: &str, stamp: Stamp, to: &str, byte: u8) -> Vec<Change> {
    vec![
        change(from, None, Some(stamp)),
        Change {
            continues: Some(VersionRef {
                path: GroupPath::parse(from).unwrap(),
                stamp,
            }),
            ..change(to, Some(byte), None)
        },
    ]
}

impl Machine {
    #[must_use]
    pub fn stamp(&self, time: u64) -> Stamp {
        Stamp {
            time,
            machine: self.key.public(),
        }
    }

    #[must_use]
    pub fn patch(&self, time: u64, changes: Vec<Change>) -> SignedPatch {
        let patch = Patch {
            stamp: self.stamp(time),
            changes,
        };
        SignedPatch::sign(&self.group, patch, self.cert.clone(), &self.key)
    }

    #[must_use]
    pub fn join(&self, time: u64) -> SignedPatch {
        let path = member_path(&self.cert.name);
        self.patch(time, vec![change(path.as_str(), Some(0), None)])
    }
}

#[must_use]
pub fn key(path: &str) -> PathKey {
    GroupPath::parse(path).unwrap().key()
}

#[must_use]
pub fn rejection(ledger: &Ledger, stamp: Stamp) -> Rejection {
    ledger.outcome(&stamp).unwrap().unwrap_err().clone()
}
