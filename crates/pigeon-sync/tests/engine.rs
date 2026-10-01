//! Engines of one group on this host: files reach the machines that hold
//! them, edits nobody may publish are set aside, undone and shown to the
//! group, which resolves them by request, drop files freeze and change
//! through requests, anyone decides a proposal once, concurrent edits of
//! one member keep the later, a taken name joins nothing, edits through
//! actions become requests, one per change, which the owner's machine
//! applies at once when the owner asks, the quota drops history, keeping
//! history keeps the past versions of others' files too, and every machine
//! follows the relay the group names.

mod common;

use common::{eventually, group, is_read_only, joined};
use pigeon_core::path::GroupPath;
use pigeon_core::retention::Retention;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_core::statement::{Mode, Reason};
use pigeon_net::relay::{relay_url, serve_relay};
use pigeon_sync::{Edit, JoinState, Waits};

fn rule(pattern: &str, cutoff: Cutoff) -> Rule {
    Rule {
        pattern: pattern.into(),
        cutoff,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_personal_file_reaches_the_machines_that_hold_it() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    alice.edit("+alice/notes.txt", "one");
    eventually("bob sees alice's file in the ledger", || async {
        bob.engine
            .history(&GroupPath::parse("+alice/notes.txt").unwrap())
            .len()
            == 1
    })
    .await;
    assert_eq!(
        bob.read("+alice/notes.txt"),
        None,
        "bob does not hold +alice yet"
    );
    bob.engine
        .set_rule(rule("+alice/", Cutoff::PlusInfinity))
        .await
        .unwrap();
    eventually("bob holds alice's file", || async {
        bob.read("+alice/notes.txt").as_deref() == Some("one")
    })
    .await;
    assert!(is_read_only(&bob.file("+alice/notes.txt")));
    assert!(!is_read_only(&alice.file("+alice/notes.txt")));
    assert!(bob.read(".pigeon/members/alice").is_some());

    alice.edit("+alice/notes.txt", "two");
    eventually("bob follows alice's edit", || async {
        bob.read("+alice/notes.txt").as_deref() == Some("two")
    })
    .await;
    std::fs::remove_file(alice.file("+alice/notes.txt")).unwrap();
    eventually("bob follows alice's deletion", || async {
        bob.read("+alice/notes.txt").is_none()
    })
    .await;

    alice.edit("+alice/kept.txt", "kept");
    eventually("bob holds the new file", || async {
        bob.read("+alice/kept.txt").is_some()
    })
    .await;
    bob.engine
        .set_rule(rule("+alice/", Cutoff::MinusInfinity))
        .await
        .unwrap();
    assert_eq!(
        bob.read("+alice/kept.txt"),
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
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.engine
        .set_rule(rule("+alice/", Cutoff::PlusInfinity))
        .await
        .unwrap();
    alice.edit("+alice/plan.txt", "alice's plan");
    eventually("bob holds the plan", || async {
        bob.read("+alice/plan.txt").is_some()
    })
    .await;
    bob.edit("+alice/plan.txt", "bob's plan");
    eventually("bob's edit is set aside and undone", || async {
        bob.read("+alice/plan.txt").as_deref() == Some("alice's plan")
            && !bob.engine.waiting_changes().await.is_empty()
    })
    .await;
    let aside = bob.engine.waiting_changes().await;
    assert_eq!(aside.len(), 1);
    assert!(
        matches!(
            &aside[0].waits,
            Waits::SetAside {
                reason: Reason::NotWritable,
                ..
            }
        ),
        "{aside:?}"
    );
    assert_eq!(aside[0].path, "+alice/plan.txt");
    assert_eq!(aside[0].owner.as_str(), "alice");
    eventually("alice sees bob's item", || async {
        alice.engine.waiting_changes().await.len() == 1
    })
    .await;
    bob.engine
        .apply_change(&aside[0].entry, "my version")
        .await
        .unwrap();
    for machine in [alice, bob] {
        eventually(
            "the forced request lands everywhere and resolves the item",
            || async {
                machine.read("+alice/plan.txt").as_deref() == Some("bob's plan")
                    && machine.engine.waiting_changes().await.is_empty()
            },
        )
        .await;
    }
    let history = alice.engine.history(&path("+alice/plan.txt"));
    assert!(history[1].applies.is_some(), "{history:?}");
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_drop_file_freezes_and_changes_by_accepted_request() {
    let machines = group(&["alice", "bob"]).await;
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
        !bob.engine.waiting_changes().await.is_empty()
    })
    .await;
    let entry = bob.engine.waiting_changes().await[0].entry.clone();
    let paths = bob.engine.ask_change(&entry, "lighter").await.unwrap();
    eventually("alice sees the proposal alone", || async {
        let waiting = alice.engine.waiting_changes().await;
        waiting.len() == 1 && waiting[0].waits == Waits::Proposed
    })
    .await;
    assert_eq!(alice.read("shared/menu.txt").as_deref(), Some("soup"));
    alice.engine.apply_change(&paths[0], "").await.unwrap();
    for machine in [alice, bob] {
        eventually("the accepted request lands everywhere", || async {
            machine.read("shared/menu.txt").as_deref() == Some("salad")
        })
        .await;
    }
    let history = bob.engine.history(&path("shared/menu.txt"));
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].owner.as_str(), "alice");
    assert_eq!(history[1].applies, Some(paths[0].clone()));
    assert!(alice.engine.waiting_changes().await.is_empty());
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn anyone_decides_a_proposal_once() {
    let machines = group(&["alice", "bob", "carol"]).await;
    joined(&machines).await;
    let [alice, bob, carol] = &machines[..] else {
        unreachable!()
    };
    alice.edit("+alice/list.txt", "milk");
    eventually("everyone knows the list", || async {
        carol.engine.history(&path("+alice/list.txt")).len() == 1
    })
    .await;
    let write = |text: &str| Edit::Write {
        path: path("+alice/list.txt"),
        bytes: text.as_bytes().to_vec(),
    };
    let asked = bob
        .engine
        .edit(vec![write("milk, bread")], Mode::Propose, "bread")
        .await
        .unwrap();
    let [proposal] = &asked.requests[..] else {
        panic!("one request: {asked:?}")
    };
    eventually("carol sees bob's proposal to alice", || async {
        carol.engine.waiting_changes().await.len() == 1
    })
    .await;
    let waiting = &carol.engine.waiting_changes().await[0];
    assert_eq!(
        (waiting.author.as_str(), waiting.owner.as_str()),
        ("bob", "alice")
    );
    carol.engine.apply_change(proposal, "").await.unwrap();
    eventually("alice's machine applies what carol accepted", || async {
        alice.read("+alice/list.txt").as_deref() == Some("milk, bread")
    })
    .await;
    eventually("alice hears the decision", || async {
        alice.engine.waiting_changes().await.is_empty()
    })
    .await;
    let again = alice.engine.discard_change(proposal, Mode::Force).await;
    assert!(again.is_err(), "the first decision is final");
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_edits_of_one_member_keep_the_later_and_set_aside_the_other() {
    let machines = group(&["alice", "alice"]).await;
    joined(&machines).await;
    let [laptop, desktop] = &machines[..] else {
        unreachable!()
    };
    laptop.edit("+alice/todo.txt", "base");
    eventually("the desktop holds the file", || async {
        desktop.read("+alice/todo.txt").as_deref() == Some("base")
    })
    .await;
    laptop.edit("+alice/todo.txt", "laptop");
    desktop.edit("+alice/todo.txt", "desktop");
    eventually("both machines agree and the loser is set aside", || async {
        let (one, two) = (
            laptop.read("+alice/todo.txt"),
            desktop.read("+alice/todo.txt"),
        );
        let (aside_one, aside_two) = (
            laptop.engine.waiting_changes().await,
            desktop.engine.waiting_changes().await,
        );
        one.is_some()
            && one == two
            && one.as_deref() != Some("base")
            && aside_one.len() == 1
            && aside_two.len() == 1
    })
    .await;
    let aside = laptop.engine.waiting_changes().await;
    assert!(
        matches!(
            &aside[0].waits,
            Waits::SetAside {
                reason: Reason::Superseded,
                ..
            }
        ),
        "{aside:?}"
    );
    for machine in machines {
        assert!(machine.engine.status().await.errors.is_empty());
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_machines_with_one_name_are_one_member() {
    let machines = group(&["alice", "alice"]).await;
    joined(&machines).await;
    for machine in &machines {
        assert_eq!(machine.engine.members().len(), 1);
    }
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_machine_resumes_without_publishing_again() {
    let mut machines = group(&["alice"]).await;
    joined(&machines).await;
    let alice = machines.remove(0);
    alice.edit("+alice/a.txt", "a");
    eventually("the file is published", || async {
        alice
            .engine
            .history(&GroupPath::parse("+alice/a.txt").unwrap())
            .len()
            == 1
    })
    .await;
    let patches = alice.engine.status().await.patches;
    let alice = alice.restart().await;
    assert_eq!(alice.engine.status().await.join, JoinState::Joined);
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_eq!(alice.engine.status().await.patches, patches);
    assert_eq!(alice.read("+alice/a.txt").as_deref(), Some("a"));
    alice.edit("+alice/a.txt", "b");
    eventually("an edit after the restart is published", || async {
        alice
            .engine
            .history(&GroupPath::parse("+alice/a.txt").unwrap())
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
async fn edits_become_requests_which_the_owners_machine_applies_at_once_when_asked_by_the_owner() {
    let machines = group(&["alice", "bob"]).await;
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
                write("+alice/docs/a.txt", "a"),
                write("+alice/docs/b.txt", "b"),
            ],
            Mode::Propose,
            "",
        )
        .await
        .unwrap();
    assert!(written.published.is_empty());
    assert_eq!(written.requests.len(), 2, "one request per change");
    eventually("alice's own requests apply at once", || async {
        alice.read("+alice/docs/b.txt").as_deref() == Some("b")
    })
    .await;
    let renamed = alice
        .engine
        .edit(
            vec![Edit::Rename {
                from: path("+alice/docs"),
                to: path("+alice/papers"),
            }],
            Mode::Propose,
            "",
        )
        .await
        .unwrap();
    assert_eq!(renamed.requests.len(), 4);
    eventually("the folder moves on alice's disk", || async {
        alice.read("+alice/papers/a.txt").as_deref() == Some("a")
            && alice.read("+alice/docs/a.txt").is_none()
    })
    .await;
    assert!(alice.engine.waiting_changes().await.is_empty());

    eventually("bob sees the renamed folder", || async {
        bob.engine
            .list(Some(&path("+alice/papers")))
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
                path: path("+alice/papers"),
            }],
            Mode::Force,
            "tidying",
        )
        .await
        .unwrap();
    assert!(deleted.published.is_empty());
    assert_eq!(deleted.requests.len(), 2);
    eventually("alice applies the forced deletion", || async {
        alice.read("+alice/papers/a.txt").is_none() && alice.read("+alice/papers/b.txt").is_none()
    })
    .await;
    let missing = alice
        .engine
        .edit(
            vec![Edit::Delete {
                path: path("+alice/papers"),
            }],
            Mode::Propose,
            "",
        )
        .await
        .unwrap_err();
    assert_eq!(missing.to_string(), "no file at +alice/papers");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_quota_drops_past_versions_and_keeps_current_ones() {
    let machines = group(&["alice"]).await;
    joined(&machines).await;
    let alice = &machines[0];
    for text in ["one", "two"] {
        let edit = Edit::Write {
            path: path("+alice/a.txt"),
            bytes: text.as_bytes().to_vec(),
        };
        alice
            .engine
            .edit(vec![edit], Mode::Propose, "")
            .await
            .unwrap();
        eventually("alice's edit applies", || async {
            alice.read("+alice/a.txt").as_deref() == Some(text)
        })
        .await;
    }
    let history = alice.engine.history(&path("+alice/a.txt"));
    let [old, current] = &history[..] else {
        panic!("two versions: {history:?}")
    };
    let (old, current) = (old.content.unwrap(), current.content.unwrap());
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(alice.engine.read(&old).await.unwrap().is_some());
    let retention = Retention {
        quota_percent: 0,
        ..alice.engine.retention().await
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
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.engine
        .set_rule(rule("+alice/", Cutoff::PlusInfinity))
        .await
        .unwrap();
    let current = bob.engine.retention().await;
    let keep = |everything| Retention {
        everything,
        ..current
    };
    bob.engine.set_retention(&keep(true)).await.unwrap();
    for text in ["one", "two"] {
        alice.edit("+alice/a.txt", text);
        eventually("bob follows alice", || async {
            bob.read("+alice/a.txt").as_deref() == Some(text)
        })
        .await;
    }
    let history = bob.engine.history(&path("+alice/a.txt"));
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

#[tokio::test(flavor = "multi_thread")]
async fn every_machine_follows_the_relay_the_group_names() {
    let mut machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let server = serve_relay("127.0.0.1:0".parse().unwrap(), None)
        .await
        .unwrap();
    let url = relay_url(&server, "127.0.0.1").unwrap().to_string();
    let alice = &machines[0].engine;
    let wrong = alice.set_relay(Some("no url")).await.unwrap_err();
    assert_eq!(wrong.to_string(), "no url is no relay URL");
    alice.set_relay(Some(&url)).await.unwrap();
    for machine in &machines {
        eventually("the machine reaches the group's relay", || async {
            machine.engine.status().await.relay.as_deref() == Some(url.as_str())
        })
        .await;
    }
    let bob = machines.pop().unwrap().restart().await;
    eventually("a restarted machine follows it at once", || async {
        bob.engine.status().await.relay.as_deref() == Some(url.as_str())
    })
    .await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_root_reached_through_a_link_syncs_both_ways() {
    let machines = group(&["alice"]).await;
    joined(&machines).await;
    let alice = &machines[0];
    let bob = alice
        .join_through_link(&alice.engine.group_key(), "bob")
        .await;
    joined(std::slice::from_ref(&bob)).await;
    for (machine, folder) in [(alice, "+bob/"), (&bob, "+alice/")] {
        machine
            .engine
            .set_rule(rule(folder, Cutoff::PlusInfinity))
            .await
            .unwrap();
    }
    alice.edit("+alice/notes.txt", "from alice");
    eventually("bob receives through his link", || async {
        bob.read("+alice/notes.txt").as_deref() == Some("from alice")
    })
    .await;
    bob.edit("+bob/notes.txt", "from bob");
    eventually("bob's edit through his link is published", || async {
        alice.read("+bob/notes.txt").as_deref() == Some("from bob")
    })
    .await;
    assert!(bob.engine.status().await.errors.is_empty());
    bob.engine.shutdown().await.unwrap();
    for machine in machines {
        machine.engine.shutdown().await.unwrap();
    }
}
