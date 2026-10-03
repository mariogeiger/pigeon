//! How often a machine waits for its disk to keep its state: once for each
//! file it writes and each patch it takes, and never for a look that finds
//! every file as the index says.

mod common;

use std::time::Duration;

use common::{eventually, eventually_reaches, group, joined, shut_down};

/// How long after its change a file is read again by a look, as a later
/// write may leave its modification time as it was: the resolution of the
/// coarsest file times.
const RESOLUTION: Duration = Duration::from_secs(2);

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_waits_for_its_disk_once_per_file_it_writes_and_never_for_a_look() {
    let mut machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let bob = machines.pop().unwrap();
    bob.follow("+alice/").await;
    let before = bob.engine.status();
    let files: Vec<String> = (0..50).map(|n| format!("+alice/inbox/{n}.txt")).collect();
    for file in &files {
        machines[0].edit(file, "x");
    }
    eventually_reaches("bob holds every file of alice", files.len(), || async {
        files
            .iter()
            .filter(|file| bob.read(file).as_deref() == Some("x"))
            .count()
    })
    .await;
    let written = bob.engine.status();
    let taken = written.patches - before.patches;
    assert!(
        written.syncs - before.syncs <= u64::try_from(files.len() + taken).unwrap(),
        "{} syncs for {} files and {taken} patches",
        written.syncs - before.syncs,
        files.len()
    );
    tokio::time::sleep(RESOLUTION).await;
    let bob = bob.restart().await;
    eventually("bob looks at his whole disk", || async {
        bob.engine.status().scans > 0
    })
    .await;
    assert_eq!(bob.engine.status().syncs, 0);
    machines.push(bob);
    shut_down(machines).await;
}
