//! Tests of a machine whose disk fails it: a folder that refuses writes
//! makes it report the files it cannot write, delete and publish nothing,
//! and write them by the scan of the whole root after the folder takes
//! writes again; a lost state makes it rebuild from the group it reaches
//! before it would claim its name anew, adopt the files its disk already
//! shows and publish nothing.

mod common;

use std::time::Duration;

use common::*;
use pigeon_sync::Options;

#[cfg(unix)]
fn set_writable(folder: &std::path::Path, writable: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if writable { 0o755 } else { 0o555 };
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_folder_that_refuses_writes_is_reported_then_written_once_it_takes_them() {
    let machines = group_with(&["alice", "bob"], |options| {
        options.rescan = Duration::from_millis(200);
    })
    .await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.follow("+alice/").await;
    alice.edit("+alice/docs/kept.txt", "first");
    eventually("bob holds the file", || async {
        bob.read("+alice/docs/kept.txt").as_deref() == Some("first")
    })
    .await;
    let folder = bob.file("+alice/docs");
    set_writable(&folder, false);
    alice.edit("+alice/docs/kept.txt", "second");
    alice.edit("+alice/docs/new.txt", "new");
    eventually("bob reports both files", || async {
        let errors = bob.engine.status().errors;
        ["kept.txt", "new.txt"]
            .iter()
            .all(|name| errors.iter().any(|error| error.contains(name)))
    })
    .await;
    bob.wait_past_settling().await;
    assert_eq!(bob.read("+alice/docs/kept.txt").as_deref(), Some("first"));
    assert_eq!(bob.read("+alice/docs/new.txt"), None);
    assert_eq!(alice.engine.history(&path("+alice/docs/kept.txt")).len(), 2);
    assert_eq!(
        bob.engine.status().patches,
        alice.engine.status().patches,
        "bob publishes nothing"
    );
    let reported = bob.engine.status().errors;
    set_writable(&folder, true);
    let expected: Vec<&str> = reported.iter().map(String::as_str).collect();
    converged(&machines, &expected).await;
    assert_eq!(bob.read("+alice/docs/kept.txt").as_deref(), Some("second"));
    assert_eq!(bob.read("+alice/docs/new.txt").as_deref(), Some("new"));
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_that_lost_its_state_adopts_its_disk_and_publishes_nothing() {
    let machines = group_with(&["alice", "bob"], |options| {
        options.join_delay = Options::default().join_delay;
    })
    .await;
    joined(&machines).await;
    let [alice, bob]: [Machine; 2] = machines.try_into().ok().unwrap();
    bob.follow("+alice/").await;
    alice.edit("+alice/notes.txt", "alice's notes");
    bob.edit("+bob/notes.txt", "bob's notes");
    let machines = [alice, bob];
    converged(&machines, &[]).await;
    let patches = machines[0].engine.status().patches;
    let [alice, bob] = machines;
    let bob = bob
        .restart_after(|dirs, _| std::fs::remove_file(dirs.state_path()).unwrap())
        .await;
    let machines = [alice, bob];
    converged(&machines, &[]).await;
    for machine in &machines {
        assert_eq!(machine.engine.status().patches, patches);
        for file in ["+alice/notes.txt", "+bob/notes.txt"] {
            assert_eq!(machine.engine.history(&path(file)).len(), 1);
        }
    }
    shut_down(machines).await;
}
