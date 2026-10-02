//! Tests of a group the network splits: two machines that reach each other
//! only through a third agree on the file both edited at once, and still
//! once the network heals; a suggestion decided on each side of a partition
//! keeps the earlier decision once it heals, whichever it is, and the
//! refused validation waits as a suggestion of its machine, whose disk
//! shows the group's version when it never held the suggested content; and
//! a burst of more patches than a session buffers all arrive and land on
//! disk.

mod common;

use common::*;
use iroh::address_lookup::MemoryLookup;
use pigeon_core::statement::Reason;
use pigeon_sync::Edit;

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
    converged(&machines, &[]).await;
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
    converged(&machines, &[]).await;
    let loser = machines
        .iter()
        .find(|machine| machine.engine.machine() == suggestion.machine)
        .unwrap();
    loser
        .engine
        .discard(std::slice::from_ref(&suggestion.statement))
        .await
        .unwrap();
    converged(&machines, &[]).await;
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

/// Bob's suggestion of Alice's plan, which reaches Alice only through
/// Carol, who holds none of its content, and which Alice validates and Bob
/// discards on each side of a partition, the validation first if
/// `validated_first`; the earlier decision holds everywhere once the
/// partition heals. A refused validation waits as Alice's suggestion, and
/// her disk, which never held the content it suggests, shows the group's
/// version.
async fn decided_apart(validated_first: bool) {
    let whole = MemoryLookup::new();
    let machines = group_on(&whole, &["alice", "bob", "carol"], |_| {}).await;
    joined(&machines).await;
    let [alice, bob, carol]: [Machine; 3] = machines.try_into().ok().unwrap();
    bob.follow("+alice/").await;
    alice.edit("+alice/plan.txt", "alice's plan");
    eventually("bob holds the plan", || async {
        bob.read("+alice/plan.txt").is_some()
    })
    .await;
    let through_carol = MemoryLookup::new();
    through_carol.add_endpoint_info(whole.get_endpoint_info(carol.engine.machine()).unwrap());
    let bob_id = bob.engine.machine();
    let reaches_bob = |machine: &Machine| machine.engine.status().peers.contains(&bob_id);
    let bob = bob.restart_on(&through_carol).await;
    eventually("only carol reaches bob", || async {
        reaches_bob(&carol) && !reaches_bob(&alice)
    })
    .await;
    bob.edit("+alice/plan.txt", "bob's plan");
    eventually("alice sees bob's suggestion", || async {
        alice.engine.suggestions().await.len() == 1
    })
    .await;
    let shown = alice.engine.suggestions().await.remove(0);
    let content = shown.changes[0].content.unwrap();
    assert!(alice.engine.read(&content).await.unwrap().is_none());
    let statement = [shown.statement];
    let bob = bob.restart_on(&MemoryLookup::new()).await;
    eventually("no machine reaches bob", || async { !reaches_bob(&carol) }).await;
    if validated_first {
        alice.engine.validate(&statement, None).await.unwrap();
        bob.engine.discard(&statement).await.unwrap();
    } else {
        bob.engine.discard(&statement).await.unwrap();
        alice.engine.validate(&statement, None).await.unwrap();
    }
    let (decider, versions) = if validated_first {
        ("alice", 2)
    } else {
        ("bob", 1)
    };
    let bob = bob.restart_on(&through_carol).await;
    let machines = [alice, bob, carol];
    eventually("the earlier decision reaches every machine", || async {
        machines.iter().all(|machine| {
            let decisions = machine.engine.history(&statement[0].path);
            matches!(&decisions[..], [_, decision] if decision.author.as_str() == decider)
        })
    })
    .await;
    let [alice, bob, carol] = machines;
    let refused = usize::from(!validated_first);
    eventually(
        "alice's refused validation waits as her suggestion",
        || async { alice.engine.suggestions().await.len() == refused },
    )
    .await;
    let bob = bob.restart_on(&whole).await;
    let machines = [alice, bob, carol];
    let fetching_what_only_bob_holds = format!("fetching {}", content.hash);
    converged(&machines, &[&fetching_what_only_bob_holds]).await;
    for machine in &machines {
        let decisions = machine.engine.history(&statement[0].path);
        let [_, decision] = &decisions[..] else {
            panic!("one decision holds: {decisions:?}")
        };
        assert_eq!(decision.author.as_str(), decider);
        let plan = machine.engine.history(&path("+alice/plan.txt"));
        assert_eq!(plan.len(), versions, "{plan:?}");
    }
    let [alice, bob, _] = &machines;
    let suggestions = alice.engine.suggestions().await;
    let shown = if validated_first {
        assert!(suggestions.is_empty(), "{suggestions:?}");
        "bob's plan"
    } else {
        let [refused] = &suggestions[..] else {
            panic!("the refused validation waits: {suggestions:?}")
        };
        assert_eq!(refused.machine, alice.engine.machine());
        assert!(matches!(refused.reason, Reason::Rejected(_)));
        assert_eq!(refused.changes[0].content, Some(content));
        "alice's plan"
    };
    for machine in [alice, bob] {
        assert_eq!(machine.read("+alice/plan.txt").as_deref(), Some(shown));
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
