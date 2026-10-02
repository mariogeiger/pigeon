//! How a machine writes its disk: a stop between writing a file and
//! recording it publishes nothing, the temporary files a stopped run left
//! go, and a draft moves onto no file pigeon did not see.

mod common;

use std::time::Duration;

use common::{eventually, group, group_with, joined, path, shut_down};
use pigeon_store::state::State;
use pigeon_sync::Edit;

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_between_writing_a_file_and_recording_it_publishes_nothing() {
    let mut machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let bob = machines.pop().unwrap();
    let alice = &machines[0];
    bob.follow("+alice/").await;
    alice.edit("+alice/a.txt", "one");
    eventually("bob holds alice's file", || async {
        bob.read("+alice/a.txt").as_deref() == Some("one")
    })
    .await;
    let key = path("+alice/a.txt").key();
    let mut recorded = None;
    let bob = bob
        .restart_after(|dirs, _| {
            let state = State::open(&dirs.state_path()).unwrap();
            recorded = state.index_entry(&key).unwrap();
        })
        .await;
    alice.edit("+alice/a.txt", "two");
    eventually("bob holds alice's edit", || async {
        bob.read("+alice/a.txt").as_deref() == Some("two")
    })
    .await;
    let bob = bob
        .restart_after(|dirs, _| {
            let state = State::open(&dirs.state_path()).unwrap();
            state.update_index([(&key, recorded.as_ref())]).unwrap();
        })
        .await;
    bob.wait_past_settling().await;
    bob.wait_past_settling().await;
    assert_eq!(alice.engine.history(&path("+alice/a.txt")).len(), 2);
    assert_eq!(bob.read("+alice/a.txt").as_deref(), Some("two"));
    for machine in [alice, &bob] {
        assert!(machine.engine.suggestions().await.is_empty());
        assert!(machine.engine.status().await.errors.is_empty());
    }
    machines.push(bob);
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_temporary_files_a_stopped_run_left_go() {
    let machines = group_with(&["alice"], |options| {
        options.rescan = Duration::from_millis(200);
    })
    .await;
    joined(&machines).await;
    let alice = &machines[0];
    alice.edit("+alice/a.txt", "a");
    let pid = std::process::id();
    let left = alice.file(&format!("+alice/.~pigeon-{}-0", pid.wrapping_add(1)));
    let written = alice.file(&format!("+alice/.~pigeon-{pid}-999999"));
    std::fs::write(&left, "half").unwrap();
    std::fs::write(&written, "being written").unwrap();
    eventually("the left-over goes", || async { !left.exists() }).await;
    eventually("alice's file is published", || async {
        alice.engine.history(&path("+alice/a.txt")).len() == 1
    })
    .await;
    assert!(written.exists(), "a write of this run stays");
    assert!(alice.engine.status().await.errors.is_empty());
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_moves_onto_no_file_pigeon_did_not_see() {
    let machines = group_with(&["alice"], |options| {
        options.settle_draft = Duration::from_secs(600);
    })
    .await;
    joined(&machines).await;
    let alice = &machines[0];
    alice.edit("shared/draft.txt", "draft");
    eventually("the draft waits", || async {
        alice
            .engine
            .pending(None)
            .await
            .iter()
            .any(|view| view.here)
    })
    .await;
    alice.edit("shared/taken.txt", "untracked");
    let rename = Edit::Rename {
        from: path("shared/draft.txt"),
        to: path("shared/taken.txt"),
    };
    let refused = alice.engine.edit(vec![rename]).await.unwrap_err();
    assert!(refused.to_string().contains("exists"), "{refused:#}");
    assert_eq!(alice.read("shared/draft.txt").as_deref(), Some("draft"));
    assert_eq!(alice.read("shared/taken.txt").as_deref(), Some("untracked"));
    shut_down(machines).await;
}
