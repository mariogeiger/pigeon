//! Actions among engines of one group on this host: they publish at once
//! whoever owns the files, a renamed folder and a file moved on disk keep
//! their history, and restoring a pattern makes its files hold what they
//! held at a time with new versions.

mod common;

use common::{Machine, eventually, group, joined};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_sync::Edit;

fn path(text: &str) -> GroupPath {
    GroupPath::parse(text).unwrap()
}

fn write(name: &str, text: &str) -> Edit {
    Edit::Write {
        path: path(name),
        bytes: text.as_bytes().to_vec(),
    }
}

async fn hold(machine: &Machine, pattern: &str) {
    let rule = Rule {
        pattern: pattern.into(),
        cutoff: Cutoff::PlusInfinity,
    };
    machine.engine.set_rule(rule).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn actions_publish_at_once_whoever_owns_the_files() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    let written = alice
        .engine
        .edit(vec![
            write("+alice/docs/a.txt", "a"),
            write("+alice/docs/b.txt", "b"),
        ])
        .await
        .unwrap();
    assert_eq!(written.published.len(), 2);
    assert_eq!(alice.read("+alice/docs/a.txt").as_deref(), Some("a"));
    eventually("bob knows alice's files", || async {
        bob.engine.history(&path("+alice/docs/b.txt")).len() == 1
    })
    .await;
    bob.engine
        .edit(vec![write("+alice/docs/a.txt", "bob's a")])
        .await
        .unwrap();
    eventually("bob's action lands on alice's disk", || async {
        alice.read("+alice/docs/a.txt").as_deref() == Some("bob's a")
    })
    .await;
    let deleted = bob
        .engine
        .edit(vec![Edit::Delete {
            path: path("+alice/docs"),
        }])
        .await
        .unwrap();
    assert_eq!(deleted.published.len(), 2);
    eventually("bob's deletion lands on alice's disk", || async {
        alice.read("+alice/docs/a.txt").is_none() && alice.read("+alice/docs/b.txt").is_none()
    })
    .await;
    let missing = alice
        .engine
        .edit(vec![Edit::Delete {
            path: path("+alice/docs"),
        }])
        .await
        .unwrap_err();
    assert_eq!(missing.to_string(), "no file at +alice/docs");
    assert!(alice.engine.suggestions().await.is_empty());
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn renamed_and_moved_files_keep_their_history() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    hold(bob, "+alice/").await;
    alice
        .engine
        .edit(vec![write("+alice/docs/a.txt", "a")])
        .await
        .unwrap();
    alice
        .engine
        .edit(vec![Edit::Rename {
            from: path("+alice/docs"),
            to: path("+alice/papers"),
        }])
        .await
        .unwrap();
    assert_eq!(alice.read("+alice/papers/a.txt").as_deref(), Some("a"));
    assert_eq!(alice.read("+alice/docs/a.txt"), None);
    std::fs::rename(
        alice.file("+alice/papers/a.txt"),
        alice.file("+alice/papers/z.txt"),
    )
    .unwrap();
    eventually("bob follows both moves", || async {
        bob.read("+alice/papers/z.txt").as_deref() == Some("a")
            && bob.read("+alice/papers/a.txt").is_none()
    })
    .await;
    let history = bob.engine.history(&path("+alice/papers/z.txt"));
    let paths: Vec<&str> = history
        .iter()
        .map(|version| version.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "+alice/docs/a.txt",
            "+alice/papers/a.txt",
            "+alice/papers/z.txt"
        ]
    );
    assert!(alice.engine.suggestions().await.is_empty());
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn restoring_a_folder_brings_back_what_it_held_with_new_versions() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    alice
        .engine
        .edit(vec![
            write("+alice/notes/kept.txt", "first"),
            write("+alice/notes/gone.txt", "gone"),
        ])
        .await
        .unwrap();
    let then = alice.engine.now();
    alice
        .engine
        .edit(vec![
            write("+alice/notes/kept.txt", "second"),
            write("+alice/notes/new.txt", "new"),
            Edit::Delete {
                path: path("+alice/notes/gone.txt"),
            },
        ])
        .await
        .unwrap();
    eventually("bob knows the latest notes", || async {
        bob.engine.history(&path("+alice/notes/kept.txt")).len() == 2
    })
    .await;
    let restored = bob.engine.restore("+alice/notes/", then).await.unwrap();
    assert_eq!(restored.published.len(), 3, "{restored:?}");
    eventually("alice's disk holds the notes as they were", || async {
        alice.read("+alice/notes/kept.txt").as_deref() == Some("first")
            && alice.read("+alice/notes/gone.txt").as_deref() == Some("gone")
            && alice.read("+alice/notes/new.txt").is_none()
    })
    .await;
    assert_eq!(
        alice.engine.history(&path("+alice/notes/kept.txt")).len(),
        3
    );
    assert!(bob.engine.restore("+alice/notes/", then).await.is_err());
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}
