//! Whether a group converged, as each test that leaves its group at rest
//! asks last: every machine knows the same patches, files and suggestions
//! and waits for nothing; each disk shows the current version of every file
//! its machine follows, what its own suggestions keep, and nothing the
//! group does not know; no machine reported an error but those the
//! scenario makes; and some machine holds the content of every version in
//! the history of every file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use pigeon_core::clock::Stamp;
use pigeon_core::patch::{Content, VersionRef};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::Cutoff;
use pigeon_core::statement::STATEMENTS;
use pigeon_store::disk::fs_path;
use pigeon_store::index::hash_file;

use super::Machine;

/// What a machine knows of the group, which every machine of a converged
/// group knows alike.
#[derive(PartialEq, Debug)]
struct Knowledge {
    patches: usize,
    files: Vec<(GroupPath, Stamp, Content)>,
    suggestions: BTreeSet<VersionRef>,
}

async fn knowledge(machine: &Machine) -> Knowledge {
    let engine = &machine.engine;
    Knowledge {
        patches: engine.status().patches,
        files: engine
            .list(None)
            .await
            .unwrap()
            .into_iter()
            .map(|file| (file.path, file.stamp, file.content))
            .collect(),
        suggestions: engine
            .suggestions()
            .await
            .into_iter()
            .map(|view| view.statement)
            .collect(),
    }
}

/// Every file under `folder`, but pigeon's statements.
fn files_under(folder: &Path, files: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        if entry.file_name() == STATEMENTS {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            files_under(&entry.path(), files);
        } else {
            files.insert(entry.path());
        }
    }
}

/// How `machine`'s disk differs from what it must show, if it does: the
/// current version of each file it follows, but where one of its own live
/// suggestions keeps another or none, and no file the group does not know.
/// A file it pins or leaves free may show any version or none.
async fn disk_difference(machine: &Machine) -> Option<String> {
    let engine = &machine.engine;
    let mut shown: BTreeMap<PathBuf, Option<Content>> = BTreeMap::new();
    let mut known = BTreeSet::new();
    for file in engine.list(None).await.unwrap() {
        let location = fs_path(&machine.root, &file.path);
        if file.cutoff == Cutoff::PlusInfinity {
            shown.insert(location.clone(), Some(file.content));
        }
        known.insert(location);
    }
    for view in engine.suggestions().await {
        if view.machine == engine.machine() {
            for change in view.changes {
                let location = fs_path(&machine.root, &change.path);
                shown.insert(location.clone(), change.content);
                known.insert(location);
            }
        }
    }
    let mut held = BTreeSet::new();
    files_under(&machine.root, &mut held);
    if let Some(unknown) = held.difference(&known).next() {
        return Some(format!(
            "{}, which the group does not know",
            unknown.display()
        ));
    }
    for (location, content) in shown {
        let hash = hash_file(&location).ok();
        if hash != content.map(|content| content.hash) {
            return Some(format!(
                "{} as {hash:?}, not {content:?}",
                location.display()
            ));
        }
    }
    None
}

/// What keeps `machines` from agreeing, if anything does.
async fn disagreement(machines: &[Machine]) -> Option<String> {
    let first = knowledge(&machines[0]).await;
    for (index, machine) in machines.iter().enumerate() {
        let status = machine.engine.status();
        if status.pending + status.fetching > 0 {
            return Some(format!("machine {index} waits: {status:?}"));
        }
        let known = knowledge(machine).await;
        if known != first {
            return Some(format!(
                "machine {index} knows {known:?}, machine 0 {first:?}"
            ));
        }
        if let Some(difference) = disk_difference(machine).await {
            return Some(format!("machine {index}'s disk shows {difference}"));
        }
    }
    None
}

/// Waits up to twenty seconds until `machines` agree, then checks that
/// every error they reported says one of `expected`, which the scenario
/// makes, and that some machine holds the content of every version in the
/// history of every file.
pub async fn converged(machines: &[Machine], expected: &[&str]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while let Some(disagreement) = disagreement(machines).await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the group never converged: {disagreement}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for (index, machine) in machines.iter().enumerate() {
        let errors = machine.engine.status().errors;
        assert!(
            errors
                .iter()
                .all(|error| expected.iter().any(|told| error.contains(told))),
            "machine {index} reported {errors:?}"
        );
    }
    let engine = &machines[0].engine;
    for file in engine.list(None).await.unwrap() {
        for version in engine.history(&file.path) {
            let Some(content) = version.content else {
                continue;
            };
            let mut held = false;
            for machine in machines {
                held |= machine.engine.read(&content).await.unwrap().is_some();
            }
            assert!(held, "no machine holds {version:?} of {}", file.path);
        }
    }
}
