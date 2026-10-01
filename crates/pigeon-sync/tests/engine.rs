//! Engines of one group on this host: files reach the machines that hold
//! them, edits nobody may publish are set aside and undone, drop files
//! freeze and change through requests, concurrent edits of one member keep
//! the later, a taken name joins nothing, and edits through actions publish
//! or request each file under its own rule, the quota drops history, and
//! keeping history keeps the past versions of others' files too.

mod common;

use common::{eventually, group, is_read_only, joined};
use pigeon_core::path::GroupPath;
use pigeon_core::retention::Retention;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_core::statement::{Decision, Mode};
use pigeon_store::aside::Reason;
use pigeon_sync::{Edit, JoinState};

fn rule(pattern: &str, cutoff: Cutoff) -> Rule {
    Rule {
        pattern: pattern.into(),
        cutoff,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_personal_file_reaches_the_machines_that_hold_it() {
    let machines = group(&[("alice", "a"), ("bob", "b")]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    alice.edit("@alice/notes.txt", "one");
    eventually("bob sees alice's file in the ledger", || async {
        bob.engine
            .history(&GroupPath::parse("@alice/notes.txt").unwrap())
            .len()
            == 1
    })
    .await;
    assert_eq!(
        bob.read("@alice/notes.txt"),
        None,
        "bob does not hold @alice yet"
    );
    bob.engine
        .set_rule(rule("@alice/", Cutoff::PlusInfinity))
        .await
        .unwrap();
    eventually("bob holds alice's file", || async {
        bob.read("@alice/notes.txt").as_deref() == Some("one")
    })
    .await;
    assert!(is_read_only(&bob.file("@alice/notes.txt")));
    assert!(!is_read_only(&alice.file("@alice/notes.txt")));
    assert!(bob.read(".pigeon/members/alice").is_some());

    alice.edit("@alice/notes.txt", "two");
    eventually("bob follows alice's edit", || async {
        bob.read("@alice/notes.txt").as_deref() == Some("two")
    })
    .await;
    std::fs::remove_file(alice.file("@alice/notes.txt")).unwrap();
    eventually("bob follows alice's deletion", || async {
        bob.read("@alice/notes.txt").is_none()
    })
    .await;

    alice.edit("@alice/kept.txt", "kept");
    eventually("bob holds the new file", || async {
        bob.read("@alice/kept.txt").is_some()
    })
    .await;
    bob.engine
        .set_rule(rule("@alice/", Cutoff::MinusInfinity))
        .await
        .unwrap();
    assert_eq!(
        bob.read("@alice/kept.txt"),
        None,
        "a released file leaves the disk"
    );
    for machine in machines {
        assert!(machine.engine.status().await.errors.is_empty());
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edit_that_may_not_be_published_is_set_aside_then_forced() {
    let machines = group(&[("alice", "a"), ("bob", "b")]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.engine
        .set_rule(rule("@alice/", Cutoff::PlusInfinity))
        .await
        .unwrap();
    alice.edit("@alice/plan.txt", "alice's plan");
    eventually("bob holds the plan", || async {
        bob.read("@alice/plan.txt").is_some()
    })
    .await;
    bob.edit("@alice/plan.txt", "bob's plan");
    eventually("bob's edit is set aside and undone", || async {
        bob.read("@alice/plan.txt").as_deref() == Some("alice's plan")
            && !bob.engine.aside().unwrap().is_empty()
    })
    .await;
    let aside = bob.engine.aside().unwrap();
    assert_eq!(aside.len(), 1);
    assert_eq!(aside[0].item.reason, Reason::NotWritable);
    assert_eq!(aside[0].item.path, "@alice/plan.txt");
    bob.engine
        .request_aside(aside[0].id, Mode::Force, "my version")
        .await
        .unwrap();
    assert!(bob.engine.aside().unwrap().is_empty());
    for machine in [alice, bob] {
        eventually("the forced request lands everywhere", || async {
            machine.read("@alice/plan.txt").as_deref() == Some("bob's plan")
        })
        .await;
    }
    let requests = alice.engine.requests().await;
    assert_eq!(requests.len(), 1);
    assert!(requests[0].applied);
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_drop_file_freezes_and_changes_by_accepted_request() {
    let machines = group(&[("alice", "a"), ("bob", "b")]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.engine
        .set_rule(rule("shared/", Cutoff::PlusInfinity))
        .await
        .unwrap();
    alice.edit("shared/menu.txt", "soup");
    eventually("bob holds the drop file", || async {
        bob.read("shared/menu.txt").as_deref() == Some("soup")
    })
    .await;
    eventually("alice's published draft freezes", || async {
        is_read_only(&alice.file("shared/menu.txt"))
    })
    .await;
    assert!(is_read_only(&bob.file("shared/menu.txt")));
    bob.edit("shared/menu.txt", "salad");
    eventually("bob's edit is set aside", || async {
        !bob.engine.aside().unwrap().is_empty()
    })
    .await;
    let id = bob.engine.aside().unwrap()[0].id;
    let paths = bob
        .engine
        .request_aside(id, Mode::Propose, "lighter")
        .await
        .unwrap();
    eventually("alice sees the proposal", || async {
        alice.engine.requests().await.len() == 1
    })
    .await;
    assert_eq!(alice.read("shared/menu.txt").as_deref(), Some("soup"));
    alice
        .engine
        .decide(&paths[0], Decision::Accept)
        .await
        .unwrap();
    for machine in [alice, bob] {
        eventually("the accepted request lands everywhere", || async {
            machine.read("shared/menu.txt").as_deref() == Some("salad")
        })
        .await;
    }
    let history = bob
        .engine
        .history(&GroupPath::parse("shared/menu.txt").unwrap());
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].owner.as_str(), "alice");
    assert_eq!(history[1].applies, Some(paths[0].clone()));
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_edits_of_one_member_keep_the_later_and_set_aside_the_other() {
    let machines = group(&[("alice", "a"), ("alice", "a")]).await;
    joined(&machines).await;
    let [laptop, desktop] = &machines[..] else {
        unreachable!()
    };
    laptop.edit("@alice/todo.txt", "base");
    eventually("the desktop holds the file", || async {
        desktop.read("@alice/todo.txt").as_deref() == Some("base")
    })
    .await;
    laptop.edit("@alice/todo.txt", "laptop");
    desktop.edit("@alice/todo.txt", "desktop");
    eventually("both machines agree and the loser is set aside", || async {
        let (one, two) = (
            laptop.read("@alice/todo.txt"),
            desktop.read("@alice/todo.txt"),
        );
        let set_aside =
            laptop.engine.aside().unwrap().len() + desktop.engine.aside().unwrap().len();
        one.is_some() && one == two && one.as_deref() != Some("base") && set_aside == 1
    })
    .await;
    let aside: Vec<_> = [laptop, desktop]
        .iter()
        .flat_map(|machine| machine.engine.aside().unwrap())
        .collect();
    assert_eq!(aside[0].item.reason, Reason::Superseded);
    for machine in machines {
        assert!(machine.engine.status().await.errors.is_empty());
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_taken_by_another_password_joins_nothing() {
    let machines = group(&[("alice", "a"), ("alice", "other")]).await;
    for machine in &machines {
        eventually("the claim is settled", || async {
            machine.engine.status().await.join != JoinState::Pending
        })
        .await;
    }
    eventually("both machines agree on who holds the name", || async {
        let mut states = Vec::new();
        for machine in &machines {
            states.push(machine.engine.status().await.join);
        }
        states
            .iter()
            .filter(|state| **state == JoinState::Joined)
            .count()
            == 1
            && states
                .iter()
                .filter(|state| matches!(state, JoinState::Taken(_)))
                .count()
                == 1
    })
    .await;
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_machine_resumes_without_publishing_again() {
    let mut machines = group(&[("alice", "a")]).await;
    joined(&machines).await;
    let alice = machines.remove(0);
    alice.edit("@alice/a.txt", "a");
    eventually("the file is published", || async {
        alice
            .engine
            .history(&GroupPath::parse("@alice/a.txt").unwrap())
            .len()
            == 1
    })
    .await;
    let patches = alice.engine.status().await.patches;
    let alice = alice.restart().await;
    assert_eq!(alice.engine.status().await.join, JoinState::Joined);
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_eq!(alice.engine.status().await.patches, patches);
    assert_eq!(alice.read("@alice/a.txt").as_deref(), Some("a"));
    alice.edit("@alice/a.txt", "b");
    eventually("an edit after the restart is published", || async {
        alice
            .engine
            .history(&GroupPath::parse("@alice/a.txt").unwrap())
            .len()
            == 2
    })
    .await;
    alice.engine.shutdown().await.unwrap();
}

fn path(text: &str) -> GroupPath {
    GroupPath::parse(text).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_publish_what_the_member_writes_and_request_the_rest() {
    let machines = group(&[("alice", "a"), ("bob", "b")]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    let write = |name: &str, text: &str| Edit::Write {
        path: path(name),
        bytes: text.as_bytes().to_vec(),
    };
    let written = alice
        .engine
        .edit(
            vec![
                write("@alice/docs/a.txt", "a"),
                write("@alice/docs/b.txt", "b"),
            ],
            Mode::Propose,
            "",
        )
        .await
        .unwrap();
    assert_eq!(written.published.len(), 2);
    let renamed = alice
        .engine
        .edit(
            vec![Edit::Rename {
                from: path("@alice/docs"),
                to: path("@alice/papers"),
            }],
            Mode::Propose,
            "",
        )
        .await
        .unwrap();
    assert_eq!(renamed.published.len(), 4);
    assert!(renamed.requests.is_empty());
    assert_eq!(alice.read("@alice/papers/a.txt").as_deref(), Some("a"));
    assert!(alice.read("@alice/docs/a.txt").is_none());

    eventually("bob sees the renamed folder", || async {
        bob.engine
            .list(Some(&path("@alice/papers")))
            .await
            .unwrap()
            .len()
            == 2
    })
    .await;
    let deleted = bob
        .engine
        .edit(
            vec![Edit::Delete {
                path: path("@alice/papers"),
            }],
            Mode::Force,
            "tidying",
        )
        .await
        .unwrap();
    assert!(deleted.published.is_empty());
    assert_eq!(deleted.requests.len(), 1);
    eventually("alice applies the forced deletion", || async {
        alice.read("@alice/papers/a.txt").is_none() && alice.read("@alice/papers/b.txt").is_none()
    })
    .await;
    let missing = alice
        .engine
        .edit(
            vec![Edit::Delete {
                path: path("@alice/papers"),
            }],
            Mode::Propose,
            "",
        )
        .await
        .unwrap_err();
    assert_eq!(missing.to_string(), "no file at @alice/papers");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_quota_drops_past_versions_and_keeps_current_ones() {
    let machines = group(&[("alice", "a")]).await;
    joined(&machines).await;
    let alice = &machines[0];
    for text in ["one", "two"] {
        let edit = Edit::Write {
            path: path("@alice/a.txt"),
            bytes: text.as_bytes().to_vec(),
        };
        alice
            .engine
            .edit(vec![edit], Mode::Propose, "")
            .await
            .unwrap();
    }
    let history = alice.engine.history(&path("@alice/a.txt"));
    let [old, current] = &history[..] else {
        panic!("two versions: {history:?}")
    };
    let (old, current) = (old.content.unwrap(), current.content.unwrap());
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(alice.engine.read(&old).await.unwrap().is_some());
    let retention = Retention {
        quota_percent: 0,
        ..alice.engine.retention().unwrap()
    };
    alice.engine.set_retention(&retention).await.unwrap();
    eventually("the past version goes", || async {
        alice.engine.read(&old).await.unwrap().is_none()
    })
    .await;
    assert_eq!(
        alice.engine.read(&current).await.unwrap().as_deref(),
        Some(&b"two"[..])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn keeping_history_keeps_past_versions_of_others_files() {
    let machines = group(&[("alice", "a"), ("bob", "b")]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.engine
        .set_rule(rule("@alice/", Cutoff::PlusInfinity))
        .await
        .unwrap();
    let keep = |everything| Retention {
        everything,
        ..bob.engine.retention().unwrap()
    };
    bob.engine.set_retention(&keep(true)).await.unwrap();
    for text in ["one", "two"] {
        alice.edit("@alice/a.txt", text);
        eventually("bob follows alice", || async {
            bob.read("@alice/a.txt").as_deref() == Some(text)
        })
        .await;
    }
    let history = bob.engine.history(&path("@alice/a.txt"));
    let old = history[0].content.unwrap();
    bob.engine.set_retention(&keep(true)).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(bob.engine.read(&old).await.unwrap().is_some());
    bob.engine.set_retention(&keep(false)).await.unwrap();
    eventually("without it the past version goes", || async {
        bob.engine.read(&old).await.unwrap().is_none()
    })
    .await;
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_root_reached_through_a_link_syncs_both_ways() {
    let machines = group(&[("alice", "a")]).await;
    joined(&machines).await;
    let alice = &machines[0];
    let bob = alice
        .join_through_link(&alice.engine.group_key(), "bob", "b")
        .await;
    joined(std::slice::from_ref(&bob)).await;
    for (machine, folder) in [(alice, "@bob/"), (&bob, "@alice/")] {
        machine
            .engine
            .set_rule(rule(folder, Cutoff::PlusInfinity))
            .await
            .unwrap();
    }
    alice.edit("@alice/notes.txt", "from alice");
    eventually("bob receives through his link", || async {
        bob.read("@alice/notes.txt").as_deref() == Some("from alice")
    })
    .await;
    bob.edit("@bob/notes.txt", "from bob");
    eventually("bob's edit through his link is published", || async {
        alice.read("@bob/notes.txt").as_deref() == Some("from bob")
    })
    .await;
    assert!(bob.engine.status().await.errors.is_empty());
    bob.engine.shutdown().await.unwrap();
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}
