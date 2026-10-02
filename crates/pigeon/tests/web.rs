//! The web UI of daemons on this host, driven as a browser drives it: its
//! forms, the pages that follow files and hear each change, the
//! configuration editor, and a file's page restoring versions and deciding
//! suggestions.

mod common;

use std::path::PathBuf;
use std::time::SystemTime;

use common::web::next_event;
use common::{GROUP, Machine, alice_with_notes, base64, eventually};
use iroh::address_lookup::MemoryLookup;
use pigeon_core::clock::{ntp_time, parse_rfc3339};
use serde_json::{Value, json};

#[tokio::test]
async fn web_forms_run_their_action_and_return() {
    let lookup = MemoryLookup::new();
    let peer = Machine::start("alice's machine", &lookup).await;
    peer.create("alice").await;
    let fields = [
        ("back", "/g/family"),
        ("group", GROUP),
        ("pattern", "/docs/"),
    ];
    let answer = peer.post_form("selection/follow", &fields).await;
    assert_eq!(answer.status, 303, "{}", answer.body);
    assert_eq!(answer.location.as_deref(), Some("/g/family"));
    assert_eq!(peer.selection().await, ["follow +alice/", "follow /docs/"]);
    let page = peer.page("/g/family/overview").await;
    assert!(page.contains("follow /docs/"), "{page}");
}

