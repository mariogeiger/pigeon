//! A member who never looks at pigeon: while the family is apart, papy's
//! laptop and desktop pile up every kind of set-aside item, an edit that
//! lost to a later one, a save over alice's read-only file, a drop whose
//! name alice took first and a name Windows cannot hold; alice resolves
//! them all from her machine, and papy's machines carry it out alone.

mod common;

use common::{Machine, eventually, family, published, settle};
use iroh::address_lookup::MemoryLookup;
use serde_json::{Value, json};

/// The entry of the item papy's machines set aside at `path`, as
/// `machine` lists it.
async fn aside_entry(machine: &Machine, path: &str) -> Value {
    let items = machine.aside().await;
    let item = items
        .iter()
        .find(|item| item["path"] == path)
        .unwrap_or_else(|| panic!("nothing set aside at {path}: {items:?}"));
    assert_eq!(item["author"], "papy", "{item}");
    item["entry"].clone()
}

/// Plays the family apart: alice drops a plan first, papy's laptop edits
/// his budget, then his desktop drops his own plan under the same name,
/// edits the budget later, saves over alice's recipe and writes an invoice
/// whose name Windows cannot hold.
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
    desktop.write("+papy/Facture: mars.txt", "42 €\n");
    settle().await;
    alice.go_online().await;
    desktop.go_online().await;
    laptop.go_online().await;
}

/// Alice resolves papy's four items from her machine: she restores his
/// laptop's budget, his plan and his invoice under new names, forces his
/// recipe, and follows his folder and the plans.
async fn resolve_for_papy(alice: &Machine) {
    let superseded = aside_entry(alice, "+papy/budget.txt").await;
    let not_writable = aside_entry(alice, "+alice/recipe.txt").await;
    let rejected = aside_entry(alice, "docs/Plan.txt").await;
    let unportable = aside_entry(alice, "+papy/Facture: mars.txt").await;
    for (entry, to) in [
        (superseded, "+papy/budget (laptop).txt"),
        (rejected, "docs/plan (papy).txt"),
        (unportable, "+papy/Facture - mars.txt"),
    ] {
        alice
            .run(
                "change",
                "place",
                json!({"entry": entry, "to": to, "mode": "force"}),
            )
            .await;
    }
    alice
        .run(
            "change",
            "apply",
            json!({"entry": not_writable, "message": "papy's sugar"}),
        )
        .await;
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
    eventually("alice sees papy's four items", &all, async || {
        alice.aside().await.len() == 4
    })
    .await;
    resolve_for_papy(&alice).await;
    eventually("nothing waits anywhere", &all, async || {
        alice.changes().await.is_empty()
            && desktop.changes().await.is_empty()
            && laptop.changes().await.is_empty()
    })
    .await;
    eventually("every machine shows what alice decided", &all, async || {
        [&alice, &desktop].iter().all(|machine| {
            machine.shows("+alice/recipe.txt", "flour, sugar\n")
                && machine.shows("+papy/budget.txt", "from home, later\n")
                && machine.shows("+papy/budget (laptop).txt", "from the train\n")
                && machine.shows("+papy/Facture - mars.txt", "42 €\n")
        }) && laptop.shows("+papy/budget (laptop).txt", "from the train\n")
            && laptop.shows("+papy/Facture - mars.txt", "42 €\n")
            && alice.shows("docs/plan.txt", "alice's plan\n")
            && alice.shows("docs/plan (papy).txt", "papy's plan\n")
            && desktop.read("+papy/Facture: mars.txt").is_none()
    })
    .await;

    desktop.switch_off().await;
    desktop.switch_on().await;
    settle().await;
    assert_eq!(desktop.changes().await, Vec::<Value>::new());
    assert!(desktop.read("+papy/Facture: mars.txt").is_none());
    assert!(desktop.shows("+papy/Facture - mars.txt", "42 €\n"));
}
