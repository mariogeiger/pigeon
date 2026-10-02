//! Replacing the whole selection: the preview counts what the draft rules
//! would download, free and pin, and which rule decides each file; the
//! replacement keeps the rules as given, refuses a stale version when given
//! one, and frees only the copies nobody modified. A configuration edited
//! by hand applies when the engine starts, and nothing writes over it
//! before. A file created outside the selection stays through changes of
//! the selection and restarts, which free only what a change unselected.

mod common;

use std::time::Duration;

use common::{Machine, eventually, group_with, joined};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_sync::{Amount, Changed, Delta, DeltaFiles, OwnFreed, Preview, RuleEffect};

fn path(text: &str) -> GroupPath {
    GroupPath::parse(text).unwrap()
}

fn rule(pattern: &str, cutoff: Cutoff) -> Rule {
    Rule {
        pattern: pattern.into(),
        cutoff,
    }
}

fn amount(files: u64, bytes: u64) -> Amount {
    Amount { files, bytes }
}

fn changed(file: &str, size: u64, rule: usize) -> Changed {
    Changed {
        path: path(file),
        size,
        rule: Some(rule),
    }
}

/// The three deltas of a preview, given as their files.
fn deltas(download: Vec<Changed>, free: Vec<Changed>, pin: Vec<Changed>) -> Vec<DeltaFiles> {
    Delta::ALL
        .into_iter()
        .zip([download, free, pin])
        .map(|(delta, largest)| DeltaFiles {
            delta,
            total: Amount {
                files: largest.len() as u64,
                bytes: largest.iter().map(|file| file.size).sum(),
            },
            largest,
        })
        .collect()
}

/// Writes `text` at `file` and publishes it at once.
async fn publish(machine: &Machine, file: &str, text: &str) {
    machine.edit(file, text);
    eventually("the edit waits", || async {
        !machine.engine.pending(None).await.is_empty()
    })
    .await;
    machine.engine.publish(None).await.unwrap();
}

/// Alice and Bob, edits waiting a minute, once Alice published
/// `+alice/a.txt` and `+alice/big.iso` and Bob `+bob/notes.txt`, which he
/// then edits again.
async fn alice_and_bob() -> Vec<Machine> {
    let machines = group_with(&["alice", "bob"], |options| {
        options.settle_personal = Duration::from_secs(60);
    })
    .await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    publish(alice, "+alice/a.txt", "aaaa").await;
    publish(alice, "+alice/big.iso", "0123456789").await;
    publish(bob, "+bob/notes.txt", "notes").await;
    eventually("bob sees alice's files", || async {
        bob.engine.history(&path("+alice/big.iso")).len() == 1
    })
    .await;
    machines
}

