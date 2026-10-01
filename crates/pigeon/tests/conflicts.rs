//! Conflicts as a family meets them: machines that go offline and come
//! back, editors that save over read-only files, two people dropping the
//! same file while apart, names one system cannot hold, requests that
//! reach an owner who edited meanwhile, and files moved or deleted by
//! someone who does not own them; each played on daemons driven through
//! the command line's client and edited on disk.

mod common;

use common::{Machine, content, eventually, family, settle};
use iroh::address_lookup::MemoryLookup;
use serde_json::{Value, json};

/// The reasons of the items `machine` lists as set aside at `path`.
async fn aside_at(machine: &Machine, path: &str) -> Vec<Value> {
    machine
        .aside()
        .await
        .into_iter()
        .filter(|item| item["path"] == path)
        .map(|item| item["reason"].clone())
        .collect()
}

#[tokio::test]
async fn an_edit_made_offline_loses_to_a_later_one_of_the_same_member_and_stays_aside() {
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
    settle().await;
    desktop.write("+papy/budget.txt", "from home, later\n");
    settle().await;
    laptop.go_online().await;

    let all = [&alice, &desktop, &laptop];
    eventually("the later edit wins on the laptop", &all, async || {
        laptop.shows("+papy/budget.txt", "from home, later\n")
    })
    .await;
    for machine in all {
        eventually(
            "every machine sees the laptop's edit aside",
            &all,
            async || aside_at(machine, "+papy/budget.txt").await == [json!("Superseded")],
        )
        .await;
    }
    let item = &alice.aside().await[0];
    assert_eq!(item["member"], "papy", "{item}");
    assert_eq!(item["content"], content("from the train\n"), "{item}");
    assert!(desktop.shows("+papy/budget.txt", "from home, later\n"));
    let history = desktop.history("+papy/budget.txt").await;
    assert_eq!(
        history.last(),
        Some(&Some(content("from home, later\n"))),
        "{history:?}"
    );
}

#[tokio::test]
async fn an_editor_saving_over_a_read_only_file_is_set_aside_then_proposed_and_accepted() {
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
    assert!(desktop.refuses_writing("+alice/recipe.txt"));

    desktop.save_atomically("+alice/recipe.txt", "flour, sugar\n");
    eventually(
        "the recipe is restored and papy's copy set aside",
        &all,
        async || {
            desktop.shows("+alice/recipe.txt", "flour\n")
                && aside_at(&desktop, "+alice/recipe.txt").await == [json!("NotWritable")]
        },
    )
    .await;
    let item = &desktop.aside().await[0];
    assert_eq!(item["here"], true, "{item}");
    desktop
        .run(
            "aside",
            "request",
            json!({"file": item["file"], "mode": "propose", "message": "sugar"}),
        )
        .await;
    eventually("alice hears the proposal", &all, async || {
        alice
            .requests()
            .await
            .iter()
            .any(|request| request["statement"]["mode"] == "propose")
    })
    .await;
    let requests = alice.requests().await;
    let proposal = requests
        .iter()
        .find(|request| request["statement"]["mode"] == "propose")
        .unwrap();
    alice
        .run("request", "accept", json!({"request": proposal["path"]}))
        .await;
    eventually(
        "both hold papy's recipe and nothing stays aside",
        &all,
        async || {
            alice.shows("+alice/recipe.txt", "flour, sugar\n")
                && desktop.shows("+alice/recipe.txt", "flour, sugar\n")
                && desktop.aside().await.is_empty()
                && alice.aside().await.is_empty()
        },
    )
    .await;
}

#[tokio::test]
async fn two_members_dropping_one_name_while_apart_both_keep_their_content() {
    let internet = MemoryLookup::new();
    let (mut alice, mut desktop, mut laptop) = family(&internet).await;
    laptop.switch_off().await;
    alice.go_offline().await;
    desktop.go_offline().await;
    alice.write("docs/plan.txt", "alice's plan\n");
    settle().await;
    desktop.write("docs/Plan.txt", "papy's plan\n");
    settle().await;
    alice.go_online().await;
    desktop.go_online().await;

    let both = [&alice, &desktop];
    eventually(
        "the first drop holds the name on both disks",
        &both,
        async || {
            alice.shows("docs/plan.txt", "alice's plan\n")
                && desktop.shows("docs/plan.txt", "alice's plan\n")
                && desktop.read("docs/Plan.txt").is_none()
        },
    )
    .await;
    eventually("papy's plan is set aside", &both, async || {
        alice.aside().await.len() == 1
    })
    .await;
    let item = &alice.aside().await[0];
    assert_eq!(item["path"], "docs/Plan.txt", "{item}");
    assert!(item["reason"]["Rejected"].is_string(), "{item}");
    assert_eq!(item["content"], content("papy's plan\n"), "{item}");
    desktop
        .run(
            "aside",
            "restore",
            json!({"file": item["file"], "to": "docs/Plan (papy).txt"}),
        )
        .await;
    alice
        .run("selection", "follow", json!({"pattern": "/docs/"}))
        .await;
    eventually("alice holds papy's plan beside hers", &both, async || {
        alice.shows("docs/Plan (papy).txt", "papy's plan\n")
            && alice.shows("docs/plan.txt", "alice's plan\n")
            && desktop.aside().await.is_empty()
    })
    .await;
    let files = alice.run("file", "list", json!({})).await;
    let restored = files
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "docs/Plan (papy).txt")
        .unwrap();
    assert_eq!(restored["owner"], "papy", "{restored}");
}

