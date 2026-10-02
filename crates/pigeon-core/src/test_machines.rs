//! Machines of a test group that sign patches for the model's tests, and
//! the changes and contents they write.

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

pub fn group() -> GroupId {
    GroupSecret([9; 32]).id()
}

pub fn machine(name: &str, seed: u8) -> Machine {
    let member = member_key(&group(), &MemberName::parse(name).unwrap());
    keyed_machine(name, &member, seed)
}

pub fn keyed_machine(name: &str, member: &SecretKey, seed: u8) -> Machine {
    let group = group();
    let key = SecretKey::from_bytes(&[seed; 32]);
    let cert = MachineCert::issue(
        &group,
        MemberName::parse(name).unwrap(),
        member,
        key.public(),
    );
    Machine { group, cert, key }
}

pub fn content(byte: u8) -> Content {
    Content {
        hash: ContentHash([byte; 32]),
        size: 1,
        executable: false,
    }
}

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
    pub fn stamp(&self, time: u64) -> Stamp {
        Stamp {
            time,
            machine: self.key.public(),
        }
    }

    pub fn patch(&self, time: u64, changes: Vec<Change>) -> SignedPatch {
        let patch = Patch {
            stamp: self.stamp(time),
            changes,
        };
        SignedPatch::sign(&self.group, patch, self.cert.clone(), &self.key)
    }

    pub fn join(&self, time: u64) -> SignedPatch {
        let path = member_path(&self.cert.name);
        self.patch(time, vec![change(path.as_str(), Some(0), None)])
    }
}

pub fn key(path: &str) -> PathKey {
    GroupPath::parse(path).unwrap().key()
}

pub fn rejection(ledger: &Ledger, stamp: Stamp) -> Rejection {
    ledger.outcome(&stamp).unwrap().unwrap_err().clone()
}
