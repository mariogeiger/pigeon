//! Conflicts as a family meets them: machines that go offline and come
//! back, edits of someone else's files, two people adding a file at the
//! same path while apart, names one system cannot hold, actions that reach an owner
//! who edited meanwhile, and files moved or deleted by someone who does
//! not own them; each becomes a suggestion that anyone decides, played on
//! daemons driven through the command line's client and edited on disk.

mod common;

use common::{content, eventually, family, published, settle};
use iroh::address_lookup::MemoryLookup;
use pigeon_core::statement::Reason;
use serde_json::{Value, json};

/// The change of `path` that `suggestion` makes.
fn change_of<'a>(suggestion: &'a Value, path: &str) -> &'a Value {
    suggestion["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["path"] == path)
        .unwrap_or_else(|| panic!("{suggestion} does not change {path}"))
}

#[tokio::test]
async fn an_edit_made_offline_loses_to_a_later_one_and_its_disk_keeps_it_until_discarded() {
    let internet = MemoryLookup::new();
    let (alice, desktop, mut laptop) = family(&internet).await;
    let all = [&alice, &desktop, &laptop];
    desktop.write("+papy/budget.txt", "v1\n");
    eventually("the laptop holds v1", &all, async || {
        laptop.shows("+papy/budget.txt", "v1\n")
    })
    .await;

    laptop.go_offline().await;
    laptop.write("+papy/budget.txt", "from the train\n");
    published(&laptop, "+papy/budget.txt", 2).await;
    desktop.write("+papy/budget.txt", "from home, later\n");
    published(&desktop, "+papy/budget.txt", 2).await;
    laptop.go_online().await;

    let all = [&alice, &desktop, &laptop];
    for machine in all {
        eventually(
            "every machine sees the laptop's edit suggested",
            &all,
            async || {
                let suggested = machine.suggested("+papy/budget.txt").await;
                suggested.len() == 1 && suggested[0]["reason"] == Reason::Superseded.to_string()
            },
        )
        .await;
    }
    let suggestion = alice.suggestion_at("+papy/budget.txt").await;
    assert_eq!(suggestion["author"], "papy", "{suggestion}");
    assert_eq!(
        change_of(&suggestion, "+papy/budget.txt")["content"],
        content("from the train\n"),
        "{suggestion}"
    );
    assert!(laptop.shows("+papy/budget.txt", "from the train\n"));
    assert!(desktop.shows("+papy/budget.txt", "from home, later\n"));
    let history = desktop.history("+papy/budget.txt").await;
    assert_eq!(
        history.last(),
        Some(&Some(content("from home, later\n"))),
        "{history:?}"
    );

    alice.discard(&[&suggestion["id"]]).await;
    eventually(
        "the laptop shows the group's version again",
        &all,
        async || {
            laptop.shows("+papy/budget.txt", "from home, later\n")
                && laptop.suggestions().await.is_empty()
        },
    )
    .await;
    let history = alice.history("+papy/budget.txt").await;
    assert!(
        history.contains(&Some(content("from the train\n"))),
        "{history:?}"
    );
}

#[tokio::test]
async fn an_edit_of_someone_elses_file_is_suggested_and_anyone_validates_it_once() {
    let internet = MemoryLookup::new();
    let (alice, desktop, laptop) = family(&internet).await;
    let all = [&alice, &desktop, &laptop];
    alice.write("+alice/recipe.txt", "flour\n");
    desktop
        .run("selection", "follow", json!({"pattern": "/+alice/"}))
        .await;
    eventually("papy holds the recipe", &all, async || {
        desktop.shows("+alice/recipe.txt", "flour\n")
    })
    .await;

    desktop.save_atomically("+alice/recipe.txt", "flour, sugar\n");
    let suggestion = alice.suggestion_at("+alice/recipe.txt").await;
    assert_eq!(
        (&suggestion["author"], &suggestion["reason"]),
        (&json!("papy"), &json!(Reason::OutsideRules.to_string())),
        "{suggestion}"
    );
    assert!(alice.shows("+alice/recipe.txt", "flour\n"));
    assert!(desktop.shows("+alice/recipe.txt", "flour, sugar\n"));

    laptop.validate(&[&suggestion["id"]], None).await;
    eventually(
        "both hold papy's recipe and nothing waits",
        &all,
        async || {
            alice.shows("+alice/recipe.txt", "flour, sugar\n")
                && desktop.shows("+alice/recipe.txt", "flour, sugar\n")
                && alice.suggestions().await.is_empty()
                && desktop.suggestions().await.is_empty()
        },
    )
    .await;
    let again = alice
        .call(
            "suggestion",
            "discard",
            json!({"suggestions": suggestion["id"]}),
        )
        .await;
    assert!(again.is_err(), "{again:?}");
    assert!(desktop.shows("+alice/recipe.txt", "flour, sugar\n"));
}