#[tokio::test]
async fn a_name_windows_cannot_hold_is_set_aside_and_restored_under_one_it_can() {
    let internet = MemoryLookup::new();
    let (alice, mut desktop, laptop) = family(&internet).await;
    desktop.write("+papy/Facture: mars.txt", "42 €\n");
    let all = [&alice, &desktop, &laptop];
    eventually("the invoice is set aside", &all, async || {
        desktop.aside().await.len() == 1
    })
    .await;
    let item = &desktop.aside().await[0];
    assert!(item["reason"]["Unportable"].is_string(), "{item}");
    desktop
        .run(
            "aside",
            "restore",
            json!({"file": item["file"], "to": "+papy/Facture - mars.txt"}),
        )
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
                && desktop.aside().await.is_empty()
        },
    )
    .await;
    desktop.switch_off().await;
    desktop.switch_on().await;
    settle().await;
    assert_eq!(desktop.aside().await, Vec::<Value>::new());
    assert!(desktop.read("+papy/Facture: mars.txt").is_none());
}

#[tokio::test]
async fn a_forced_request_reaching_an_owner_who_edited_offline_keeps_both_in_history() {
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
    settle().await;
    desktop
        .run(
            "file",
            "write",
            json!({"path": "+alice/list.txt", "content": common::base64("milk, bread\n"), "mode": "force"}),
        )
        .await;
    alice.go_online().await;
    eventually(
        "both hold papy's forced list",
        &[&alice, &desktop],
        async || {
            alice.shows("+alice/list.txt", "milk, bread\n")
                && desktop.shows("+alice/list.txt", "milk, bread\n")
        },
    )
    .await;
    let history = alice.history("+alice/list.txt").await;
    assert!(
        history.contains(&Some(content("milk, eggs\n"))),
        "{history:?}"
    );
}

#[tokio::test]
async fn a_proposal_made_before_the_owners_edit_says_it_is_outdated() {
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
    desktop
        .run(
            "file",
            "write",
            json!({"path": "+alice/list.txt", "content": common::base64("milk, bread\n")}),
        )
        .await;
    eventually("alice hears it", &both, async || {
        alice.requests().await.len() == 1
    })
    .await;
    assert_eq!(alice.requests().await[0]["outdated"], false);
    alice.write("+alice/list.txt", "milk, eggs\n");
    eventually("the proposal is outdated", &both, async || {
        alice.requests().await[0]["outdated"] == true
    })
    .await;
}

#[tokio::test]
async fn deleting_someone_elses_file_on_disk_only_stops_holding_it() {
    let internet = MemoryLookup::new();
    let (alice, desktop, _laptop) = family(&internet).await;
    alice.write("+alice/recipe.txt", "flour\n");
    desktop
        .run("selection", "follow", json!({"pattern": "/+alice/"}))
        .await;
    eventually("papy holds the recipe", &[&alice, &desktop], async || {
        desktop.shows("+alice/recipe.txt", "flour\n")
    })
    .await;
    desktop.remove("+alice/recipe.txt");
    settle().await;
    alice.write("+alice/recipe.txt", "flour, eggs\n");
    settle().await;
    assert!(alice.shows("+alice/recipe.txt", "flour, eggs\n"));
    assert!(desktop.read("+alice/recipe.txt").is_none());
    assert!(desktop.aside().await.is_empty());
}

#[tokio::test]
async fn moving_a_folder_of_someone_elses_files_on_disk_copies_them_as_ones_own() {
    let internet = MemoryLookup::new();
    let (alice, desktop, _laptop) = family(&internet).await;
    let both = [&alice, &desktop];
    alice.write("photos/2024/beach.txt", "sand\n");
    desktop
        .run("selection", "follow", json!({"pattern": "/photos/"}))
        .await;
    eventually("papy holds the photo", &both, async || {
        desktop.shows("photos/2024/beach.txt", "sand\n")
    })
    .await;
    desktop.rename("photos/2024", "archive/2024");
    eventually("alice lists papy's copy", &both, async || {
        let files = alice.run("file", "list", json!({})).await;
        files
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "archive/2024/beach.txt" && file["owner"] == "papy")
    })
    .await;
    assert!(alice.shows("photos/2024/beach.txt", "sand\n"));
    assert!(desktop.shows("archive/2024/beach.txt", "sand\n"));
    assert_eq!(
        alice.history("photos/2024/beach.txt").await,
        [Some(content("sand\n"))]
    );
    assert!(desktop.aside().await.is_empty());
}
