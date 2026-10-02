//! Daemons on this host, each with its own pigeon folder, driven through
//! the API and the web UI as the command line and a browser drive them,
//! including the event stream that keeps a group's pages live and ends
//! when the daemon stops.

use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use data_encoding::BASE64;
use iroh::address_lookup::MemoryLookup;
use pigeon::api::{App, serve};
use pigeon::catalog::{ACTIONS, Kind};
use pigeon::client::call_at;
use pigeon::daemon::{Daemon, Stop};
use pigeon::home::Home;
use pigeon_sync::{Network, Options};
use serde_json::{Map, Value, json};
use tempfile::TempDir;

struct Peer {
    address: SocketAddr,
    token: String,
    dir: TempDir,
    app: Arc<App>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
}

/// What a raw HTTP request got back.
struct Answer {
    status: u16,
    location: Option<String>,
    cookie: Option<String>,
    body: String,
}

fn options(lookup: &MemoryLookup) -> Options {
    Options {
        network: Network::Local(lookup.clone()),
        settle_personal: Duration::from_millis(100),
        settle_drop: Duration::from_millis(400),
        rescan: Duration::from_secs(60),
        tick: Duration::from_millis(50),
        join_delay: Duration::from_millis(300),
        gc: Duration::from_millis(300),
        ..Options::default()
    }
}

impl Peer {
    async fn start(lookup: &MemoryLookup) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path().join("home"));
        let token = home.token().unwrap();
        let daemon = Daemon::start(home, options(lookup)).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut stopping = daemon.stopping();
        let app = Arc::new(App {
            daemon,
            token: token.clone(),
        });
        let stop = async move {
            let _ = stopping.wait_for(Option::is_some).await;
        };
        let server = tokio::spawn(serve(app.clone(), listener, stop));
        Self {
            address,
            token,
            dir,
            app,
            server,
        }
    }

    fn root(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Sends one request to the server, following no redirect.
    async fn send(
        &self,
        method: &'static str,
        path: &str,
        headers: Vec<(&'static str, String)>,
        body: Option<(String, Vec<u8>)>,
    ) -> Answer {
        let url = format!("http://{}{path}", self.address);
        tokio::task::spawn_blocking(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .build()
                .into();
            let mut request = ureq::http::Request::builder().method(method).uri(&url);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let mut response = match body {
                Some((kind, bytes)) => agent
                    .run(request.header("content-type", kind).body(bytes).unwrap())
                    .unwrap(),
                None => agent.run(request.body(()).unwrap()).unwrap(),
            };
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .map(|value| value.to_str().unwrap().to_owned())
            };
            Answer {
                status: response.status().as_u16(),
                location: header("location"),
                cookie: header("set-cookie"),
                body: response.body_mut().read_to_string().unwrap(),
            }
        })
        .await
        .unwrap()
    }

    fn cookie(&self) -> (&'static str, String) {
        ("cookie", format!("pigeon_token={}", self.token))
    }

    async fn call(&self, noun: &str, verb: &str, args: Value) -> Result<Value, String> {
        let Value::Object(args) = args else {
            unreachable!()
        };
        let (address, token) = (self.address, self.token.clone());
        let (noun, verb) = (noun.to_owned(), verb.to_owned());
        tokio::task::spawn_blocking(move || call_at(address, &token, &noun, &verb, &args))
            .await
            .unwrap()
            .map_err(|error| error.to_string())
    }

    async fn page(&self, path: &str) -> String {
        let answer = self.send("GET", path, vec![self.cookie()], None).await;
        assert_eq!(answer.status, 200, "{path}");
        answer.body
    }

    async fn joined(&self) -> bool {
        let status = self.call("group", "status", json!({})).await.unwrap();
        status["join"]["state"] == "joined"
    }
}

/// Waits up to twenty seconds for `condition`.
async fn eventually<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn base64(text: &str) -> String {
    BASE64.encode(text.as_bytes())
}

