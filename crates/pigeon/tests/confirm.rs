//! Actions that cannot be undone or reach the whole group ask before they
//! go ahead, in the same words on the command line, the API and the web UI,
//! and change nothing until asked again with `yes`.

mod common;

use common::{GROUP, alice_with_notes, eventually, family};
use iroh::address_lookup::MemoryLookup;
use serde_json::json;

#[tokio::test]
async fn deleting_and_restoring_ask_and_change_nothing_until_yes() {
    let internet = MemoryLookup::new();
    let (alice, notes) = alice_with_notes(&internet).await;
    let delete = json!({"path": "+alice/notes.txt"});
    assert_eq!(
        alice.question("file", "delete", delete.clone()).await,
        Some(
            "Delete +alice/notes.txt? It deletes it for the whole group, and the history keeps it."
                .to_owned()
        )
    );
    let restore = json!({"pattern": "/+alice/notes.txt", "time": "now"});
    assert_eq!(
        alice.question("file", "restore", restore.clone()).await,
        Some("Restore /+alice/notes.txt as it was at now? It publishes it again as a new version, for the whole group.".to_owned())
    );
    assert!(notes.exists());
    assert!(alice.listed("+alice/notes.txt").await.is_some());
    let error = alice
        .call("file", "delete", delete.clone())
        .await
        .unwrap_err();
    assert!(error.ends_with("Pass --yes to go ahead."), "{error}");
    let error = alice
        .call(
            "file",
            "restore",
            json!({"pattern": "/+alice/notes.txt", "time": "now", "yes": true}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("already"), "{error}");
    alice
        .run(
            "file",
            "delete",
            json!({"path": "+alice/notes.txt", "yes": true}),
        )
        .await;
    eventually("the notes are deleted", &[&alice], async || !notes.exists()).await;
}

#[tokio::test]
async fn leaving_says_when_this_machine_holds_the_only_copy_of_the_history() {
    let internet = MemoryLookup::new();
    let (alice, _) = alice_with_notes(&internet).await;
    let question = alice.question("group", "leave", json!({})).await.unwrap();
    assert!(
        question.starts_with("Leave family on this machine?"),
        "{question}"
    );
    assert!(
        question.ends_with("leaving loses it for good."),
        "{question}"
    );
    assert!(alice.joined().await);

    let (alice, _desktop, _laptop) = family(&internet).await;
    eventually("alice knows the machines of papy", &[&alice], async || {
        let question = alice.question("group", "leave", json!({})).await.unwrap();
        !question.contains("for good")
    })
    .await;
    assert!(alice.joined().await);
    alice.run("group", "leave", json!({"yes": true})).await;
    assert_eq!(alice.run("group", "list", json!({})).await, json!([]));
}

#[tokio::test]
async fn the_web_asks_the_same_question_on_a_page_whose_form_says_yes() {
    let internet = MemoryLookup::new();
    let (alice, notes) = alice_with_notes(&internet).await;
    let fields = [
        ("back", "/g/family"),
        ("group", GROUP),
        ("path", "+alice/notes.txt"),
    ];
    let question = alice
        .question("file", "delete", json!({"path": "+alice/notes.txt"}))
        .await
        .unwrap();
    let answer = alice.post_form("file/delete", &fields).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(answer.body.contains(&question), "{}", answer.body);
    assert!(
        answer.body.contains(r#"name="yes" value="true""#),
        "{}",
        answer.body
    );
    assert!(
        answer
            .body
            .contains(r#"name="path" value="+alice/notes.txt""#),
        "{}",
        answer.body
    );
    assert!(notes.exists());
    let fields = [&fields[..], &[("yes", "true")]].concat();
    let answer = alice.post_form("file/delete", &fields).await;
    assert_eq!(answer.status, 303, "{}", answer.body);
    eventually("the notes are deleted", &[&alice], async || !notes.exists()).await;
}
