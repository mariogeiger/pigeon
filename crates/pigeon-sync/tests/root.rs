//! The root as the place of the group's files: a root gone, or left
//! without its `.pigeon` folder as an empty mountpoint is, deletes nothing
//! and syncs again once back; moved, the group resumes where its files
//! went, starts afresh in an empty folder, which leaves the old root as it
//! was, and refuses a folder holding other files.

mod common;

use std::time::Duration;

use common::{eventually, group_with, joined, path, shut_down};

fn rescanning(options: &mut pigeon_sync::Options) {
    options.rescan = Duration::from_millis(200);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_root_gone_or_unmarked_deletes_nothing_and_syncs_again_once_back() {
    let machines = group_with(&["alice", "bob"], rescanning).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.follow("+alice/").await;
    alice.edit("+alice/notes.txt", "one");
    eventually("bob holds alice's file", || async {
        bob.read("+alice/notes.txt").as_deref() == Some("one")
    })
    .await;
    let away = alice.root.with_file_name("away");
    std::fs::rename(&alice.root, &away).unwrap();
    eventually("alice pauses", || async {
        alice.engine.status().await.paused.is_some()
    })
    .await;
    alice.wait_past_settling().await;
    std::fs::create_dir(&alice.root).unwrap();
    alice.wait_past_settling().await;
    alice.wait_past_settling().await;
    let paused = alice.engine.status().await.paused.unwrap_or_default();
    assert!(paused.contains(".pigeon"), "{paused}");
    assert_eq!(alice.engine.history(&path("+alice/notes.txt")).len(), 1);
    assert_eq!(bob.read("+alice/notes.txt").as_deref(), Some("one"));
    std::fs::remove_dir(&alice.root).unwrap();
    std::fs::rename(&away, &alice.root).unwrap();
    eventually("alice resumes", || async {
        alice.engine.status().await.paused.is_none()
    })
    .await;
    alice.edit("+alice/notes.txt", "two");
    eventually("bob follows alice's edit", || async {
        bob.read("+alice/notes.txt").as_deref() == Some("two")
    })
    .await;
    assert_eq!(alice.engine.history(&path("+alice/notes.txt")).len(), 2);
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_root_moves_to_where_its_files_went_or_starts_afresh_in_an_empty_folder() {
    let mut machines = group_with(&["alice", "bob"], rescanning).await;
    joined(&machines).await;
    let bob = machines.pop().unwrap();
    let alice = machines.pop().unwrap();
    alice.follow("+alice/").await;
    bob.follow("+alice/").await;
    alice.edit("+alice/notes.txt", "one");
    eventually("bob holds alice's file", || async {
        bob.read("+alice/notes.txt").as_deref() == Some("one")
    })
    .await;

    let moved = alice.root.with_file_name("moved");
    let alice = alice
        .restart_in(moved.clone(), |old| std::fs::rename(old, &moved).unwrap())
        .await
        .unwrap();
    alice.wait_past_settling().await;
    assert_eq!(alice.engine.history(&path("+alice/notes.txt")).len(), 1);
    assert_eq!(alice.read("+alice/notes.txt").as_deref(), Some("one"));
    alice.edit("+alice/notes.txt", "two");
    eventually("bob follows the edit in the moved root", || async {
        bob.read("+alice/notes.txt").as_deref() == Some("two")
    })
    .await;

    let old = alice.root.clone();
    let fresh = alice.root.with_file_name("fresh");
    let alice = alice
        .restart_in(fresh.clone(), |_| std::fs::create_dir(&fresh).unwrap())
        .await
        .unwrap();
    eventually("the fresh root holds alice's file", || async {
        alice.read("+alice/notes.txt").as_deref() == Some("two")
    })
    .await;
    alice.wait_past_settling().await;
    assert_eq!(alice.engine.history(&path("+alice/notes.txt")).len(), 2);
    assert_eq!(
        std::fs::read_to_string(old.join("+alice/notes.txt")).unwrap(),
        "two"
    );

    let other = alice.root.with_file_name("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("letter.txt"), "dear").unwrap();
    let refused = alice.restart_in(other, |_| {}).await.err().unwrap();
    assert!(
        refused.to_string().contains("no .pigeon folder"),
        "{refused:#}"
    );
    shut_down([bob]).await;
}
