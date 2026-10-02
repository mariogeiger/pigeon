//! Tests of a server, a machine of a member of its own that follows every
//! file and keeps the history of every file it downloads: once the owner's
//! only machine is off, another member gets the owner's file from it, and
//! a new machine of the owner restores a past version that only the server
//! still holds.

mod common;

use common::*;
use pigeon_core::retention::Retention;
use pigeon_sync::Edit;

fn write(name: &str, text: &str) -> Edit {
    Edit::Write {
        path: path(name),
        bytes: text.as_bytes().to_vec(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_serves_the_files_and_the_history_of_a_member_whose_machine_is_off() {
    let machines = group(&["alice", "server", "bob"]).await;
    joined(&machines).await;
    let [laptop, server, bob]: [Machine; 3] = machines.try_into().ok().unwrap();
    server.follow("*").await;
    server
        .engine
        .set_retention(&Retention {
            everything: true,
            ..Retention::default()
        })
        .await
        .unwrap();
    laptop
        .engine
        .edit(vec![write("+alice/report.txt", "first draft")])
        .await
        .unwrap();
    let then = laptop.engine.now();
    laptop
        .engine
        .edit(vec![write("+alice/report.txt", "final")])
        .await
        .unwrap();
    let history = laptop.engine.history(&path("+alice/report.txt"));
    let [first, _] = &history[..] else {
        panic!("two versions: {history:?}")
    };
    let first = first.content.unwrap();
    eventually("the server holds the final report", || async {
        server.read("+alice/report.txt").as_deref() == Some("final")
    })
    .await;
    server.wait_past_collection().await;
    assert!(server.engine.read(&first).await.unwrap().is_some());
    let key = server.engine.group_key();
    shut_down([laptop]).await;
    bob.follow("+alice/").await;
    eventually("bob gets the report from the server", || async {
        bob.read("+alice/report.txt").as_deref() == Some("final")
    })
    .await;
    let desktop = server.join_with(&key, "alice").await;
    let machines = [server, bob, desktop];
    joined(&machines).await;
    let [_, bob, desktop] = &machines;
    eventually(
        "the new desktop gets the report from the server",
        || async { desktop.read("+alice/report.txt").as_deref() == Some("final") },
    )
    .await;
    for machine in [bob, desktop] {
        assert!(machine.engine.read(&first).await.unwrap().is_none());
    }
    desktop
        .engine
        .restore("+alice/report.txt", then)
        .await
        .unwrap();
    eventually("the desktop shows the first draft again", || async {
        desktop.read("+alice/report.txt").as_deref() == Some("first draft")
    })
    .await;
    converged(&machines, &[]).await;
    assert_eq!(
        bob.read("+alice/report.txt").as_deref(),
        Some("first draft")
    );
    shut_down(machines).await;
}
