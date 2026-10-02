//! The state database as pigeon left it until 0.6: patches in the first
//! format, which opening moves into today's table, noting that the disk
//! may hold the read-only files of that time, and the set-aside list with
//! the files that showed its items, which the engine reads once to make
//! each item a suggestion.

use pigeon_core::clock::Stamp;
use pigeon_core::patch::{Content, SignedPatch};
use pigeon_core::patch_v1::SignedPatchV1;
use redb::{ReadableDatabase, ReadableTable, TableDefinition, TableHandle, WriteTransaction};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::state::{PATCHES, State};

const PATCHES_V1: TableDefinition<&[u8], &[u8]> = TableDefinition::new("patches");
const ASIDE: TableDefinition<u64, &[u8]> = TableDefinition::new("aside");
const ASIDE_FILES: TableDefinition<u64, &str> = TableDefinition::new("aside files");
const FROZEN: TableDefinition<(), ()> = TableDefinition::new("frozen files");

/// Moves every patch of the first format's table into today's, as signed
/// in that format, and removes the old table.
pub(crate) fn upgrade_patches(transaction: &WriteTransaction) -> Result<()> {
    let mut moved = false;
    {
        let old = transaction.open_table(PATCHES_V1)?;
        let mut new = transaction.open_table(PATCHES)?;
        for entry in old.iter()? {
            let (key, value) = entry?;
            let first: SignedPatchV1 = postcard::from_bytes(value.value())?;
            let bytes = postcard::to_stdvec(&SignedPatch::from(first))?;
            new.insert(key.value(), bytes.as_slice())?;
            moved = true;
        }
    }
    transaction.delete_table(PATCHES_V1)?;
    if moved {
        transaction.open_table(FROZEN)?;
    }
    Ok(())
}

/// Why pigeon set content aside until 0.6.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ReasonV1 {
    NotWritable,
    Unportable(String),
    Rejected(String),
    Superseded,
}

/// An item of the set-aside list: content found at `path`, relative to
/// the root, that the machine did not publish as it was.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AsideItemV1 {
    pub path: String,
    pub content: Option<Content>,
    pub replaces: Option<Stamp>,
    pub reason: ReasonV1,
    pub time: u64,
}

impl State {
    /// Whether the database comes from pigeon 0.6 or before, whose disk may
    /// hold read-only files, and still holds what that pigeon left: the
    /// set-aside list, oldest first.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read.
    pub fn left_by_v1(&self) -> Result<Option<Vec<AsideItemV1>>> {
        let transaction = self.database().begin_read()?;
        if !table_exists(&transaction, FROZEN.name())? {
            return Ok(None);
        }
        let mut items = Vec::new();
        if table_exists(&transaction, ASIDE.name())? {
            for entry in transaction.open_table(ASIDE)?.iter()? {
                let (_, value) = entry?;
                items.push(postcard::from_bytes(value.value())?);
            }
        }
        Ok(Some(items))
    }

    /// Forgets what pigeon 0.6 left, once the engine took it over.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be written.
    pub fn forget_v1(&self) -> Result<()> {
        let transaction = self.database().begin_write()?;
        transaction.delete_table(ASIDE)?;
        transaction.delete_table(ASIDE_FILES)?;
        transaction.delete_table(FROZEN)?;
        transaction.commit()?;
        Ok(())
    }
}

fn table_exists(transaction: &redb::ReadTransaction, name: &str) -> Result<bool> {
    Ok(transaction.list_tables()?.any(|table| table.name() == name))
}

#[cfg(test)]
mod tests {
    use iroh_base::SecretKey;
    use pigeon_core::identity::{GroupSecret, MachineCert, member_key};
    use pigeon_core::name::MemberName;
    use pigeon_core::patch::{ContentHash, Format};
    use pigeon_core::patch_v1::{ChangeV1, PatchV1, signed_message};
    use pigeon_core::path::GroupPath;
    use redb::Database;

    use super::*;
    use crate::state::stamp_key;

    #[test]
    fn opening_keeps_the_first_formats_patches_and_set_aside_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.redb");
        let group = GroupSecret([3; 32]).id();
        let name = MemberName::parse("mario").unwrap();
        let machine = SecretKey::from_bytes(&[1; 32]);
        let cert = MachineCert::issue(
            &group,
            name.clone(),
            &member_key(&group, &name),
            machine.public(),
        );
        let content = Content {
            hash: ContentHash([7; 32]),
            size: 1,
            executable: false,
        };
        let stamp = Stamp {
            time: 5,
            machine: machine.public(),
        };
        let applies = Some(GroupPath::parse(".pigeon/requests/r.json").unwrap());
        let first = PatchV1 {
            stamp,
            changes: vec![ChangeV1 {
                path: GroupPath::parse(".pigeon/members/mario").unwrap(),
                content: Some(content),
                replaces: None,
            }],
            applies: applies.clone(),
        };
        let unsigned = SignedPatch::from(SignedPatchV1 {
            patch: first.clone(),
            cert: cert.clone(),
            signature: machine.sign(b""),
        });
        let message = signed_message(&group, &unsigned.patch, applies).unwrap();
        let signed = SignedPatchV1 {
            patch: first,
            cert,
            signature: machine.sign(&message),
        };
        let item = AsideItemV1 {
            path: "notes.txt".into(),
            content: Some(content),
            replaces: None,
            reason: ReasonV1::NotWritable,
            time: 3,
        };
        {
            let database = Database::create(&path).unwrap();
            let transaction = database.begin_write().unwrap();
            transaction
                .open_table(PATCHES_V1)
                .unwrap()
                .insert(
                    stamp_key(&stamp).as_slice(),
                    postcard::to_stdvec(&signed).unwrap().as_slice(),
                )
                .unwrap();
            transaction
                .open_table(ASIDE)
                .unwrap()
                .insert(1, postcard::to_stdvec(&item).unwrap().as_slice())
                .unwrap();
            transaction.commit().unwrap();
        }
        let state = State::open(&path).unwrap();
        let patches = state.patches().unwrap();
        assert!(matches!(patches[0].signed_as, Format::V1 { .. }));
        let ledger = state.ledger(group).unwrap();
        assert!(ledger.members().contains_key(&name));
        assert_eq!(state.left_by_v1().unwrap(), Some(vec![item]));
        drop(state);
        let state = State::open(&path).unwrap();
        assert_eq!(state.patches().unwrap().len(), 1);
        assert!(state.left_by_v1().unwrap().is_some());
        state.forget_v1().unwrap();
        assert_eq!(state.left_by_v1().unwrap(), None);
        let fresh = State::open(&dir.path().join("fresh.redb")).unwrap();
        assert_eq!(fresh.left_by_v1().unwrap(), None);
    }
}