#[tokio::test]
async fn two_members_adding_one_path_while_apart_both_keep_their_content() {
    let internet = MemoryLookup::new();
    let (mut alice, mut desktop, mut laptop) = family(&internet).await;
    laptop.switch_off().await;
    alice.go_offline().await;
    desktop.go_offline().await;
    alice.write("docs/plan.txt", "alice's plan\n");
    published(&alice, "docs/plan.txt", 1).await;
    desktop.write("docs/Plan.txt", "papy's plan\n");
    published(&desktop, "docs/Plan.txt", 1).await;
    alice.go_online().await;
    desktop.go_online().await;

    let both = [&alice, &desktop];
    let suggestion = alice.suggestion_at("docs/plan.txt").await;
    assert_eq!(
        suggestion["reason"],
        Reason::Superseded.to_string(),
        "{suggestion}"
    );
    assert_eq!(
        change_of(&suggestion, "docs/plan.txt")["content"],
        content("alice's plan\n"),
        "{suggestion}"
    );
    eventually("papy's later plan holds the name", &both, async || {
        alice
            .listed("docs/Plan.txt")
            .await
            .is_some_and(|file| file["content"] == content("papy's plan\n"))
    })
    .await;
    assert!(alice.shows("docs/plan.txt", "alice's plan\n"));
    alice
        .validate(&[&suggestion["id"]], Some("docs/plan (alice).txt"))
        .await;
    desktop
        .run("selection", "follow", json!({"pattern": "/docs/"}))
        .await;
    eventually("papy holds alice's plan beside his", &both, async || {
        desktop.shows("docs/plan (alice).txt", "alice's plan\n")
            && desktop.shows("docs/Plan.txt", "papy's plan\n")
            && alice.shows("docs/plan (alice).txt", "alice's plan\n")
            && alice.suggestions().await.is_empty()
    })
    .await;
    let placed = desktop.listed("docs/plan (alice).txt").await.unwrap();
    assert_eq!(
        (&placed["owner"], &placed["author"]),
        (&Value::Null, &json!("alice")),
        "{placed}"
    );
}

