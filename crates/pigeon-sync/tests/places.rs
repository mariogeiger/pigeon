//! Tests of placed folders: a folder moves to its destination behind a
//! link and keeps syncing both ways, a missing destination pauses it
//! without publishing a deletion, even through a restart, and the folder
//! moves back into the root.

#![cfg(unix)]

mod common;

use std::path::Path;

use common::{Machine, eventually, group, joined, path, shut_down};
use pigeon_store::layout::link_target;

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_placed_folder_moves_behind_a_link_and_keeps_syncing() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.follow("+alice/").await;
    alice.follow("+bob/").await;
    alice.edit("+alice/videos/a.mp4", "a");
    eventually("bob holds alice's video", || async {
        bob.read("+alice/videos/a.mp4").is_some()
    })
    .await;
    let disk = tempfile::tempdir().unwrap();
    let videos = disk.path().canonicalize().unwrap().join("videos");
    bob.engine
        .place(path("+alice/videos"), &videos)
        .await
        .unwrap();
    assert_eq!(read(&videos.join("a.mp4")).as_deref(), Some("a"));
    assert_eq!(
        link_target(&bob.file("+alice/videos")),
        Some(videos.clone())
    );
    alice.edit("+alice/videos/b.mp4", "b");
    eventually("a new video lands at the destination", || async {
        read(&videos.join("b.mp4")).as_deref() == Some("b")
    })
    .await;

    let big = disk.path().canonicalize().unwrap().join("big");
    bob.engine.place(path("+bob/big"), &big).await.unwrap();
    std::fs::write(big.join("x.txt"), "written at the destination").unwrap();
    eventually("an edit at the destination is published", || async {
        alice.read("+bob/big/x.txt").as_deref() == Some("written at the destination")
    })
    .await;
    let views = bob.engine.places().await;
    assert_eq!(views.len(), 2);
    assert!(views.iter().all(|view| view.problem.is_none()));
    assert!(bob.engine.status().errors.is_empty());
    assert!(
        bob.engine
            .place(path("+alice/videos/old"), &disk.path().join("old"))
            .await
            .is_err()
    );
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_destination_pauses_its_folder_until_it_returns() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob]: [Machine; 2] = machines.try_into().ok().unwrap();
    alice.follow("+bob/").await;
    let disk = tempfile::tempdir().unwrap();
    let mount = disk.path().canonicalize().unwrap().join("mount");
    std::fs::create_dir(&mount).unwrap();
    let big = mount.join("big");
    bob.engine.place(path("+bob/big"), &big).await.unwrap();
    bob.edit("+bob/big/x.txt", "x");
    eventually("alice holds bob's file", || async {
        alice.read("+bob/big/x.txt").is_some()
    })
    .await;

    let away = disk.path().join("away");
    std::fs::rename(&mount, &away).unwrap();
    let bob = bob.restart().await;
    eventually("bob's folder waits for its disk", || async {
        bob.engine.places().await[0].problem.is_some()
    })
    .await;
    bob.wait_past_settling().await;
    assert_eq!(alice.read("+bob/big/x.txt").as_deref(), Some("x"));
    assert!(!mount.exists(), "a missing disk is never made again");

    std::fs::rename(&away, &mount).unwrap();
    bob.edit("+bob/note.txt", "wakes it again");
    eventually("the folder is back in place", || async {
        bob.engine.places().await[0].problem.is_none()
    })
    .await;
    std::fs::write(big.join("x.txt"), "y").unwrap();
    eventually("its edits are published again", || async {
        alice.read("+bob/big/x.txt").as_deref() == Some("y")
    })
    .await;

    bob.engine.unplace(&path("+bob/big")).await.unwrap();
    assert!(link_target(&bob.file("+bob/big")).is_none());
    assert_eq!(bob.read("+bob/big/x.txt").as_deref(), Some("y"));
    assert!(!big.exists());
    assert!(bob.engine.places().await.is_empty());
    bob.wait_past_settling().await;
    assert_eq!(alice.read("+bob/big/x.txt").as_deref(), Some("y"));
    shut_down([alice, bob]).await;
}
