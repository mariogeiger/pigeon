//! Suggestions among engines of one group on this host: what the rules
//! leave to the group waits as a suggestion the author's disk keeps, anyone
//! validates or discards it and only the first decision counts, a discarded
//! one brings the group's version back and stays in history, a folder's
//! suggestions are decided together, an outdated one stays decidable, the
//! loser of concurrent edits is suggested, a move is one suggestion that
//! keeps the file's history, an unportable file is validated at another
//! path, and what pigeon 0.6 left becomes writable files and suggestions.

mod common;

use common::{Machine, eventually, group, is_read_only, joined};
use pigeon_core::patch::VersionRef;
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_core::statement::Reason;
use pigeon_sync::SuggestionView;

fn path(text: &str) -> GroupPath {
    GroupPath::parse(text).unwrap()
}

async fn hold(machine: &Machine, pattern: &str) {
    let rule = Rule {
        pattern: pattern.into(),
        cutoff: Cutoff::PlusInfinity,
    };
    machine.engine.set_rule(rule).await.unwrap();
}

/// The statements of every suggestion `machine` shows.
async fn shown(machine: &Machine) -> Vec<VersionRef> {
    machine
        .engine
        .suggestions()
        .await
        .into_iter()
        .map(|view| view.statement)
        .collect()
}

/// Alice's plan, which Bob holds and changes.
async fn bob_suggests_a_plan(machines: &[Machine]) -> SuggestionView {
    let [alice, bob] = machines else {
        unreachable!()
    };
    hold(bob, "+alice/").await;
    alice.edit("+alice/plan.txt", "alice's plan");
    eventually("bob holds the plan", || async {
        bob.read("+alice/plan.txt").is_some()
    })
    .await;
    bob.edit("+alice/plan.txt", "bob's plan");
    eventually("alice sees bob's suggestion", || async {
        alice.engine.suggestions().await.len() == 1
    })
    .await;
    alice.engine.suggestions().await.remove(0)
}

