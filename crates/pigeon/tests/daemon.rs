//! Daemons on this host driven through the API as the command line drives
//! them: who may call, sharing files and deciding suggestions, joining,
//! hearing a group before choosing a name, leaving, reloading and setting
//! the configurations, and stopping with the event streams open.

mod common;

use std::time::Duration;

use common::{GROUP, Machine, alice_with_notes, base64, eventually};
use iroh::address_lookup::MemoryLookup;
use serde_json::json;

#[tokio::test]
async fn the_api_answers_only_localhost_calls_with_the_token() {
    let lookup = MemoryLookup::new();
    let peer = Machine::start("alice's machine", &lookup).await;
    let list = "/api/group/list";
    let anonymous = peer.send("POST", list, vec![], None).await;
    assert_eq!(anonymous.status, 401);
    let wrong = vec![("authorization", "Bearer nope".to_owned())];
    assert_eq!(peer.send("POST", list, wrong, None).await.status, 401);
    let rebound = vec![
        ("authorization", format!("Bearer {}", peer.token())),
        ("host", "evil.example".to_owned()),
    ];
    assert_eq!(peer.send("POST", list, rebound, None).await.status, 403);
    assert_eq!(peer.call("group", "list", json!({})).await, Ok(json!([])));
    let error = peer.call("file", "list", json!({})).await.unwrap_err();
    assert!(error.contains("`pigeon group create`"), "{error}");

    assert_eq!(peer.send("GET", "/", vec![], None).await.status, 401);
    let open = format!("/open?token={}", peer.token());
    let open = peer.send("GET", &open, vec![], None).await;
    assert_eq!(open.status, 303);
    let cookie = open.cookie.unwrap();
    assert!(cookie.starts_with(&format!("pigeon_token={};", peer.token())));
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    assert!(cookie.contains("Max-Age=34560000"));
    let home = peer.page("/").await;
    assert!(home.contains(r#"action="/act/group/create""#));
    assert!(home.contains(r#"action="/act/group/join""#));
}

#[tokio::test]
async fn two_daemons_share_files_and_decide_suggestions() {
    let lookup = MemoryLookup::new();
    let a = Machine::start("alice's machine", &lookup).await;
    let b = Machine::start("bob's machine", &lookup).await;
    let key = a.create("alice").await;
    b.call(
        "group",
        "join",
        json!({"key": key, "member": "bob", "root": b.root()}),
    )
    .await
    .unwrap();
    eventually("both joined", &[], async || {
        a.joined().await && b.joined().await
    })
    .await;

    let wrote = a
        .call(
            "file",
            "write",
            json!({"path": "+alice/notes.txt", "content": base64("hello\n")}),
        )
        .await
        .unwrap();
    assert_eq!(wrote, json!({"published": ["+alice/notes.txt"]}));
    b.call("selection", "follow", json!({"pattern": "/+alice/"}))
        .await
        .unwrap();
    let on_b = b.path("+alice/notes.txt");
    eventually("bob holds the note", &[], async || {
        std::fs::read_to_string(&on_b).ok().as_deref() == Some("hello\n")
    })
    .await;

    std::fs::write(&on_b, "bonjour\n").unwrap();
    eventually("alice sees bob's suggestion", &[], async || {
        let suggestions = a.call("suggestion", "list", json!({})).await.unwrap();
        suggestions.as_array().unwrap().len() == 1
    })
    .await;
    eventually("alice reviews the difference", &[], async || {
        let page = a.page("/g/family/file?path=%2Balice/notes.txt").await;
        page.contains("- hello") && page.contains("+ bonjour")
    })
    .await;
    let page = a.page("/g/family/file?path=%2Balice/notes.txt").await;
    assert!(
        page.contains("bob suggests a new version, as the rules leave it to the group"),
        "{page}"
    );
    assert!(
        page.contains(r#"action="/act/suggestion/validate""#),
        "{page}"
    );
    let page = a.page("/g/family").await;
    assert!(page.contains("Files (1)"), "{page}");
    assert!(page.contains("📬 bob"), "{page}");
    let suggestions = a.call("suggestion", "list", json!({})).await.unwrap();
    assert_eq!(
        a.question(
            "suggestion",
            "validate",
            json!({"suggestions": suggestions[0]["id"]})
        )
        .await
        .as_deref(),
        Some("Validate this suggestion? It publishes at once, for the whole group.")
    );
    a.call(
        "suggestion",
        "validate",
        json!({"suggestions": suggestions[0]["id"], "yes": true}),
    )
    .await
    .unwrap();
    let on_a = a.path("+alice/notes.txt");
    eventually("both hold the validated note", &[], async || {
        std::fs::read_to_string(&on_a).ok().as_deref() == Some("bonjour\n")
            && std::fs::read_to_string(&on_b).ok().as_deref() == Some("bonjour\n")
    })
    .await;
    let history = a
        .call("file", "history", json!({"path": "+alice/notes.txt"}))
        .await
        .unwrap();
    assert_eq!(history.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn joining_with_a_members_name_adds_a_machine_of_theirs() {
    let lookup = MemoryLookup::new();
    let a = Machine::start("alice's machine", &lookup).await;
    let b = Machine::start("bob's machine", &lookup).await;
    let key = a.create("alice").await;
    assert!(a.joined().await);
    b.call(
        "group",
        "join",
        json!({"key": key, "member": "alice", "root": b.root()}),
    )
    .await
    .unwrap();
    assert!(b.joined().await);
    let members = b.call("member", "list", json!({})).await.unwrap();
    assert_eq!(members.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_machine_hears_the_group_before_choosing_its_name() {
    let lookup = MemoryLookup::new();
    let a = Machine::start("alice's machine", &lookup).await;
    let b = Machine::start("bob's machine", &lookup).await;
    let key = a.create("alice").await;
    eventually("alice joined", &[], async || a.joined().await).await;
    a.call(
        "file",
        "write",
        json!({"path": "docs/+carol/plan.txt", "content": base64("plan\n")}),
    )
    .await
    .unwrap();
    let names = b.call("group", "names", json!({"key": key})).await.unwrap();
    assert_eq!(names["heard"], true, "{names}");
    assert_eq!(names["members"], json!(["alice"]));
    eventually("the plan names carol", &[], async || {
        let names = b.call("group", "names", json!({"key": key})).await.unwrap();
        names["taken"] == json!(["alice", "carol"])
    })
    .await;
    assert_eq!(b.call("group", "list", json!({})).await, Ok(json!([])));
    b.call(
        "group",
        "join",
        json!({"key": key, "member": "bob", "root": b.root()}),
    )
    .await
    .unwrap();
    assert!(b.joined().await);
    let error = b
        .call("group", "names", json!({"key": key}))
        .await
        .unwrap_err();
    assert!(error.contains("already in the group family"), "{error}");
}

#[tokio::test]
async fn a_group_that_does_not_start_says_why_and_leaving_forgets_any_keeping_its_files() {
    let lookup = MemoryLookup::new();
    let (mut peer, notes) = alice_with_notes(&lookup).await;
    let page = peer.page("/g/family/overview").await;
    assert!(page.contains(r#"action="/act/group/leave""#), "{page}");
    assert!(
        page.contains("<td>alice (you)</td><td>1</td><td>1</td>"),
        "{page}"
    );
    let home = peer.home();
    let broken = home.group("broken");
    std::fs::create_dir_all(broken.config()).unwrap();
    std::fs::copy(home.group(GROUP).config_path(), broken.config_path()).unwrap();
    std::fs::create_dir_all(broken.data()).unwrap();
    std::fs::write(broken.secrets_path(), "machine = \"00\"\n[cert]\n").unwrap();
    peer.switch_off().await;
    peer.switch_on().await;
    let groups = peer.call("group", "list", json!({})).await.unwrap();
    let state = groups[0]["join"]["state"].as_str().unwrap();
    assert!(
        groups[0]["name"] == "broken"
            && state.starts_with("does not start: ")
            && state.contains("cert"),
        "{groups}"
    );
    assert_eq!(groups[1]["name"], GROUP, "{groups}");
    let error = peer
        .call("file", "list", json!({"group": "broken"}))
        .await
        .unwrap_err();
    assert!(
        error.contains("the group broken does not start: ") && error.contains("cert"),
        "{error}"
    );
    assert!(
        error.contains("`pigeon group leave --group broken`"),
        "{error}"
    );
    let error = peer.call("file", "list", json!({})).await.unwrap_err();
    assert!(error.contains("one of broken, family"), "{error}");
    let page = peer
        .send("GET", "/g/broken", vec![peer.cookie()], None)
        .await;
    assert_eq!(page.status, 400);
    assert!(
        page.body.contains("the group broken does not start"),
        "{}",
        page.body
    );
    peer.call("group", "leave", json!({"group": "broken", "yes": true}))
        .await
        .unwrap();
    assert!(!broken.config().exists() && !broken.data().exists());
    peer.call("group", "leave", json!({"yes": true}))
        .await
        .unwrap();
    assert_eq!(
        peer.call("group", "list", json!({})).await.unwrap(),
        json!([])
    );
    let family = home.group(GROUP);
    assert!(!family.config().exists() && !family.data().exists());
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello\n");
    let error = peer
        .call("group", "leave", json!({"group": GROUP}))
        .await
        .unwrap_err();
    assert!(error.contains("no group family"), "{error}");
}

#[tokio::test]
async fn the_daemon_stops_with_event_streams_open_and_restarts_only_onto_another_program() {
    let lookup = MemoryLookup::new();
    let mut peer = Machine::start("alice's machine", &lookup).await;
    peer.create("alice").await;
    let page = peer.page("/g/family").await;
    assert!(page.contains(r#"<p id="updated" class="notice" hidden>"#));
    let lines = peer.listen().await;
    assert_eq!(
        peer.call("daemon", "restart", json!({})).await,
        Ok(json!({"restarts": false}))
    );
    tokio::time::timeout(Duration::from_secs(10), peer.switch_off())
        .await
        .expect("the server stops with the stream open");
    tokio::task::spawn_blocking(move || while lines.recv().is_ok() {})
        .await
        .unwrap();
}

#[tokio::test]
async fn a_group_that_does_not_start_from_a_configuration_runs_on_as_it_was() {
    let lookup = MemoryLookup::new();
    let (peer, notes) = alice_with_notes(&lookup).await;
    let shown = peer.call("config", "show", json!({})).await.unwrap();
    let text = shown["text"].as_str().unwrap();
    let set = |text: &str| json!({"text": text, "version": shown["version"], "yes": true});
    let blocker = peer.outside("blocker");
    std::fs::write(&blocker, "").unwrap();
    let unrooted: String = text
        .lines()
        .map(|line| {
            if line.starts_with("root = ") {
                format!("root = {:?}\n", blocker.join("root"))
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    assert_ne!(unrooted, text);
    let error = peer
        .call("config", "set", set(&unrooted))
        .await
        .unwrap_err();
    assert!(error.contains("runs on with the one it had"), "{error}");
    let config = peer.home().group(GROUP).config_path();
    assert_eq!(std::fs::read_to_string(config).unwrap(), text);
    assert!(
        peer.joined().await,
        "a group that does not start runs on as it was"
    );
    assert!(notes.exists());
}

#[tokio::test]
async fn reloading_applies_the_configurations_edited_by_hand_unless_one_is_invalid() {
    let lookup = MemoryLookup::new();
    let (peer, notes) = alice_with_notes(&lookup).await;
    let home = peer.home();
    let config = home.group(GROUP).config_path();
    let text = std::fs::read_to_string(&config).unwrap();
    let edited = text
        .replace("member = \"alice\"", "member = \"carol\"")
        .replace("\"follow +alice/\"", "\"free +alice/\", \"free *.iso\"")
        .replace("quota = 20", "quota = 7");
    assert_ne!(edited, text);
    std::fs::write(&config, &edited).unwrap();
    let invalid = home.group("other").config_path();
    std::fs::create_dir_all(invalid.parent().unwrap()).unwrap();
    std::fs::write(&invalid, "member = 1\n").unwrap();
    let member = async || {
        let status = peer.call("group", "status", json!({})).await.unwrap();
        status["member"].clone()
    };
    let error = peer.call("daemon", "reload", json!({})).await.unwrap_err();
    assert!(error.contains(&invalid.display().to_string()), "{error}");
    assert_eq!(member().await, "alice", "a refused reload changes nothing");
    std::fs::remove_dir_all(invalid.parent().unwrap()).unwrap();
    let error = peer.call("daemon", "reload", json!({})).await.unwrap_err();
    assert!(
        error.contains("family frees 1 file, 6 B") && error.contains("--yes"),
        "{error}"
    );
    let question = peer.question("daemon", "reload", json!({})).await.unwrap();
    assert!(question.contains("family frees 1 file, 6 B"), "{question}");
    assert_eq!(member().await, "alice");
    assert!(notes.exists());
    assert_eq!(
        peer.call("daemon", "reload", json!({"yes": true})).await,
        Ok(json!([{"group": GROUP, "download": "", "free": "1 file, 6 B", "pin": ""}]))
    );
    assert_eq!(member().await, "carol");
    assert_eq!(peer.selection().await, ["free +alice/", "free *.iso"]);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), edited);
    eventually("the notes are freed", &[], async || !notes.exists()).await;
}
