//! Tests that this pigeon opens the state every released pigeon left: each
//! release records, once its version is set, the directories and the root
//! of a machine it ran into `fixtures/<version>`, with
//! `cargo test -p pigeon-sync --test upgrade -- --ignored`, and nobody
//! changes them after; this pigeon must open each of them as it is and show
//! the files recorded there, publishing nothing.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::*;
use iroh::address_lookup::MemoryLookup;
use pigeon_store::blobs::blob_hash;
use serde::{Deserialize, Serialize};

/// What a release recorded of its machine besides the directories: how
/// many patches it held and the content hash of each of its files.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Recorded {
    patches: usize,
    files: BTreeMap<String, String>,
}

/// The files the recorded machine holds: a file, one in a folder, one
/// edited after it was first published, and one too large for the blob
/// store to keep inline.
fn files() -> [(&'static str, String); 4] {
    [
        ("+alice/notes.txt", "notes".to_owned()),
        ("+alice/folder/inside.txt", "inside a folder".to_owned()),
        ("+alice/edited.txt", "edited once".to_owned()),
        ("+alice/large.txt", "pigeon ".repeat(50_000)),
    ]
}

/// What `machine` records of itself.
async fn recorded(machine: &Machine) -> Recorded {
    Recorded {
        patches: machine.engine.status().patches,
        files: machine
            .engine
            .list(None)
            .await
            .unwrap()
            .into_iter()
            .map(|file| {
                (
                    file.path.to_string(),
                    blob_hash(&file.content.hash).to_string(),
                )
            })
            .collect(),
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Copies the folder `from`, every file and folder in it, to `to`.
fn copy_folder(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_folder(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "run once by each release, to record the state it leaves"]
async fn record_the_state_this_version_leaves() {
    let fixture = fixtures().join(env!("CARGO_PKG_VERSION"));
    assert!(!fixture.exists(), "a release records its state once");
    let machines = group(&["alice"]).await;
    joined(&machines).await;
    let [alice]: [Machine; 1] = machines.try_into().ok().unwrap();
    alice.edit("+alice/edited.txt", "first");
    eventually("the first version is published", || async {
        alice.engine.history(&path("+alice/edited.txt")).len() == 1
    })
    .await;
    for (file, text) in files() {
        alice.edit(file, &text);
    }
    eventually("every file is published", || async {
        alice.engine.history(&path("+alice/edited.txt")).len() == 2
            && alice.engine.list(None).await.unwrap().len() == files().len()
    })
    .await;
    converged(std::slice::from_ref(&alice), &[]).await;
    let recorded = recorded(&alice).await;
    let alice = alice
        .restart_after(|dirs, root| {
            copy_folder(dirs.config(), &fixture.join("config"));
            copy_folder(dirs.data(), &fixture.join("data"));
            copy_folder(root, &fixture.join("root"));
            let text = serde_json::to_string_pretty(&recorded).unwrap();
            std::fs::write(fixture.join("recorded.json"), text + "\n").unwrap();
        })
        .await;
    shut_down([alice]).await;
}

/// Opens a copy of the state `fixture` holds and checks that it shows the
/// files recorded there, with their contents, publishing nothing.
async fn opens(fixture: &Path) {
    let text = std::fs::read_to_string(fixture.join("recorded.json")).unwrap();
    let recorded_then: Recorded = serde_json::from_str(&text).unwrap();
    let dir = tempfile::tempdir().unwrap();
    for folder in ["config", "data", "root"] {
        copy_folder(&fixture.join(folder), &dir.path().join(folder));
    }
    let machine = reopen(dir, options(&MemoryLookup::new())).await;
    machine.wait_past_settling().await;
    let machines = [machine];
    converged(&machines, &[]).await;
    assert_eq!(
        recorded(&machines[0]).await,
        recorded_then,
        "{}",
        fixture.display()
    );
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_state_a_release_left_opens_with_its_files() {
    let Ok(releases) = std::fs::read_dir(fixtures()) else {
        return;
    };
    for release in releases {
        opens(&release.unwrap().path()).await;
    }
}
