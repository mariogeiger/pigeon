//! Tests of a group the network splits: two machines that reach each other
//! only through a third agree on the file both edited at once, and still
//! once the network heals; a suggestion decided on each side of a partition
//! keeps the earlier decision once it heals, whichever it is, and the
//! refused validation waits as a suggestion of its machine; and a burst of
//! more patches than a session buffers all arrive and land on disk.

mod common;

use common::*;
use iroh::address_lookup::MemoryLookup;
use pigeon_core::statement::Reason;
use pigeon_sync::Edit;

/// What a machine on the far side of the machine between reports while the
/// machine between has yet to fetch a content it wants.
const NOT_YET_BETWEEN: &str = "no machine reached holds";

#[tokio::test(flavor = "multi_thread")]
async fn edits_made_at_once_on_both_sides_of_a_partial_partition_converge_then_and_once_it_heals() {
    let near = MemoryLookup::new();
    let mut machines = group_on(&near, &["bob", "alice"], |_| {}).await;
    joined(&machines).await;
    let far = MemoryLookup::new();
    far.add_endpoint_info(
        near.get_endpoint_info(machines[0].engine.machine())
            .unwrap(),
    );
    let key = machines[0].engine.group_key().parse().unwrap();
    machines.push(start_with(key, "alice", options(&far)).await);
    joined(&machines).await;
    for machine in &machines {
        machine.follow("*").await;
    }
    let (laptop, desktop) = (&machines[1], &machines[2]);
    laptop.edit("+alice/todo.txt", "base");
    eventually("the desktop holds the file through bob", || async {
        desktop.read("+alice/todo.txt").as_deref() == Some("base")
    })
    .await;
    laptop.edit("+alice/todo.txt", "laptop");
    laptop.edit("+alice/laptop.txt", "from the laptop");
    desktop.edit("+alice/todo.txt", "desktop");
    desktop.edit("+alice/desktop.txt", "from the desktop");
    converged(&machines, &[NOT_YET_BETWEEN]).await;
    let desktop_id = machines[2].engine.machine();
    assert!(!machines[1].engine.status().peers.contains(&desktop_id));
    let suggestions = machines[1].engine.suggestions().await;
    let [suggestion] = &suggestions[..] else {
        panic!("one edit loses: {suggestions:?}")
    };
    assert_eq!(suggestion.reason, Reason::Superseded);
    let desktop = machines.pop().unwrap().restart_on(&near).await;
    machines.push(desktop);
    eventually("the laptop reaches the desktop directly", || async {
        machines[1].engine.status().peers.contains(&desktop_id)
    })
    .await;
    converged(&machines, &[NOT_YET_BETWEEN]).await;
    let loser = machines
        .iter()
        .find(|machine| machine.engine.machine() == suggestion.machine)
        .unwrap();
    loser
        .engine
        .discard(std::slice::from_ref(&suggestion.statement))
        .await
        .unwrap();
    converged(&machines, &[NOT_YET_BETWEEN]).await;
    let todo = machines[0].read("+alice/todo.txt");
    assert!(todo.is_some());
    for machine in &machines {
        assert_eq!(machine.read("+alice/todo.txt"), todo);
        assert_eq!(
            machine.read("+alice/desktop.txt").as_deref(),
            Some("from the desktop")
        );
    }
    shut_down(machines).await;
}

/// Bob's suggestion of Alice's plan, which Alice validates and Bob
/// discards on each side of a partition, the validation first if
/// `validated_first`; the earlier decision holds everywhere once the
/// partition heals, and a refused validation waits as Alice's suggestion.
async fn decided_apart(validated_first: bool) {
    let whole = MemoryLookup::new();
    let machines = group_on(&whole, &["alice", "bob"], |_| {}).await;
    joined(&machines).await;
    let [alice, bob]: [Machine; 2] = machines.try_into().ok().unwrap();
    bob.follow("+alice/").await;
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
    let statement = [alice.engine.suggestions().await.remove(0).statement];
    let bob_id = bob.engine.machine();
    let bob = bob.restart_on(&MemoryLookup::new()).await;
    eventually("alice no longer reaches bob", || async {
        !alice.engine.status().peers.contains(&bob_id)
    })
    .await;
    if validated_first {
        alice.engine.validate(&statement, None).await.unwrap();
        bob.engine.discard(&statement).await.unwrap();
    } else {
        bob.engine.discard(&statement).await.unwrap();
        alice.engine.validate(&statement, None).await.unwrap();
    }
    let bob = bob.restart_on(&whole).await;
    let machines = [alice, bob];
    converged(&machines, &[]).await;
    let (decider, versions) = if validated_first {
        ("alice", 2)
    } else {
        ("bob", 1)
    };
    for machine in &machines {
        let decisions = machine.engine.history(&statement[0].path);
        let [_, decision] = &decisions[..] else {
            panic!("one decision holds: {decisions:?}")
        };
        assert_eq!(decision.author.as_str(), decider);
        let plan = machine.engine.history(&path("+alice/plan.txt"));
        assert_eq!(plan.len(), versions, "{plan:?}");
    }
    let [alice, bob] = &machines;
    let suggestions = alice.engine.suggestions().await;
    if validated_first {
        assert!(suggestions.is_empty(), "{suggestions:?}");
        for machine in [alice, bob] {
            assert_eq!(
                machine.read("+alice/plan.txt").as_deref(),
                Some("bob's plan")
            );
        }
    } else {
        let [refused] = &suggestions[..] else {
            panic!("the refused validation waits: {suggestions:?}")
        };
        assert_eq!(refused.machine, alice.engine.machine());
        assert!(matches!(refused.reason, Reason::Rejected(_)));
        assert_eq!(alice.read("+alice/plan.txt").as_deref(), Some("bob's plan"));
        assert_eq!(bob.read("+alice/plan.txt").as_deref(), Some("alice's plan"));
    }
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_validation_made_apart_before_a_discard_holds_once_the_partition_heals() {
    Box::pin(decided_apart(true)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_discard_made_apart_before_a_validation_holds_once_the_partition_heals() {
    Box::pin(decided_apart(false)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_burst_of_more_patches_than_a_session_buffers_all_arrive_and_land_on_disk() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.follow("+alice/").await;
    let burst = 300;
    for index in 0..burst {
        alice
            .engine
            .edit(vec![Edit::Write {
                path: path(&format!("+alice/burst/{index}.txt")),
                bytes: index.to_string().into_bytes(),
            }])
            .await
            .unwrap();
    }
    converged(&machines, &[]).await;
    assert_eq!(bob.engine.list(None).await.unwrap().len(), burst);
    assert_eq!(bob.read("+alice/burst/299.txt").as_deref(), Some("299"));
    shut_down(machines).await;
}