/// The selection lines of the config.toml of `peer`'s only group.
async fn selection(peer: &Peer) -> Vec<String> {
    let shown = peer.call("config", "show", json!({})).await.unwrap();
    let config: toml::Table = shown["text"].as_str().unwrap().parse().unwrap();
    config["selection"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn the_api_answers_only_localhost_calls_with_the_token() {
    let lookup = MemoryLookup::new();
    let peer = Peer::start(&lookup).await;
    let list = "/api/group/list";
    let anonymous = peer.send("POST", list, vec![], None).await;
    assert_eq!(anonymous.status, 401);
    let wrong = vec![("authorization", "Bearer nope".to_owned())];
    assert_eq!(peer.send("POST", list, wrong, None).await.status, 401);
    let rebound = vec![
        ("authorization", format!("Bearer {}", peer.token)),
        ("host", "evil.example".to_owned()),
    ];
    assert_eq!(peer.send("POST", list, rebound, None).await.status, 403);
    assert_eq!(peer.call("group", "list", json!({})).await, Ok(json!([])));
    let error = peer.call("file", "list", json!({})).await.unwrap_err();
    assert!(error.contains("`pigeon group create`"), "{error}");

    assert_eq!(peer.send("GET", "/", vec![], None).await.status, 401);
    let open = format!("/open?token={}", peer.token);
    let open = peer.send("GET", &open, vec![], None).await;
    assert_eq!(open.status, 303);
    let cookie = open.cookie.unwrap();
    assert!(cookie.starts_with(&format!("pigeon_token={};", peer.token)));
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    assert!(cookie.contains("Max-Age=34560000"));
    let home = peer.page("/").await;
    assert!(home.contains(r#"action="/act/group/create""#));
    assert!(home.contains(r#"action="/act/group/join""#));
}

#[tokio::test]
async fn two_daemons_share_files_and_decide_suggestions() {
    let lookup = MemoryLookup::new();
    let a = Peer::start(&lookup).await;
    let b = Peer::start(&lookup).await;
    let created = a
        .call(
            "group",
            "create",
            json!({"name": "cheapmo", "member": "alice", "root": a.root("cheapmo")}),
        )
        .await
        .unwrap();
    let key = created["key"].as_str().unwrap();
    b.call(
        "group",
        "join",
        json!({"key": key, "member": "bob", "root": b.root("cheapmo")}),
    )
    .await
    .unwrap();
    eventually("both joined", async || a.joined().await && b.joined().await).await;

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
    let on_b = b.root("cheapmo").join("+alice/notes.txt");
    eventually("bob holds the note", async || {
        std::fs::read_to_string(&on_b).ok().as_deref() == Some("hello\n")
    })
    .await;

    std::fs::write(&on_b, "bonjour\n").unwrap();
    eventually("alice sees bob's suggestion", async || {
        let suggestions = a.call("suggestion", "list", json!({})).await.unwrap();
        suggestions.as_array().unwrap().len() == 1
    })
    .await;
    eventually("alice reviews the difference", async || {
        let page = a.page("/g/cheapmo/file?path=%2Balice/notes.txt").await;
        page.contains("- hello") && page.contains("+ bonjour")
    })
    .await;
    let page = a.page("/g/cheapmo/file?path=%2Balice/notes.txt").await;
    assert!(
        page.contains("bob suggests a new version, as the rules leave it to the group"),
        "{page}"
    );
    assert!(
        page.contains(r#"action="/act/suggestion/validate""#),
        "{page}"
    );
    let page = a.page("/g/cheapmo/files").await;
    assert!(page.contains("Files (1)"), "{page}");
    assert!(page.contains("📬 bob"), "{page}");
    let suggestions = a.call("suggestion", "list", json!({})).await.unwrap();
    a.call(
        "suggestion",
        "validate",
        json!({"suggestions": suggestions[0]["id"]}),
    )
    .await
    .unwrap();
    let on_a = a.root("cheapmo").join("+alice/notes.txt");
    eventually("both hold the validated note", async || {
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
    let a = Peer::start(&lookup).await;
    let b = Peer::start(&lookup).await;
    let created = a
        .call(
            "group",
            "create",
            json!({"name": "cheapmo", "member": "alice", "root": a.root("cheapmo")}),
        )
        .await
        .unwrap();
    assert!(a.joined().await);
    let key = created["key"].as_str().unwrap();
    b.call(
        "group",
        "join",
        json!({"key": key, "member": "alice", "root": b.root("cheapmo")}),
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
    let a = Peer::start(&lookup).await;
    let b = Peer::start(&lookup).await;
    let created = a
        .call(
            "group",
            "create",
            json!({"name": "cheapmo", "member": "alice", "root": a.root("cheapmo")}),
        )
        .await
        .unwrap();
    eventually("alice joined", async || a.joined().await).await;
    a.call(
        "file",
        "write",
        json!({"path": "docs/+carol/plan.txt", "content": base64("plan\n")}),
    )
    .await
    .unwrap();
    let key = created["key"].as_str().unwrap();
    let names = b.call("group", "names", json!({"key": key})).await.unwrap();
    assert_eq!(names["heard"], true, "{names}");
    assert_eq!(names["members"], json!(["alice"]));
    eventually("the plan names carol", async || {
        let names = b.call("group", "names", json!({"key": key})).await.unwrap();
        names["taken"] == json!(["alice", "carol"])
    })
    .await;
    assert_eq!(b.call("group", "list", json!({})).await, Ok(json!([])));
    b.call(
        "group",
        "join",
        json!({"key": key, "member": "bob", "root": b.root("cheapmo")}),
    )
    .await
    .unwrap();
    assert!(b.joined().await);
    let error = b
        .call("group", "names", json!({"key": key}))
        .await
        .unwrap_err();
    assert!(error.contains("already in the group cheapmo"), "{error}");
}

/// Posts a web form of `fields` to `/act/<action>`, as a browser does.
async fn post_form(peer: &Peer, action: &str, fields: &[(&str, &str)]) -> Answer {
    let boundary = "pigeonboundary";
    let parts: Vec<String> = fields
        .iter()
        .map(|(name, value)| {
            format!(
                "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
        })
        .collect();
    let form = format!("{}--{boundary}--\r\n", parts.concat());
    let kind = format!("multipart/form-data; boundary={boundary}");
    peer.send(
        "POST",
        &format!("/act/{action}"),
        vec![peer.cookie()],
        Some((kind, form.into_bytes())),
    )
    .await
}

#[tokio::test]
async fn web_forms_run_their_action_and_return() {
    let lookup = MemoryLookup::new();
    let peer = Peer::start(&lookup).await;
    peer.call(
        "group",
        "create",
        json!({"name": "cheapmo", "member": "alice", "root": peer.root("cheapmo")}),
    )
    .await
    .unwrap();
    let fields = [
        ("back", "/g/cheapmo/files"),
        ("group", "cheapmo"),
        ("pattern", "/docs/"),
    ];
    let answer = post_form(&peer, "selection/follow", &fields).await;
    assert_eq!(answer.status, 303, "{}", answer.body);
    assert_eq!(answer.location.as_deref(), Some("/g/cheapmo/files"));
    assert_eq!(selection(&peer).await, ["follow +alice/", "follow /docs/"]);
    let page = peer.page("/g/cheapmo").await;
    assert!(page.contains("follow /docs/"), "{page}");
}

fn dummy(kind: Kind, peer: &Peer) -> Value {
    match kind {
        Kind::Text => json!("x"),
        Kind::Path => json!("+alice/dummy.txt"),
        Kind::Pattern => json!("/dummy/"),
        Kind::Folder => json!(peer.root("dummy")),
        Kind::Bytes => json!(base64("dummy")),
        Kind::Document => json!("member = \"alice\"\n"),
        Kind::Time => json!("2026-01-01T00:00:00Z"),
        Kind::Flag => json!(false),
    }
}

#[tokio::test]
async fn every_action_of_the_catalog_is_carried_out() {
    let lookup = MemoryLookup::new();
    let peer = Peer::start(&lookup).await;
    peer.call(
        "group",
        "create",
        json!({"name": "cheapmo", "member": "alice", "root": peer.root("cheapmo")}),
    )
    .await
    .unwrap();
    for action in ACTIONS {
        let mut args = Map::new();
        for param in action.params {
            args.insert(param.name.to_owned(), dummy(param.kind, &peer));
        }
        if action.scope == pigeon::catalog::Scope::Group {
            args.insert("group".to_owned(), json!("cheapmo"));
        }
        let result = peer
            .call(action.noun, action.verb, Value::Object(args))
            .await;
        if let Err(error) = result {
            assert!(
                !error.contains("not implemented"),
                "{}: {error}",
                action.command()
            );
        }
    }
}

#[tokio::test]
async fn leaving_forgets_the_group_here_and_keeps_its_files_even_when_it_does_not_start() {
    let lookup = MemoryLookup::new();
    let (peer, notes) = alice_with_notes(&lookup).await;
    let page = peer.page("/g/cheapmo").await;
    assert!(page.contains(r#"action="/act/group/leave""#), "{page}");
    assert!(
        page.contains("<td>alice (you)</td><td>1</td><td>1</td>"),
        "{page}"
    );
    let home = Home::new(peer.dir.path().join("home"));
    let broken = home.group("broken");
    std::fs::create_dir_all(broken.config()).unwrap();
    std::fs::copy(home.group("cheapmo").config_path(), broken.config_path()).unwrap();
    std::fs::create_dir_all(broken.data()).unwrap();
    std::fs::write(broken.secrets_path(), "machine = \"00\"\n[cert]\n").unwrap();
    peer.call("group", "leave", json!({"group": "broken"}))
        .await
        .unwrap();
    assert!(!broken.config().exists() && !broken.data().exists());
    peer.call("group", "leave", json!({})).await.unwrap();
    assert_eq!(
        peer.call("group", "list", json!({})).await.unwrap(),
        json!([])
    );
    let cheapmo = home.group("cheapmo");
    assert!(!cheapmo.config().exists() && !cheapmo.data().exists());
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello\n");
    let error = peer
        .call("group", "leave", json!({"group": "cheapmo"}))
        .await
        .unwrap_err();
    assert!(error.contains("no group cheapmo"), "{error}");
}

/// The next line of an event stream.
fn next_line(lines: &Receiver<String>) -> String {
    lines.recv_timeout(Duration::from_secs(20)).unwrap()
}

/// Opens the event stream of `group`, checks that it is one and that it
/// first names the program serving it, and returns its lines, lowercased.
async fn listen(peer: &Peer, group: &str) -> Receiver<String> {
    let (sender, lines) = std::sync::mpsc::channel();
    let (address, cookie) = (peer.address, peer.cookie());
    let request = format!(
        "GET /g/{group}/events HTTP/1.1\r\nhost: {address}\r\n{}: {}\r\n\r\n",
        cookie.0, cookie.1
    );
    std::thread::spawn(move || {
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                return;
            };
            if sender.send(line.to_lowercase()).is_err() {
                return;
            }
        }
    });
    tokio::task::spawn_blocking(move || {
        assert_eq!(next_line(&lines), "http/1.1 200 ok");
        let mut kind = String::new();
        loop {
            let line = next_line(&lines);
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("content-type: ") {
                value.clone_into(&mut kind);
            }
        }
        assert_eq!(kind, "text/event-stream");
        while next_line(&lines) != "event: program" {}
        let program = next_line(&lines);
        assert!(
            program
                .strip_prefix("data: ")
                .is_some_and(|hash| hash.len() == 64),
            "{program}"
        );
        lines
    })
    .await
    .unwrap()
}

/// Waits for the next event of a stream.
async fn next_event(lines: Receiver<String>) -> Receiver<String> {
    tokio::task::spawn_blocking(move || {
        while !next_line(&lines).starts_with("data:") {}
        lines
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn group_pages_follow_files_and_hear_each_change() {
    let lookup = MemoryLookup::new();
    let peer = Peer::start(&lookup).await;
    peer.call(
        "group",
        "create",
        json!({"name": "cheapmo", "member": "alice", "root": peer.root("cheapmo")}),
    )
    .await
    .unwrap();
    eventually("alice joined", async || peer.joined().await).await;
    let script = peer
        .send("GET", "/live.js", vec![peer.cookie()], None)
        .await;
    assert_eq!(script.status, 200);
    assert!(script.body.contains("new EventSource("));
    let page = peer.page("/g/cheapmo/files").await;
    assert!(page.contains(r#"<script src="/live.js" defer></script>"#));
    assert!(page.contains(r#"<body data-group="cheapmo">"#));

    let lines = listen(&peer, "cheapmo").await;
    peer.call(
        "file",
        "write",
        json!({"path": "+alice/notes.txt", "content": base64("hello\n")}),
    )
    .await
    .unwrap();
    let lines = next_event(lines).await;

    let page = peer.page("/g/cheapmo/files?under=%2Balice").await;
    assert!(
        page.contains(r#"data-pattern="/+alice/notes.txt" data-state="checked" checked"#),
        "{page}"
    );
    peer.call(
        "selection",
        "unfollow",
        json!({"pattern": "/+alice/notes.txt"}),
    )
    .await
    .unwrap();
    drop(next_event(lines).await);
    let page = peer.page("/g/cheapmo/files?under=%2Balice").await;
    assert!(
        page.contains(r#"title="a copy kept here, frozen: it no longer syncs">🧊"#),
        "{page}"
    );
    let page = peer.page("/g/cheapmo/files").await;
    assert!(page.contains(r#"data-pattern="/+alice/" data-state="unchecked">"#));
    peer.call("selection", "follow", json!({"pattern": "/+alice/"}))
        .await
        .unwrap();
    assert_eq!(
        selection(&peer).await,
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
async fn the_daemon_stops_with_event_streams_open_and_restarts_only_onto_another_program() {
    let lookup = MemoryLookup::new();
    let peer = Peer::start(&lookup).await;
    peer.call(
        "group",
        "create",
        json!({"name": "cheapmo", "member": "alice", "root": peer.root("cheapmo")}),
    )
    .await
    .unwrap();
    let page = peer.page("/g/cheapmo").await;
    assert!(page.contains(r#"<p id="updated" class="notice" hidden>"#));
    let lines = listen(&peer, "cheapmo").await;
    assert_eq!(
        peer.call("daemon", "restart", json!({})).await,
        Ok(json!({"restarts": false}))
    );
    peer.app.daemon.stop(Stop::Quit);
    tokio::time::timeout(Duration::from_secs(10), peer.server)
        .await
        .expect("the server stops with the stream open")
        .unwrap()
        .unwrap();
    tokio::task::spawn_blocking(move || while lines.recv().is_ok() {})
        .await
        .unwrap();
}

/// `text` as a value of a URL-encoded form.
fn form_value(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// What the configuration editor of the group cheapmo shows of `text`.
async fn editor_preview(peer: &Peer, text: &str) -> (u16, Value) {
    let answer = peer
        .send(
            "POST",
            "/g/cheapmo/config/preview",
            vec![peer.cookie()],
            Some((
                "application/x-www-form-urlencoded".to_owned(),
                format!("text={}", form_value(text)).into_bytes(),
            )),
        )
        .await;
    (answer.status, serde_json::from_str(&answer.body).unwrap())
}

/// A daemon whose member alice created cheapmo and wrote a note of 6
/// bytes, which is on disk, and the note's path there.
async fn alice_with_notes(lookup: &MemoryLookup) -> (Peer, PathBuf) {
    let peer = Peer::start(lookup).await;
    peer.call(
        "group",
        "create",
        json!({"name": "cheapmo", "member": "alice", "root": peer.root("cheapmo")}),
    )
    .await
    .unwrap();
    eventually("alice joined", async || peer.joined().await).await;
    peer.call(
        "file",
        "write",
        json!({"path": "+alice/notes.txt", "content": base64("hello\n")}),
    )
    .await
    .unwrap();
    let notes = peer.root("cheapmo").join("+alice/notes.txt");
    eventually("the notes are on disk", async || notes.exists()).await;
    (peer, notes)
}

#[tokio::test]
async fn the_config_editor_previews_a_text_and_saves_it_whole() {
    let lookup = MemoryLookup::new();
    let (peer, notes) = alice_with_notes(&lookup).await;
    let page = peer.page("/g/cheapmo").await;
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
    let (status, parts) = editor_preview(&peer, &freeing).await;
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
    let (_, pinned) = editor_preview(&peer, &pinning).await;
    assert!(pinned["panel"].as_str().unwrap().contains(r#"class="pin""#));
    let (status, invalid) = editor_preview(&peer, &text.replace("follow +alice/", "keep x")).await;
    assert_ne!(status, 200);
    assert!(
        invalid["error"].as_str().unwrap().contains("not a mode"),
        "{invalid}"
    );
    let (_, unfinished) =
        editor_preview(&peer, &text.replace("follow +alice/", "pin +alice/")).await;
    let fix = unfinished["fix"].as_str().unwrap();
    assert!(fix.contains(r#"data-pattern="+alice/""#), "{fix}");
    assert!(fix.contains(r#"data-files="1 file""#), "{fix}");

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
    assert_eq!(selection(&peer).await, ["free +alice/"]);
    eventually("the notes are freed", async || !notes.exists()).await;
    assert!(peer.joined().await);
}

#[tokio::test]
async fn reloading_applies_the_configurations_edited_by_hand_unless_one_is_invalid() {
    let lookup = MemoryLookup::new();
    let (peer, notes) = alice_with_notes(&lookup).await;
    let home = Home::new(peer.dir.path().join("home"));
    let config = home.group("cheapmo").config_path();
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
        error.contains("cheapmo frees 1 file, 6 B") && error.contains("--yes"),
        "{error}"
    );
    assert_eq!(member().await, "alice");
    assert!(notes.exists());
    assert_eq!(
        peer.call("daemon", "reload", json!({"yes": true})).await,
        Ok(json!([{"group": "cheapmo", "download": "", "free": "1 file, 6 B", "freeze": ""}]))
    );
    assert_eq!(member().await, "carol");
    assert_eq!(selection(&peer).await, ["free +alice/", "free *.iso"]);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), edited);
    eventually("the notes are freed", async || !notes.exists()).await;
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
    eventually("the notes change", async || read(&notes) == "hello again\n").await;
    let history = peer
        .call("file", "history", json!({"path": "+alice/notes.txt"}))
        .await
        .unwrap();
    let first = history[0]["time"].as_str().unwrap().to_owned();
    let page = peer.page("/g/cheapmo/file?path=%2Balice/notes.txt").await;
    assert_eq!(
        page.matches(r#"action="/act/file/restore""#).count(),
        2,
        "{page}"
    );
    let back = "/g/cheapmo/file?path=%2Balice/notes.txt";
    let restore = [
        ("back", back),
        ("group", "cheapmo"),
        ("pattern", "/+alice/notes.txt"),
        ("time", first.as_str()),
    ];
    let answer = post_form(&peer, "file/restore", &restore).await;
    assert_eq!(answer.status, 303, "{}", answer.body);
    assert_eq!(answer.location.as_deref(), Some(back));
    eventually("the first version is back", async || {
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
    let plan = peer.root("cheapmo").join("docs/plan.txt");
    eventually("the plan is on disk", async || read(&plan) == "plan\n").await;
    let mut ids = Vec::new();
    for text in ["first idea\n", "second idea\n"] {
        std::fs::write(&plan, text).unwrap();
        eventually("the edit is suggested", async || {
            let suggestions = peer.call("suggestion", "list", json!({})).await.unwrap();
            suggestions.as_array().unwrap().len() == 1
                && suggestions[0]["changes"][0]["content"]["size"] == text.len()
        })
        .await;
        let suggestions = peer.call("suggestion", "list", json!({})).await.unwrap();
        ids.push(suggestions[0]["id"].as_str().unwrap().to_owned());
        let page = peer.page("/g/cheapmo/files?under=docs").await;
        assert!(page.contains("alice suggests a new version"), "{page}");
        assert!(page.contains("Files (1)"), "{page}");
    }
    let stale = [
        ("back", "/g/cheapmo/files"),
        ("group", "cheapmo"),
        ("suggestions", ids[0].as_str()),
    ];
    let answer = post_form(&peer, "suggestion/discard", &stale).await;
    assert_ne!(
        answer.status, 303,
        "a suggestion changed since it was shown"
    );
    assert_eq!(read(&plan), "second idea\n");
    let shown = [
        ("back", "/g/cheapmo/files"),
        ("group", "cheapmo"),
        ("suggestions", ids[1].as_str()),
    ];
    let answer = post_form(&peer, "suggestion/discard", &shown).await;
    assert_eq!(answer.status, 303, "{}", answer.body);
    eventually("the disk shows the group's plan again", async || {
        read(&plan) == "plan\n"
    })
    .await;
    let suggestions = peer.call("suggestion", "list", json!({})).await.unwrap();
    assert_eq!(suggestions, json!([]));
}
