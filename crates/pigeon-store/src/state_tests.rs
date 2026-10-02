//! Tests of the state database: patches survive reopening and fold back
//! into the same ledger, and the index, kept suggestions, and placed
//! folders round-trip, every write counted.

use iroh_base::SecretKey;
use pigeon_core::clock::Stamp;
use pigeon_core::identity::{GroupSecret, MachineCert, member_key};
use pigeon_core::name::MemberName;
use pigeon_core::patch::{Change, Content, ContentHash, Patch};
use pigeon_core::path::GroupPath;
use pigeon_core::places::Places;
use pigeon_core::statement::member_path;

use super::*;
use crate::disk::Stat;
use crate::index::Seen;

fn group() -> GroupId {
    GroupSecret([3; 32]).id()
}

fn content(byte: u8) -> Content {
    Content {
        hash: ContentHash([byte; 32]),
        size: 1,
        executable: false,
    }
}

fn patch(time: u64, path: &GroupPath) -> SignedPatch {
    let name = MemberName::parse("mario").unwrap();
    let key = SecretKey::from_bytes(&[1; 32]);
    let cert = MachineCert::issue(
        &group(),
        name,
        &member_key(&group(), &MemberName::parse("mario").unwrap()),
        key.public(),
    );
    let unsigned = Patch {
        stamp: Stamp {
            time,
            machine: key.public(),
        },
        changes: vec![Change {
            path: path.clone(),
            content: Some(content(1)),
            replaces: None,
            continues: None,
        }],
    };
    SignedPatch::sign(&group(), unsigned, cert, &key)
}

#[test]
fn patches_survive_reopening_in_stamp_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.redb");
    let file = GroupPath::parse("+mario/a").unwrap();
    let join = patch(1, &member_path(&MemberName::parse("mario").unwrap()));
    let write = patch(300, &file);
    {
        let state = State::open(&path).unwrap();
        state.add_patch(&write).unwrap();
        state.add_patch(&join).unwrap();
        state.add_patch(&join).unwrap();
    }
    let state = State::open(&path).unwrap();
    let stamps: Vec<u64> = state
        .patches()
        .unwrap()
        .iter()
        .map(|p| p.stamp().time)
        .collect();
    assert_eq!(stamps, [1, 300]);
    let ledger = state.ledger(group()).unwrap();
    assert_eq!(ledger.head(&file.key()).unwrap().content, Some(content(1)));
}

#[test]
fn index_entries_are_set_and_removed_together() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::open(&dir.path().join("s")).unwrap();
    let a = GroupPath::parse("A").unwrap();
    let b = GroupPath::parse("b").unwrap();
    let entry = |path: &GroupPath| IndexEntry {
        path: path.clone(),
        seen: Some(Seen {
            stat: Stat {
                size: 1,
                modified: -5,
                executable: None,
            },
            content: content(2),
        }),
        synced: None,
    };
    let (ea, eb) = (entry(&a), entry(&b));
    state
        .update_index([(&a.key(), Some(&ea)), (&b.key(), Some(&eb))])
        .unwrap();
    assert_eq!(state.index_entry(&a.key()).unwrap(), Some(ea));
    state.update_index([(&b.key(), None)]).unwrap();
    assert_eq!(state.index(None).unwrap().len(), 1);
    assert_eq!(state.index_entry(&b.key()).unwrap(), None);
}

#[test]
fn index_entries_are_listed_under_a_folder_without_case() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::open(&dir.path().join("s")).unwrap();
    let paths = ["a", "a/b", "A/c", "ab", "a.txt", "b"].map(|p| GroupPath::parse(p).unwrap());
    let entries: Vec<IndexEntry> = paths
        .iter()
        .map(|path| IndexEntry {
            path: path.clone(),
            seen: None,
            synced: None,
        })
        .collect();
    let keys: Vec<PathKey> = paths.iter().map(GroupPath::key).collect();
    state
        .update_index(keys.iter().zip(entries.iter().map(Some)))
        .unwrap();
    let under = |text: &str| -> Vec<String> {
        let prefix = GroupPath::parse(text).unwrap();
        let listed = state.index(Some(&prefix)).unwrap();
        listed
            .into_iter()
            .map(|e| e.path.as_str().to_owned())
            .collect()
    };
    assert_eq!(under("a"), ["a", "a/b", "A/c"]);
    assert_eq!(under("A/C"), ["A/c"]);
    assert!(under("c").is_empty());
    assert_eq!(state.index(None).unwrap().len(), 6);
}

#[test]
fn kept_suggestions_round_trip_each_write_counted() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::open(&dir.path().join("s")).unwrap();
    let kept = |content: Option<Content>| Kept {
        statement: GroupPath::parse(".pigeon/suggestions/1.json").unwrap(),
        content,
    };
    assert_eq!(state.revision(), 0);
    state.keep("a", &kept(Some(content(4)))).unwrap();
    assert_eq!(state.revision(), 1);
    state.keep("b:c", &kept(None)).unwrap();
    state.keep("a", &kept(None)).unwrap();
    state.unkeep("b:c").unwrap();
    drop(state);
    let state = State::open(&dir.path().join("s")).unwrap();
    assert_eq!(state.kept().unwrap(), [("a".to_owned(), kept(None))].into());
    assert_eq!(state.kept_at("a").unwrap(), Some(kept(None)));
    assert_eq!(state.kept_at("b:c").unwrap(), None);
    assert_eq!(state.revision(), 0, "no read counts");
}

#[test]
fn placed_folders_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::open(&dir.path().join("s")).unwrap();
    assert_eq!(state.placed().unwrap(), Places::default());
    let mut places = Places::default();
    let root = std::env::temp_dir().join("root");
    for folder in ["videos", "Photos 2026"] {
        let destination = std::env::temp_dir().join("disk").join(folder);
        places
            .set(&root, GroupPath::parse(folder).unwrap(), destination)
            .unwrap();
    }
    state.set_placed(&places).unwrap();
    assert_eq!(state.placed().unwrap(), places);
    assert!(places.remove(&GroupPath::parse("videos").unwrap()));
    state.set_placed(&places).unwrap();
    drop(state);
    let state = State::open(&dir.path().join("s")).unwrap();
    assert_eq!(state.placed().unwrap(), places);
}