async fn shut_down(machines: Vec<Machine>) {
    for machine in machines {
        let errors = machine.engine.status().await.errors;
        assert!(errors.is_empty(), "{errors:?}");
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edit_outside_the_rules_waits_on_its_disk_until_anyone_validates_it_once() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let suggestion = bob_suggests_a_plan(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    assert_eq!(suggestion.author.as_str(), "bob");
    assert_eq!(suggestion.reason, Reason::OutsideRules);
    let [change] = &suggestion.changes[..] else {
        panic!("one change: {suggestion:?}")
    };
    assert_eq!(change.path, "+alice/plan.txt");
    assert!(!change.outdated);
    assert_eq!(bob.read("+alice/plan.txt").as_deref(), Some("bob's plan"));
    assert_eq!(
        alice.read("+alice/plan.txt").as_deref(),
        Some("alice's plan")
    );
    assert_eq!(alice.engine.history(&path("+alice/plan.txt")).len(), 1);
    let statement = [suggestion.statement.clone()];
    alice.engine.validate(&statement, None).await.unwrap();
    for machine in [alice, bob] {
        eventually("the validated edit lands everywhere", || async {
            machine.read("+alice/plan.txt").as_deref() == Some("bob's plan")
                && machine.engine.suggestions().await.is_empty()
        })
        .await;
    }
    assert_eq!(alice.engine.history(&path("+alice/plan.txt")).len(), 2);
    assert!(
        bob.engine.discard(&statement).await.is_err(),
        "decided once"
    );
    assert!(alice.engine.validate(&statement, None).await.is_err());
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_discarded_suggestion_brings_the_group_version_back_and_stays_in_history() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let suggestion = bob_suggests_a_plan(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    alice
        .engine
        .discard(std::slice::from_ref(&suggestion.statement))
        .await
        .unwrap();
    eventually("bob's disk shows the group's version again", || async {
        bob.read("+alice/plan.txt").as_deref() == Some("alice's plan")
            && bob.engine.suggestions().await.is_empty()
    })
    .await;
    let history = bob.engine.history(&suggestion.statement.path);
    assert_eq!(history.len(), 2, "{history:?}");
    let content = suggestion.changes[0].content.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        bob.engine.read(&content).await.unwrap().as_deref(),
        Some(&b"bob's plan"[..]),
        "the history keeps the discarded content"
    );
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folders_suggestions_are_validated_together_deletions_included() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    hold(bob, "shared/").await;
    alice.edit("shared/a.txt", "a");
    alice.edit("shared/b.txt", "b");
    eventually("bob holds the published drops", || async {
        bob.read("shared/a.txt").is_some() && bob.read("shared/b.txt").is_some()
    })
    .await;
    assert!(!is_read_only(&alice.file("shared/a.txt")));
    assert!(!is_read_only(&bob.file("shared/a.txt")));
    bob.edit("shared/a.txt", "bob's a");
    std::fs::remove_file(bob.file("shared/b.txt")).unwrap();
    eventually("alice sees both suggestions", || async {
        alice.engine.suggestions().await.len() == 2
    })
    .await;
    assert_eq!(
        bob.read("shared/b.txt"),
        None,
        "bob's disk keeps the deletion"
    );
    assert_eq!(alice.read("shared/b.txt").as_deref(), Some("b"));
    alice
        .engine
        .validate(&shown(alice).await, None)
        .await
        .unwrap();
    for machine in [alice, bob] {
        eventually("both suggestions land everywhere", || async {
            machine.read("shared/a.txt").as_deref() == Some("bob's a")
                && machine.read("shared/b.txt").is_none()
                && machine.engine.suggestions().await.is_empty()
        })
        .await;
    }
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_outdated_suggestion_stays_on_its_disk_and_decidable() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let suggestion = bob_suggests_a_plan(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    alice.edit("+alice/plan.txt", "alice's second plan");
    eventually("the suggestion is outdated", || async {
        bob.engine
            .suggestions()
            .await
            .first()
            .is_some_and(|view| view.changes[0].outdated)
    })
    .await;
    assert_eq!(bob.read("+alice/plan.txt").as_deref(), Some("bob's plan"));
    alice
        .engine
        .validate(&[suggestion.statement], None)
        .await
        .unwrap();
    for machine in [alice, bob] {
        eventually("the outdated suggestion lands everywhere", || async {
            machine.read("+alice/plan.txt").as_deref() == Some("bob's plan")
        })
        .await;
    }
    assert_eq!(alice.engine.history(&path("+alice/plan.txt")).len(), 3);
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn of_two_concurrent_edits_the_later_wins_and_the_other_is_suggested_from_its_disk() {
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
    eventually(
        "one edit wins and the other's disk keeps it as a suggestion",
        || async {
            let views = laptop.engine.suggestions().await;
            let [view] = &views[..] else {
                return false;
            };
            let (loser, lost, winner, won) = if view.machine == laptop.engine.machine() {
                (laptop, "laptop", desktop, "desktop")
            } else {
                (desktop, "desktop", laptop, "laptop")
            };
            view.reason == Reason::Superseded
                && desktop.engine.suggestions().await.len() == 1
                && loser.read("+alice/todo.txt").as_deref() == Some(lost)
                && winner.read("+alice/todo.txt").as_deref() == Some(won)
        },
    )
    .await;
    let views = laptop.engine.suggestions().await;
    let loser = if views[0].machine == laptop.engine.machine() {
        laptop
    } else {
        desktop
    };
    loser.engine.discard(&shown(loser).await).await.unwrap();
    eventually("both machines agree", || async {
        let text = laptop.read("+alice/todo.txt");
        text.is_some() && text == desktop.read("+alice/todo.txt")
    })
    .await;
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_outside_the_rules_is_one_suggestion_that_keeps_the_history() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    hold(bob, "+alice/").await;
    alice.edit("+alice/old.txt", "moving text");
    eventually("bob holds the file", || async {
        bob.read("+alice/old.txt").is_some()
    })
    .await;
    std::fs::rename(bob.file("+alice/old.txt"), bob.file("+alice/new.txt")).unwrap();
    eventually("alice sees one suggestion of both halves", || async {
        alice
            .engine
            .suggestions()
            .await
            .first()
            .is_some_and(|view| view.changes.len() == 2)
    })
    .await;
    let views = alice.engine.suggestions().await;
    assert_eq!(views.len(), 1, "{views:?}");
    let moved = views[0]
        .changes
        .iter()
        .find(|change| change.path == "+alice/new.txt")
        .unwrap();
    assert_eq!(
        moved.continues.as_ref().map(|from| from.path.as_str()),
        Some("+alice/old.txt")
    );
    assert_eq!(bob.read("+alice/new.txt").as_deref(), Some("moving text"));
    assert_eq!(bob.read("+alice/old.txt"), None);
    alice
        .engine
        .validate(&shown(alice).await, None)
        .await
        .unwrap();
    eventually("the move lands on alice's disk", || async {
        alice.read("+alice/new.txt").as_deref() == Some("moving text")
            && alice.read("+alice/old.txt").is_none()
    })
    .await;
    let history = alice.engine.history(&path("+alice/new.txt"));
    assert_eq!(history.len(), 2, "{history:?}");
    assert_eq!(history[0].path, path("+alice/old.txt"));
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unportable_file_is_validated_at_another_path() {
    let machines = group(&["alice"]).await;
    joined(&machines).await;
    let alice = &machines[0];
    alice.edit("shared/what?.txt", "odd");
    eventually("the unportable file is suggested", || async {
        alice
            .engine
            .suggestions()
            .await
            .first()
            .is_some_and(|view| matches!(view.reason, Reason::Unportable(_)))
    })
    .await;
    let statement = shown(alice).await;
    assert_eq!(
        alice.engine.suggestions().await[0].changes[0].path,
        "shared/what?.txt"
    );
    assert!(alice.engine.validate(&statement, None).await.is_err());
    let to = path("shared/what.txt");
    alice.engine.validate(&statement, Some(&to)).await.unwrap();
    eventually("the file moves to its portable path", || async {
        alice.read("shared/what.txt").as_deref() == Some("odd")
            && alice.read("shared/what?.txt").is_none()
    })
    .await;
    shut_down(machines).await;
}