/// Waits for Bob's edit of his notes.
async fn edit_notes(bob: &Machine) {
    bob.edit("+bob/notes.txt", "changed");
    eventually("bob's edit waits", || async {
        !bob.engine.pending(None).await.is_empty()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_is_previewed_then_saved_whole_keeping_modified_copies() {
    let machines = alice_and_bob().await;
    let bob = &machines[1];
    let draft = vec![
        rule("*.iso", Cutoff::PlusInfinity),
        rule("+alice/", Cutoff::PlusInfinity),
        rule("*.iso", Cutoff::MinusInfinity),
        rule("+bob/", Cutoff::MinusInfinity),
    ];
    let preview = bob.engine.preview(draft.clone()).await.unwrap();
    let decides = |files, bytes| RuleEffect {
        matches: files,
        decides: amount(files, bytes),
    };
    assert_eq!(
        preview,
        Preview {
            version: preview.version.clone(),
            now: amount(1, 5),
            after: amount(1, 4),
            rules: vec![
                RuleEffect {
                    matches: 1,
                    decides: amount(0, 0),
                },
                RuleEffect {
                    matches: 2,
                    decides: amount(1, 4),
                },
                decides(1, 10),
                decides(1, 5),
            ],
            deltas: deltas(
                vec![changed("+alice/a.txt", 4, 1)],
                vec![changed("+bob/notes.txt", 5, 3)],
                vec![],
            ),
            own_freed: vec![OwnFreed {
                rule: Some(3),
                total: amount(1, 5),
            }],
        }
    );
    edit_notes(bob).await;
    let kept = bob.engine.preview(draft.clone()).await.unwrap();
    assert_eq!(kept.deltas[1].total, amount(0, 0));
    assert_eq!(kept.after, amount(2, 9));
    assert!(
        bob.engine
            .set_selection(draft.clone(), Some("stale"))
            .await
            .is_err()
    );
    bob.engine
        .set_selection(draft.clone(), Some(&preview.version))
        .await
        .unwrap();
    assert_eq!(bob.engine.selection().await, draft);
    eventually("bob holds alice's text", || async {
        bob.read("+alice/a.txt").as_deref() == Some("aaaa")
    })
    .await;
    assert_eq!(bob.read("+bob/notes.txt").as_deref(), Some("changed"));
    assert!(bob.read("+alice/big.iso").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_saved_draft_frees_untouched_copies_and_previews_pins() {
    let machines = alice_and_bob().await;
    let bob = &machines[1];
    edit_notes(bob).await;
    let alice_text = vec![rule("/+alice/a.txt", Cutoff::PlusInfinity)];
    bob.engine.set_selection(alice_text, None).await.unwrap();
    eventually("bob holds alice's text", || async {
        bob.read("+alice/a.txt").as_deref() == Some("aaaa")
    })
    .await;
    let only_iso = vec![rule("/+alice/big.iso", Cutoff::PlusInfinity)];
    let preview = bob.engine.preview(only_iso.clone()).await.unwrap();
    assert_eq!(
        preview.deltas,
        deltas(
            vec![changed("+alice/big.iso", 10, 0)],
            vec![Changed {
                path: path("+alice/a.txt"),
                size: 4,
                rule: None,
            }],
            vec![],
        )
    );
    assert!(preview.own_freed.is_empty());
    bob.engine.set_selection(only_iso, None).await.unwrap();
    assert!(bob.read("+alice/a.txt").is_none());
    eventually("bob holds the iso", || async {
        bob.read("+alice/big.iso").is_some()
    })
    .await;
    assert_eq!(bob.read("+bob/notes.txt").as_deref(), Some("changed"));

    let pinned = vec![rule("+alice/", Cutoff::At(bob.engine.now()))];
    let preview = bob.engine.preview(pinned).await.unwrap();
    assert_eq!(
        preview.deltas,
        deltas(
            vec![changed("+alice/a.txt", 4, 0)],
            vec![],
            vec![changed("+alice/big.iso", 10, 0)],
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_configuration_edited_by_hand_applies_at_restart_and_survives_until_then() {
    let mut machines = alice_and_bob().await;
    let bob = machines.remove(1);
    let alice_text = vec![rule("/+alice/a.txt", Cutoff::PlusInfinity)];
    bob.engine.set_selection(alice_text, None).await.unwrap();
    eventually("bob holds alice's text", || async {
        bob.read("+alice/a.txt").as_deref() == Some("aaaa")
    })
    .await;
    let config = bob.dirs.config_path();
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("\"follow /+alice/a.txt\""), "{text}");
    let edited = text
        .replace("follow /+alice/a.txt", "follow /+alice/big.iso")
        .replace("quota = 20", "quota = 3");
    std::fs::write(&config, &edited).unwrap();
    let refused = bob
        .engine
        .set_rule(rule("/+bob/", Cutoff::PlusInfinity))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("pigeon daemon reload"), "{refused}");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), edited);
    let bob = bob.restart().await;
    eventually("bob holds the iso", || async {
        bob.read("+alice/big.iso").is_some()
    })
    .await;
    assert!(bob.read("+alice/a.txt").is_none());
    assert_eq!(bob.engine.retention().await.quota_percent, 3);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), edited);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_created_outside_the_selection_stays_through_selection_changes_and_restarts() {
    let mut machines = alice_and_bob().await;
    let bob = machines.remove(1);
    publish(&bob, "shared/idea.txt", "idea").await;
    assert_eq!(bob.engine.history(&path("shared/idea.txt")).len(), 1);
    bob.engine
        .set_rule(rule("/+alice/a.txt", Cutoff::PlusInfinity))
        .await
        .unwrap();
    eventually("bob holds alice's text", || async {
        bob.read("+alice/a.txt").as_deref() == Some("aaaa")
    })
    .await;
    assert_eq!(bob.read("shared/idea.txt").as_deref(), Some("idea"));
    let bob = bob.restart().await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(bob.read("shared/idea.txt").as_deref(), Some("idea"));
    assert_eq!(bob.read("+alice/a.txt").as_deref(), Some("aaaa"));
}
