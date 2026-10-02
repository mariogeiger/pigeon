//! What did not last: a change a machine published that lost to a
//! concurrent one becomes a suggestion of that machine once, whatever its
//! disk holds, and a suggestion the group decided never comes back through
//! a machine that took it further meanwhile.

mod common;

use common::{Machine, eventually, group_on, joined, path, rule, shut_down};
use iroh::address_lookup::MemoryLookup;
use pigeon_core::patch::VersionRef;
use pigeon_core::selection::Cutoff;
use pigeon_core::statement::Reason;
use pigeon_sync::{Edit, SuggestionView};

/// The live suggestions `machine` shows, as shown.
async fn shown(machine: &Machine) -> Vec<VersionRef> {
    machine
        .engine
        .suggestions()
        .await
        .into_iter()
        .map(|view| view.statement)
        .collect()
}

/// Publishes `text` at `at` as an action of `machine`.
async fn write(machine: &Machine, at: &str, text: &str) {
    let edit = Edit::Write {
        path: path(at),
        bytes: text.as_bytes().to_vec(),
    };
    machine.engine.edit(vec![edit]).await.unwrap();
}

/// Whether `views` is one suggestion of `machine` with one change, of
/// `size` bytes at `at`, for `reason`.
fn one_change(views: &[SuggestionView], machine: &Machine, at: &str, size: u64) -> bool {
    let [view] = views else {
        return false;
    };
    let [change] = &view.changes[..] else {
        return false;
    };
    view.machine == machine.engine.machine()
        && change.path == path(at)
        && change.content.is_some_and(|content| content.size == size)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_action_that_lost_is_suggested_once_by_its_machine_holding_nothing_there() {
    let lookup = MemoryLookup::new();
    let mut machines = group_on(&lookup, &["alice", "bob"], |_| {}).await;
    joined(&machines).await;
    let bob = machines.pop().unwrap();
    let alice = &machines[0];
    alice.follow("shared/").await;
    bob.engine
        .set_rule(rule("shared/", Cutoff::MinusInfinity))
        .await
        .unwrap();
    write(alice, "shared/notes.txt", "base").await;
    eventually("bob learns of the file", || async {
        bob.engine.history(&path("shared/notes.txt")).len() == 1
    })
    .await;
    let bob = bob.restart_on(&MemoryLookup::new()).await;
    write(&bob, "shared/notes.txt", "bob's").await;
    write(alice, "shared/notes.txt", "alice's").await;
    assert_eq!(bob.read("shared/notes.txt"), None);
    let bob = bob.restart_on(&lookup).await;
    eventually("bob suggests what he wrote", || async {
        let views = alice.engine.suggestions().await;
        one_change(&views, &bob, "shared/notes.txt", 5) && views[0].reason == Reason::Superseded
    })
    .await;
    assert_eq!(alice.read("shared/notes.txt").as_deref(), Some("alice's"));
    alice.engine.discard(&shown(alice).await).await.unwrap();
    eventually("bob sees the suggestion decided", || async {
        bob.engine.suggestions().await.is_empty()
    })
    .await;
    let bob = bob.restart().await;
    bob.wait_past_settling().await;
    bob.wait_past_settling().await;
    assert!(alice.engine.suggestions().await.is_empty());
    assert!(bob.engine.suggestions().await.is_empty());
    for machine in [alice, &bob] {
        assert!(machine.engine.status().await.errors.is_empty());
    }
    machines.push(bob);
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_suggestion_decided_while_its_machine_took_it_further_never_comes_back() {
    let lookup = MemoryLookup::new();
    let mut machines = group_on(&lookup, &["alice", "bob"], |_| {}).await;
    joined(&machines).await;
    let bob = machines.pop().unwrap();
    let alice = &machines[0];
    bob.follow("+alice/").await;
    alice.edit("+alice/old.txt", "moving text");
    eventually("bob holds the file", || async {
        bob.read("+alice/old.txt").is_some()
    })
    .await;
    std::fs::rename(bob.file("+alice/old.txt"), bob.file("+alice/new.txt")).unwrap();
    eventually("alice sees bob's move", || async {
        alice
            .engine
            .suggestions()
            .await
            .first()
            .is_some_and(|view| view.changes.len() == 2)
    })
    .await;
    let bob = bob.restart_on(&MemoryLookup::new()).await;
    alice
        .engine
        .validate(&shown(alice).await, None)
        .await
        .unwrap();
    bob.edit("+alice/new.txt", "moved and edited");
    eventually("bob takes his suggestion further", || async {
        let views = bob.engine.suggestions().await;
        views.len() == 1
            && views[0].changes.len() == 2
            && views[0]
                .changes
                .iter()
                .any(|change| change.content.is_some_and(|content| content.size == 16))
    })
    .await;
    let bob = bob.restart_on(&lookup).await;
    eventually("only bob's last edit waits for the group", || async {
        let views = alice.engine.suggestions().await;
        one_change(&views, &bob, "+alice/new.txt", 16)
            && one_change(&bob.engine.suggestions().await, &bob, "+alice/new.txt", 16)
    })
    .await;
    assert_eq!(alice.read("+alice/new.txt").as_deref(), Some("moving text"));
    assert_eq!(alice.read("+alice/old.txt"), None);
    assert_eq!(
        bob.read("+alice/new.txt").as_deref(),
        Some("moved and edited")
    );
    assert_eq!(bob.read("+alice/old.txt"), None);
    machines.push(bob);
    shut_down(machines).await;
}
