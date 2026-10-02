//! A member who never looks at pigeon: while the family is apart, papy's
//! laptop and desktop pile up every kind of suggestion, an edit that lost
//! to a later one, a save over alice's file and a new file at a path he
//! took after alice; alice decides them all from her machine, and papy's
//! machines carry it out alone.

mod common;

use common::{Machine, eventually, family, published};
use iroh::address_lookup::MemoryLookup;
use serde_json::{Value, json};

/// The id of the one suggestion of `author` at `path`, as `machine` lists
/// it.
async fn suggestion_id(machine: &Machine, path: &str, author: &str) -> Value {
    let suggestion = machine.suggestion_at(path).await;
    assert_eq!(suggestion["author"], author, "{suggestion}");
    suggestion["id"].clone()
}

/// Plays the family apart: alice adds a plan first, papy's laptop edits
/// his budget, then his desktop adds his own plan under the same name,
/// edits the budget later and saves over alice's recipe.
async fn apart(alice: &mut Machine, desktop: &mut Machine, laptop: &mut Machine) {
    alice.go_offline().await;
    desktop.go_offline().await;
    laptop.go_offline().await;
    alice.write("docs/plan.txt", "alice's plan\n");
    published(alice, "docs/plan.txt", 1).await;
    laptop.write("+papy/budget.txt", "from the train\n");
    published(laptop, "+papy/budget.txt", 2).await;
    desktop.write("docs/Plan.txt", "papy's plan\n");
    published(desktop, "docs/Plan.txt", 1).await;
    desktop.write("+papy/budget.txt", "from home, later\n");
    published(desktop, "+papy/budget.txt", 2).await;
    desktop.save_atomically("+alice/recipe.txt", "flour, sugar\n");
    desktop.wait_past_settling().await;
    alice.go_online().await;
    desktop.go_online().await;
    laptop.go_online().await;
}

/// Alice decides the three suggestions from her machine: she places his
/// laptop's budget and her own plan under new names, validates his recipe,
/// and follows his folder and the plans.
async fn resolve_for_papy(alice: &Machine) {
    let superseded = suggestion_id(alice, "+papy/budget.txt", "papy").await;
    let recipe = suggestion_id(alice, "+alice/recipe.txt", "papy").await;
    let plan = suggestion_id(alice, "docs/plan.txt", "alice").await;
    for (id, to) in [
        (&superseded, "+papy/budget (laptop).txt"),
        (&plan, "docs/plan (alice).txt"),
    ] {
        alice.validate(&[id], Some(to)).await;
    }
    alice.validate(&[&recipe], None).await;
    for pattern in ["/+papy/", "/docs/"] {
        alice
            .run("selection", "follow", json!({"pattern": pattern}))
            .await;
    }
}

#[tokio::test]
async fn a_passive_members_conflicts_are_all_resolved_by_another_from_her_machine() {
    let internet = MemoryLookup::new();
    let (mut alice, mut desktop, mut laptop) = family(&internet).await;
    alice.write("+alice/recipe.txt", "flour\n");
    desktop
        .run("selection", "follow", json!({"pattern": "/+alice/"}))
        .await;
    desktop.write("+papy/budget.txt", "v1\n");
    eventually(
        "papy's machines hold the recipe and the budget",
        &[&alice, &desktop, &laptop],
        async || {
            desktop.shows("+alice/recipe.txt", "flour\n")
                && laptop.shows("+papy/budget.txt", "v1\n")
        },
    )
    .await;
    apart(&mut alice, &mut desktop, &mut laptop).await;

    let all = [&alice, &desktop, &laptop];
    eventually("alice sees the three suggestions", &all, async || {
        alice.suggestions().await.len() == 3
    })
    .await;
    resolve_for_papy(&alice).await;
    eventually("nothing waits anywhere", &all, async || {
        alice.suggestions().await.is_empty()
            && desktop.suggestions().await.is_empty()
            && laptop.suggestions().await.is_empty()
    })
    .await;
    eventually("every machine shows what alice decided", &all, async || {
        [&alice, &desktop].iter().all(|machine| {
            machine.shows("+alice/recipe.txt", "flour, sugar\n")
                && machine.shows("+papy/budget.txt", "from home, later\n")
                && machine.shows("+papy/budget (laptop).txt", "from the train\n")
        }) && laptop.shows("+papy/budget (laptop).txt", "from the train\n")
            && alice.shows("docs/Plan.txt", "papy's plan\n")
            && alice.shows("docs/plan (alice).txt", "alice's plan\n")
    })
    .await;

    desktop.switch_off().await;
    desktop.switch_on().await;
    desktop.wait_past_settling().await;
    assert_eq!(desktop.suggestions().await, Vec::<Value>::new());
    assert!(desktop.shows("+alice/recipe.txt", "flour, sugar\n"));
}
