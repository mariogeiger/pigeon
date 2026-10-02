//! How much a machine sends at once: edits that settle together beyond what
//! one patch carries go out in several patches, and an announcement names
//! only as many drafts as it carries.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{eventually, group, group_with, joined, path, shut_down};
use pigeon_net::wire::MAX_MESSAGE;

/// `count` paths of nearly two thousand bytes each, under `folder`, so
/// that seven hundred carry more than a patch.
fn long_paths(folder: &str, count: usize) -> Vec<String> {
    let deep = ["a", "b", "c", "d", "e", "f", "g", "h"]
        .map(|letter| letter.repeat(240))
        .join("/");
    (0..count)
        .map(|n| format!("{folder}/{deep}/{n}.txt"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_beyond_what_one_patch_carries_go_out_in_several_patches() {
    let mut machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let bob = machines.pop().unwrap();
    let alice = &machines[0];
    bob.follow("+alice/").await;
    let files = long_paths("+alice", 700);
    for file in &files {
        alice.edit(file, "x");
    }
    eventually("bob holds every file of alice", || async {
        files
            .iter()
            .all(|file| bob.read(file).as_deref() == Some("x"))
    })
    .await;
    let mut patches: BTreeMap<_, usize> = BTreeMap::new();
    for file in &files {
        let history = alice.engine.history(&path(file));
        assert_eq!(history.len(), 1, "{file} is published once");
        *patches.entry(history[0].stamp).or_default() += file.len();
    }
    assert!(patches.len() > 1, "the edits go out in several patches");
    for bytes in patches.values() {
        assert!(
            *bytes <= MAX_MESSAGE / 64,
            "a patch of {bytes} bytes of paths"
        );
    }
    for machine in [alice, &bob] {
        assert!(machine.engine.status().await.errors.is_empty());
    }
    machines.push(bob);
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_announcement_names_only_as_many_drafts_as_it_carries() {
    let machines = group_with(&["alice", "bob"], |options| {
        options.settle_draft = Duration::from_secs(600);
    })
    .await;
    joined(&machines).await;
    let (alice, bob) = (&machines[0], &machines[1]);
    let files = long_paths("inbox", 700);
    for file in &files {
        alice.edit(file, "x");
    }
    eventually("alice holds every draft", || async {
        alice.engine.pending(None).await.len() == files.len()
    })
    .await;
    eventually("bob learns of alice's drafts", || async {
        bob.engine.pending(None).await.len() > 400
    })
    .await;
    let announced = bob.engine.pending(None).await;
    assert!(announced.len() < files.len());
    let bytes: usize = announced.iter().map(|view| view.path.as_str().len()).sum();
    assert!(
        bytes <= MAX_MESSAGE / 64,
        "an announcement of {bytes} bytes of paths"
    );
    assert!(announced.iter().all(|view| !view.here && view.draft));
    for machine in &machines {
        assert!(machine.engine.status().await.errors.is_empty());
    }
    shut_down(machines).await;
}
