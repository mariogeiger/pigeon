//! Tests of placed folders: a folder moves to its destination behind a
//! link and keeps syncing both ways, a missing destination freezes it
//! without publishing a deletion, even through a restart, and the folder
//! moves back into the root.

#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Machine, eventually, group, joined};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_store::layout::link_target;

fn folder(text: &str) -> GroupPath {
    GroupPath::parse(text).unwrap()
}

async fn follow(machine: &Machine, pattern: &str) {
    machine
        .engine
        .set_rule(Rule {
            pattern: pattern.into(),
            cutoff: Cutoff::PlusInfinity,
        })
        .await
        .unwrap();
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_placed_folder_moves_behind_a_link_and_keeps_syncing() {
    let machines = group(&[("alice", "a"), ("bob", "b")]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    follow(bob, "+alice/").await;
    follow(alice, "+bob/").await;
    alice.edit("+alice/videos/a.mp4", "a");
    eventually("bob holds alice's video", || async {
        bob.read("+alice/videos/a.mp4").is_some()
    })
    .await;
    let disk = tempfile::tempdir().unwrap();
    let videos = disk.path().canonicalize().unwrap().join("videos");
    bob.engine
        .place(folder("+alice/videos"), &videos)
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
    bob.engine.place(folder("+bob/big"), &big).await.unwrap();
    std::fs::write(big.join("x.txt"), "written at the destination").unwrap();
    eventually("an edit at the destination is published", || async {
        alice.read("+bob/big/x.txt").as_deref() == Some("written at the destination")
    })
    .await;
    let views = bob.engine.places().await;
    assert_eq!(views.len(), 2);
    assert!(views.iter().all(|view| view.problem.is_none()));
    assert!(bob.engine.status().await.errors.is_empty());
    assert!(
        bob.engine
            .place(folder("+alice/videos/old"), &disk.path().join("old"))
            .await
            .is_err()
    );
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_destination_freezes_its_folder_until_it_returns() {
    let machines = group(&[("alice", "a"), ("bob", "b")]).await;
    joined(&machines).await;
    let [alice, bob]: [Machine; 2] = machines.try_into().ok().unwrap();
    follow(&alice, "+bob/").await;
    let disk = tempfile::tempdir().unwrap();
    let mount = disk.path().canonicalize().unwrap().join("mount");
    std::fs::create_dir(&mount).unwrap();
    let big = mount.join("big");
    bob.engine.place(folder("+bob/big"), &big).await.unwrap();
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
    tokio::time::sleep(Duration::from_secs(2)).await;
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

    bob.engine.unplace(&folder("+bob/big")).await.unwrap();
    assert!(link_target(&bob.file("+bob/big")).is_none());
    assert_eq!(bob.read("+bob/big/x.txt").as_deref(), Some("y"));
    assert!(!big.exists());
    assert!(bob.engine.places().await.is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(alice.read("+bob/big/x.txt").as_deref(), Some("y"));
    for machine in [alice, bob] {
        machine.engine.shutdown().await.unwrap();
    }
}
