//! Tests of the state database: patches, recorded together or one by one,
//! survive reopening and fold back into the same ledger, the index, kept
//! suggestions, and placed folders round-trip, every write counted, and a
//! fresh start forgets everything recorded of the disk but the losses
//! noted, which concern the ledger.

use pigeon_core::patch::Content;
use pigeon_core::path::GroupPath;
use pigeon_core::places::Places;
use pigeon_core::test_machines::{change, content, group, machine};

use super::*;
use crate::disk::Stat;
use crate::index::Seen;

#[test]
fn patches_survive_reopening_in_stamp_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.redb");
    let file = GroupPath::parse("+mario/a").unwrap();
    let mario = machine("mario", 1);
    let join = mario.join(1);
    let write = mario.patch(300, vec![change(file.as_str(), Some(1), None)]);
    {
        let state = State::open(&path).unwrap();
        state.add_patches(&[write, join.clone()]).unwrap();
        let revision = state.revision();
        state.add_patch(&join).unwrap();
        assert_eq!(state.revision(), revision + 1);
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
                inode: None,
            },
            content: content(2),
            racy: false,
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
fn only_writes_that_change_what_the_disk_keeps_wait_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::open(&dir.path().join("s")).unwrap();
    let kept = Kept {
        statement: GroupPath::parse(".pigeon/suggestions/1.json").unwrap(),
        content: None,
    };
    state.keep("a", &kept).unwrap();
    state.keep("a", &kept).unwrap();
    state.unkeep("b").unwrap();
    state.set_applied_root(dir.path()).unwrap();
    state.set_applied_root(dir.path()).unwrap();
    assert_eq!((state.revision(), state.syncs()), (2, 2));
    state.unkeep("a").unwrap();
    assert_eq!(state.kept_at("a").unwrap(), None);
    assert_eq!((state.revision(), state.syncs()), (3, 3));
    let path = GroupPath::parse("a.txt").unwrap();
    let entry = IndexEntry {
        path: path.clone(),
        seen: None,
        synced: None,
    };
    state.refresh_index([(&path.key(), Some(&entry))]).unwrap();
    assert_eq!(state.index_entry(&path.key()).unwrap(), Some(entry));
    assert_eq!((state.revision(), state.syncs()), (4, 3));
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
            .set(
                &root,
                GroupPath::parse(folder).unwrap(),
                destination,
                crate::layout::resolved,
            )
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

#[test]
fn the_index_of_0_8_moves_to_the_current_form_trusted_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s");
    let file = GroupPath::parse("Docs/a.txt").unwrap();
    {
        let database = redb::Database::create(&path).unwrap();
        let transaction = database.begin_write().unwrap();
        let old: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("index");
        let seen = Some(((7_u64, 42_i128, Some(true)), content(3)));
        let bytes = postcard::to_stdvec(&(&file, seen, None::<Stamp>)).unwrap();
        transaction
            .open_table(old)
            .unwrap()
            .insert(file.key().as_str(), bytes.as_slice())
            .unwrap();
        transaction.commit().unwrap();
    }
    let state = State::open(&path).unwrap();
    let entry = state.index_entry(&file.key()).unwrap().unwrap();
    let seen = entry.seen.unwrap();
    assert_eq!(entry.path, file);
    assert_eq!((seen.stat.size, seen.stat.modified), (7, 42));
    assert_eq!(seen.stat.executable, Some(true));
    assert_eq!(seen.stat.inode, None);
    assert_eq!(seen.content, content(3));
    assert!(!seen.racy);
    drop(state);
    let again = State::open(&path).unwrap();
    assert_eq!(again.index(None).unwrap().len(), 1);
}

#[test]
fn a_fresh_start_forgets_the_disk_and_its_root() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::open(&dir.path().join("s")).unwrap();
    assert!(state.index_is_empty().unwrap());
    assert_eq!(state.applied_root().unwrap(), None);
    let root = dir.path().join("root");
    state.set_applied_root(&root).unwrap();
    assert_eq!(state.applied_root().unwrap(), Some(root));
    let path = GroupPath::parse("a.txt").unwrap();
    let entry = IndexEntry {
        path: path.clone(),
        seen: None,
        synced: None,
    };
    state.update_index([(&path.key(), Some(&entry))]).unwrap();
    state.set_applied_selection(&[]).unwrap();
    assert!(!state.index_is_empty().unwrap());
    state.forget_disk().unwrap();
    assert!(state.index_is_empty().unwrap());
    assert_eq!(state.applied_root().unwrap(), None);
    assert_eq!(state.applied_selection().unwrap(), None);
}

#[test]
fn noted_losses_survive_reopening_and_a_fresh_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.redb");
    let stamp = machine("mario", 1).join(5).stamp();
    let lost = GroupPath::parse("+mario/a").unwrap().key();
    let kept = GroupPath::parse("+mario/b").unwrap().key();
    {
        let state = State::open(&path).unwrap();
        assert!(!state.notes_losses().unwrap());
        assert!(!state.loss_noted(&stamp, &lost).unwrap());
        state.note_losses([]).unwrap();
        assert!(state.notes_losses().unwrap());
        state.note_losses([(&stamp, &lost)]).unwrap();
        state.forget_disk().unwrap();
    }
    let state = State::open(&path).unwrap();
    assert!(state.notes_losses().unwrap());
    assert!(state.loss_noted(&stamp, &lost).unwrap());
    assert!(!state.loss_noted(&stamp, &kept).unwrap());
}