#[tokio::test]
async fn group_pages_follow_files_and_hear_each_change() {
    let lookup = MemoryLookup::new();
    let peer = Machine::start("alice's machine", &lookup).await;
    peer.create("alice").await;
    eventually("alice joined", &[], async || peer.joined().await).await;
    let script = peer
        .send("GET", "/live.js", vec![peer.cookie()], None)
        .await;
    assert_eq!(script.status, 200);
    assert!(script.body.contains("new EventSource("));
    let page = peer.page("/g/family").await;
    assert!(page.contains(r#"<script src="/live.js" defer></script>"#));
    assert!(page.contains(r#"<body data-group="family">"#));

    let lines = peer.listen().await;
    peer.call(
        "file",
        "write",
        json!({"path": "+alice/notes.txt", "content": base64("hello\n")}),
    )
    .await
    .unwrap();
    let lines = next_event(lines).await;

    let page = peer.page("/g/family?under=%2Balice").await;
    assert!(
        page.contains(r#"data-pattern="/+alice/notes.txt" data-state="checked" checked"#),
        "{page}"
    );
    let before = ntp_time(SystemTime::now());
    peer.call(
        "selection",
        "pin",
        json!({"pattern": "/+alice/notes.txt", "time": "now"}),
    )
    .await
    .unwrap();
    let after = ntp_time(SystemTime::now());
    drop(next_event(lines).await);
    let page = peer.page("/g/family?under=%2Balice").await;
    assert!(
        page.contains(r#"title="a copy kept here, pinned at a time: it no longer syncs">📌"#),
        "{page}"
    );
    let rules = peer.selection().await;
    let pinned = rules.last().unwrap().strip_prefix("pin ").unwrap();
    let time = parse_rfc3339(pinned.strip_suffix(" /+alice/notes.txt").unwrap()).unwrap();
    assert!((before..=after).contains(&time), "pinned at now: {rules:?}");
    let notes = peer.path("+alice/notes.txt");
    eventually("the notes are on disk", &[], async || notes.exists()).await;
    peer.call("selection", "free", json!({"pattern": "/+alice/notes.txt"}))
        .await
        .unwrap();
    eventually("the notes are freed", &[], async || !notes.exists()).await;
    let page = peer.page("/g/family").await;
    assert!(page.contains(r#"data-pattern="/+alice/" data-state="unchecked">"#));
    peer.call("selection", "follow", json!({"pattern": "/+alice/"}))
        .await
        .unwrap();
    assert_eq!(
        peer.selection().await,
        ["follow +alice/", "follow /+alice/"]
    );
    assert_eq!(
        peer.call("file", "pending", json!({})).await.unwrap(),
        json!([])
    );
    let error = peer.call("file", "publish", json!({})).await.unwrap_err();
    assert!(error.contains("no edit waits"), "{error}");
}

#[tokio::test]
async fn the_config_editor_previews_what_a_text_changes() {
    let lookup = MemoryLookup::new();
    let (peer, _) = alice_with_notes(&lookup).await;
    let page = peer.page("/g/family/overview").await;
    for part in [
        r#"<section id="config" data-keep"#,
        r#"<script src="/config_editor.js" defer></script>"#,
        "<h2>Places</h2>",
        "<h2>Members</h2>",
    ] {
        assert!(page.contains(part), "{part}: {page}");
    }
    let script = peer
        .send("GET", "/config_editor.js", vec![peer.cookie()], None)
        .await;
    assert_eq!(script.status, 200);
    assert!(script.body.contains("/config/preview"));

    let shown = peer.call("config", "show", json!({})).await.unwrap();
    let text = shown["text"].as_str().unwrap();
    let freeing = text.replace("\"follow +alice/\"", "\"free +alice/\"");
    assert_ne!(freeing, text);
    let (status, parts) = peer.preview(&freeing).await;
    assert_eq!(status, 200, "{parts}");
    assert_eq!(parts["version"], shown["version"]);
    assert_eq!(parts["save"], "Save: -6 B");
    assert!(parts["confirm"].is_string());
    let panel = parts["panel"].as_str().unwrap();
    assert!(
        panel.contains("matches 1 file · decides 1 (6 B)"),
        "{panel}"
    );
    assert!(
        panel.contains("warning: <code>+alice/</code> will no longer be here"),
        "{panel}"
    );
    assert!(panel.contains("free: -1 file, -6 B"), "{panel}");

    let pinning = text.replace("\"follow +alice/\"", "\"pin 2026-01-01T00:00:00Z +alice/\"");
    let preview = peer
        .call("config", "preview", json!({"text": pinning}))
        .await
        .unwrap();
    let times = &preview["rules"][0]["times"];
    assert_eq!(times.as_array().unwrap().len(), 1, "{preview}");
    assert_eq!(times[0]["files"], 1);
    let span = &preview["rules"][0]["span"];
    let start = usize::try_from(span[0].as_u64().unwrap()).unwrap();
    let end = usize::try_from(span[1].as_u64().unwrap()).unwrap();
    assert_eq!(&pinning[start..end], "\"pin 2026-01-01T00:00:00Z +alice/\"");
    let (_, pinned) = peer.preview(&pinning).await;
    assert!(pinned["panel"].as_str().unwrap().contains(r#"class="pin""#));
    let (status, invalid) = peer
        .preview(&text.replace("follow +alice/", "keep x"))
        .await;
    assert_ne!(status, 200);
    assert!(
        invalid["error"].as_str().unwrap().contains("not a mode"),
        "{invalid}"
    );
    let (_, unfinished) = peer
        .preview(&text.replace("follow +alice/", "pin +alice/"))
        .await;
    let fix = unfinished["fix"].as_str().unwrap();
    assert!(fix.contains(r#"data-pattern="+alice/""#), "{fix}");
    assert!(fix.contains(r#"data-files="1 file""#), "{fix}");
}

#[tokio::test]
async fn a_configuration_is_set_whole_once_what_it_frees_is_confirmed_as_the_editor_asks() {
    let lookup = MemoryLookup::new();
    let (peer, notes) = alice_with_notes(&lookup).await;
    let shown = peer.call("config", "show", json!({})).await.unwrap();
    let text = shown["text"].as_str().unwrap();
    let freeing = text.replace("\"follow +alice/\"", "\"free +alice/\"");
    let (_, parts) = peer.preview(&freeing).await;
    let set = |text: &str, version: &Value, yes: bool| json!({"text": text, "version": version, "yes": yes});
    let error = peer
        .call("config", "set", set(&freeing, &json!("stale"), true))
        .await
        .unwrap_err();
    assert!(error.contains("changed since"), "{error}");
    let error = peer
        .call("config", "set", set(&freeing, &shown["version"], false))
        .await
        .unwrap_err();
    assert!(
        error.contains("frees 1 file, 6 B") && error.contains("--yes"),
        "{error}"
    );
    let question = peer
        .question("config", "set", set(&freeing, &shown["version"], false))
        .await
        .unwrap();
    assert!(
        question.contains("family frees 1 file, 6 B") && question.ends_with("Apply them?"),
        "{question}"
    );
    assert_eq!(parts["confirm"], json!(question), "the web asks the same");
    let error = peer
        .call(
            "config",
            "set",
            set("member = 1\n", &shown["version"], true),
        )
        .await
        .unwrap_err();
    assert!(error.contains("member"), "{error}");
    assert!(notes.exists(), "a refused text changes nothing");
    peer.call("config", "set", set(&freeing, &shown["version"], true))
        .await
        .unwrap();
    assert_eq!(peer.selection().await, ["free +alice/"]);
    eventually("the notes are freed", &[], async || !notes.exists()).await;
    assert!(peer.joined().await);
}

#[tokio::test]
async fn the_web_restores_a_version_and_decides_the_suggestions_it_shows() {
    let lookup = MemoryLookup::new();
    let (peer, notes) = alice_with_notes(&lookup).await;
    peer.call(
        "file",
        "write",
        json!({"path": "+alice/notes.txt", "content": base64("hello again\n")}),
    )
    .await
    .unwrap();
    let read = |path: &PathBuf| std::fs::read_to_string(path).unwrap_or_default();
    eventually("the notes change", &[], async || {
        read(&notes) == "hello again\n"
    })
    .await;
    let history = peer
        .call("file", "history", json!({"path": "+alice/notes.txt"}))
        .await
        .unwrap();
    let first = history[0]["time"].as_str().unwrap().to_owned();
    let page = peer.page("/g/family/file?path=%2Balice/notes.txt").await;
    assert_eq!(
        page.matches(r#"action="/act/file/restore""#).count(),
        2,
        "{page}"
    );
    let back = "/g/family/file?path=%2Balice/notes.txt";
    let restore = [
        ("back", back),
        ("group", GROUP),
        ("pattern", "/+alice/notes.txt"),
        ("time", first.as_str()),
    ];
    let answer = peer.post_form("file/restore", &restore).await;
    assert_eq!(answer.status, 303, "{}", answer.body);
    assert_eq!(answer.location.as_deref(), Some(back));
    eventually("the first version is back", &[], async || {
        read(&notes) == "hello\n"
    })
    .await;

    peer.call("selection", "follow", json!({"pattern": "/docs/"}))
        .await
        .unwrap();
    peer.call(
        "file",
        "write",
        json!({"path": "docs/plan.txt", "content": base64("plan\n")}),
    )
    .await
    .unwrap();
    let plan = peer.path("docs/plan.txt");
    eventually("the plan is on disk", &[], async || read(&plan) == "plan\n").await;
    let mut ids = Vec::new();
    for text in ["first idea\n", "second idea\n"] {
        std::fs::write(&plan, text).unwrap();
        eventually("the edit is suggested", &[], async || {
            let suggestions = peer.call("suggestion", "list", json!({})).await.unwrap();
            suggestions.as_array().unwrap().len() == 1
                && suggestions[0]["changes"][0]["content"]["size"] == text.len()
        })
        .await;
        let suggestions = peer.call("suggestion", "list", json!({})).await.unwrap();
        ids.push(suggestions[0]["id"].as_str().unwrap().to_owned());
        let page = peer.page("/g/family?under=docs").await;
        assert!(page.contains("alice suggests a new version"), "{page}");
        assert!(page.contains("Files (1)"), "{page}");
    }
    let stale = [
        ("back", "/g/family"),
        ("group", GROUP),
        ("suggestions", ids[0].as_str()),
    ];
    let answer = peer.post_form("suggestion/discard", &stale).await;
    assert_ne!(
        answer.status, 303,
        "a suggestion changed since it was shown"
    );
    assert_eq!(read(&plan), "second idea\n");
    let shown = [
        ("back", "/g/family"),
        ("group", GROUP),
        ("suggestions", ids[1].as_str()),
    ];
    let answer = peer.post_form("suggestion/discard", &shown).await;
    assert_eq!(answer.status, 303, "{}", answer.body);
    eventually("the disk shows the group's plan again", &[], async || {
        read(&plan) == "plan\n"
    })
    .await;
    let suggestions = peer.call("suggestion", "list", json!({})).await.unwrap();
    assert_eq!(suggestions, json!([]));
}