#[tokio::test]
async fn a_name_windows_cannot_hold_is_suggested_and_validated_under_one_it_can() {
    let internet = MemoryLookup::new();
    let (alice, mut desktop, laptop) = family(&internet).await;
    desktop.write("+papy/Facture: mars.txt", "42 €\n");
    let all = [&alice, &desktop, &laptop];
    let suggestion = desktop.suggestion_at("+papy/Facture: mars.txt").await;
    assert!(
        suggestion["reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with("a name not every machine can hold")),
        "{suggestion}"
    );
    let refused = desktop
        .call(
            "suggestion",
            "validate",
            json!({"suggestions": suggestion["id"]}),
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    desktop
        .validate(&[&suggestion["id"]], Some("+papy/Facture - mars.txt"))
        .await;
    alice
        .run("selection", "follow", json!({"pattern": "/+papy/"}))
        .await;
    eventually(
        "alice holds the invoice and papy's disk only its portable name",
        &all,
        async || {
            alice.shows("+papy/Facture - mars.txt", "42 €\n")
                && desktop.shows("+papy/Facture - mars.txt", "42 €\n")
                && desktop.read("+papy/Facture: mars.txt").is_none()
                && desktop.suggestions().await.is_empty()
        },
    )
    .await;
    desktop.switch_off().await;
    desktop.switch_on().await;
    settle().await;
    assert_eq!(desktop.suggestions().await, Vec::<Value>::new());
    assert!(desktop.read("+papy/Facture: mars.txt").is_none());
}

#[tokio::test]
async fn an_action_reaching_an_owner_who_edited_offline_wins_and_keeps_both_in_history() {
    let internet = MemoryLookup::new();
    let (mut alice, desktop, _laptop) = family(&internet).await;
    alice.write("+alice/list.txt", "milk\n");
    desktop
        .run("selection", "follow", json!({"pattern": "/+alice/"}))
        .await;
    eventually("papy holds the list", &[&alice, &desktop], async || {
        desktop.shows("+alice/list.txt", "milk\n")
    })
    .await;
    alice.go_offline().await;
    alice.write("+alice/list.txt", "milk, eggs\n");
    published(&alice, "+alice/list.txt", 2).await;
    desktop
        .run(
            "file",
            "write",
            json!({"path": "+alice/list.txt", "content": common::base64("milk, bread\n")}),
        )
        .await;
    alice.go_online().await;
    let both = [&alice, &desktop];
    eventually("the later action wins", &both, async || {
        desktop.shows("+alice/list.txt", "milk, bread\n")
            && alice
                .listed("+alice/list.txt")
                .await
                .is_some_and(|file| file["content"] == content("milk, bread\n"))
    })
    .await;
    let history = alice.history("+alice/list.txt").await;
    assert!(
        history.contains(&Some(content("milk, eggs\n"))),
        "{history:?}"
    );
    let suggestion = alice.suggestion_at("+alice/list.txt").await;
    assert_eq!(
        suggestion["reason"],
        Reason::Superseded.to_string(),
        "{suggestion}"
    );
}

#[tokio::test]
async fn a_suggestion_made_before_the_owners_edit_says_it_is_outdated_and_still_validates() {
    let internet = MemoryLookup::new();
    let (alice, desktop, _laptop) = family(&internet).await;
    let both = [&alice, &desktop];
    alice.write("+alice/list.txt", "milk\n");
    desktop
        .run("selection", "follow", json!({"pattern": "/+alice/"}))
        .await;
    eventually("papy holds the list", &both, async || {
        desktop.shows("+alice/list.txt", "milk\n")
    })
    .await;
    desktop.write("+alice/list.txt", "milk, bread\n");
    let suggestion = alice.suggestion_at("+alice/list.txt").await;
    assert_eq!(change_of(&suggestion, "+alice/list.txt")["outdated"], false);
    alice.write("+alice/list.txt", "milk, eggs\n");
    eventually("the suggestion is outdated", &both, async || {
        let suggestion = alice.suggestion_at("+alice/list.txt").await;
        change_of(&suggestion, "+alice/list.txt")["outdated"] == true
    })
    .await;
    alice.validate(&[&suggestion["id"]], None).await;
    eventually("papy's list wins", &both, async || {
        alice.shows("+alice/list.txt", "milk, bread\n")
    })
    .await;
    let history = alice.history("+alice/list.txt").await;
    assert!(
        history.contains(&Some(content("milk, eggs\n"))),
        "{history:?}"
    );
}

#[tokio::test]
async fn deleting_someone_elses_file_on_disk_suggests_its_deletion_and_a_discard_brings_it_back() {
    let internet = MemoryLookup::new();
    let (alice, desktop, _laptop) = family(&internet).await;
    let both = [&alice, &desktop];
    alice.write("+alice/recipe.txt", "flour\n");
    desktop
        .run("selection", "follow", json!({"pattern": "/+alice/"}))
        .await;
    eventually("papy holds the recipe", &both, async || {
        desktop.shows("+alice/recipe.txt", "flour\n")
    })
    .await;
    desktop.remove("+alice/recipe.txt");
    let suggestion = alice.suggestion_at("+alice/recipe.txt").await;
    assert_eq!(
        change_of(&suggestion, "+alice/recipe.txt")["content"],
        Value::Null,
        "{suggestion}"
    );
    assert!(alice.shows("+alice/recipe.txt", "flour\n"));
    assert!(desktop.read("+alice/recipe.txt").is_none());
    alice.discard(&[&suggestion["id"]]).await;
    eventually("papy holds the recipe again", &both, async || {
        desktop.shows("+alice/recipe.txt", "flour\n") && desktop.suggestions().await.is_empty()
    })
    .await;
}

#[tokio::test]
async fn moving_a_folder_of_someone_elses_files_on_disk_suggests_the_move_with_its_history() {
    let internet = MemoryLookup::new();
    let (alice, desktop, _laptop) = family(&internet).await;
    let both = [&alice, &desktop];
    alice.write("photos/2024/beach.txt", "sand\n");
    alice.write("photos/2024/sea.txt", "salt\n");
    desktop
        .run("selection", "follow", json!({"pattern": "/photos/"}))
        .await;
    desktop
        .run("selection", "follow", json!({"pattern": "/archive/"}))
        .await;
    eventually("papy holds the photos", &both, async || {
        desktop.shows("photos/2024/beach.txt", "sand\n")
            && desktop.shows("photos/2024/sea.txt", "salt\n")
    })
    .await;
    desktop.rename("photos/2024", "archive/2024");
    eventually("alice sees both moves suggested", &both, async || {
        alice.suggested("archive/2024/beach.txt").await.len() == 1
            && alice.suggested("archive/2024/sea.txt").await.len() == 1
    })
    .await;
    let moves = alice.suggestions().await;
    for suggestion in &moves {
        let changes = suggestion["changes"].as_array().unwrap();
        assert_eq!(changes.len(), 2, "one move is one suggestion: {suggestion}");
    }
    assert!(alice.shows("photos/2024/beach.txt", "sand\n"));
    assert!(desktop.shows("archive/2024/beach.txt", "sand\n"));
    let ids: Vec<&Value> = moves.iter().map(|suggestion| &suggestion["id"]).collect();
    alice.validate(&ids, None).await;
    alice
        .run("selection", "follow", json!({"pattern": "/archive/"}))
        .await;
    eventually(
        "alice holds the photos where papy moved them",
        &both,
        async || {
            alice.shows("archive/2024/beach.txt", "sand\n")
                && alice.shows("archive/2024/sea.txt", "salt\n")
                && alice.read("photos/2024/beach.txt").is_none()
                && desktop.suggestions().await.is_empty()
        },
    )
    .await;
    let history = alice
        .run("file", "history", json!({"path": "archive/2024/beach.txt"}))
        .await;
    let paths: Vec<&Value> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|version| &version["path"])
        .collect();
    assert_eq!(
        paths,
        [
            &json!("photos/2024/beach.txt"),
            &json!("archive/2024/beach.txt")
        ],
        "{history}"
    );
}
