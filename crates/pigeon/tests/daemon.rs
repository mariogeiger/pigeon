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
async fn two_daemons_share_files_and_answer_requests() {
    let lookup = MemoryLookup::new();
    let a = Peer::start(&lookup).await;
    let b = Peer::start(&lookup).await;
    let created = a
        .call(
            "group",
            "create",
            json!({"name": "cheapmo", "member": "alice", "password": "pa", "root": a.root("cheapmo")}),
        )
        .await
        .unwrap();
    let key = created["key"].as_str().unwrap();
    b.call(
        "group",
        "join",
        json!({"key": key, "member": "bob", "password": "pb", "root": b.root("cheapmo")}),
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
    assert_eq!(
        wrote,
        json!({"published": ["+alice/notes.txt"], "requests": []})
    );
    b.call("selection", "follow", json!({"pattern": "/+alice/"}))
        .await
        .unwrap();
    let on_b = b.root("cheapmo").join("+alice/notes.txt");
    eventually("bob holds the note", async || {
        std::fs::read_to_string(&on_b).ok().as_deref() == Some("hello\n")
    })
    .await;

    let asked = b
        .call(
            "file",
            "write",
            json!({"path": "+alice/notes.txt", "content": base64("bonjour\n"), "message": "in French"}),
        )
        .await
        .unwrap();
    assert_eq!(asked["published"], json!([]));
    let request = asked["requests"][0].as_str().unwrap().to_owned();
    eventually("alice sees the request", async || {
        let requests = a.call("request", "list", json!({})).await.unwrap();
        requests.as_array().unwrap().len() == 1
    })
    .await;
    eventually("alice reviews the difference", async || {
        let page = a.page("/g/cheapmo/requests").await;
        page.contains("- hello") && page.contains("+ bonjour")
    })
    .await;
    let page = a.page("/g/cheapmo/requests").await;
    assert!(page.contains(r#"action="/act/request/accept""#));
    a.call("request", "accept", json!({"request": request}))
        .await
        .unwrap();
    let on_a = a.root("cheapmo").join("+alice/notes.txt");
    eventually("both hold the accepted note", async || {
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
async fn joining_with_a_wrong_password_names_the_command_that_logs_in() {
    let lookup = MemoryLookup::new();
    let a = Peer::start(&lookup).await;
    let b = Peer::start(&lookup).await;
    let created = a
        .call(
            "group",
            "create",
            json!({"name": "cheapmo", "member": "alice", "password": "pa", "root": a.root("cheapmo")}),
        )
        .await
        .unwrap();
    assert!(a.joined().await);
    let key = created["key"].as_str().unwrap();
    let error = b
        .call(
            "group",
            "join",
            json!({"key": key, "member": "alice", "password": "wrong", "root": b.root("cheapmo")}),
        )
        .await
        .unwrap_err();
    assert!(
        error.contains("the name alice is taken by another password"),
        "{error}"
    );
    assert!(
        error.contains("`pigeon member claim --group cheapmo --member alice`"),
        "{error}"
    );
    b.call(
        "member",
        "claim",
        json!({"member": "alice", "password": "pa"}),
    )
    .await
    .unwrap();
    assert!(b.joined().await);
}

#[tokio::test]
async fn web_forms_run_their_action_and_return() {
    let lookup = MemoryLookup::new();
    let peer = Peer::start(&lookup).await;
    peer.call(
        "group",
        "create",
        json!({"name": "cheapmo", "member": "alice", "password": "pa", "root": peer.root("cheapmo")}),
    )
    .await
    .unwrap();
    let boundary = "pigeonboundary";
    let fields = [
        ("back", "/g/cheapmo/selection"),
        ("group", "cheapmo"),
        ("pattern", "/docs/"),
    ];
    let parts = fields.map(|(name, value)| {
        format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
    });
    let form = format!("{}--{boundary}--\r\n", parts.concat());
    let kind = format!("multipart/form-data; boundary={boundary}");
    let answer = peer
        .send(
            "POST",
            "/act/selection/follow",
            vec![peer.cookie()],
            Some((kind, form.into_bytes())),
        )
        .await;
    assert_eq!(answer.status, 303, "{}", answer.body);
    assert_eq!(answer.location.as_deref(), Some("/g/cheapmo/selection"));
    let rules = peer.call("selection", "list", json!({})).await.unwrap();
    assert_eq!(rules, json!("follow +alice/\nfollow /docs/\n"));
    let page = peer.page("/g/cheapmo/selection").await;
    assert!(page.contains("/docs/"));
    let set = peer
        .call("retention", "set", json!({"quota": 5, "everything": "on"}))
        .await
        .unwrap();
    assert_eq!(
        set,
        json!({"every": 1, "daily": 30, "weekly": 365, "deletion": 365, "quota": 5, "everything": "on"})
    );
    let page = peer.page("/g/cheapmo/retention").await;
    assert!(
        page.contains(r#"<input type="number" min="0" name="quota" value="5">"#),
        "{page}"
    );
    assert!(page.contains(r#"<option value="on" selected>"#));
    let error = peer
        .call("retention", "set", json!({"quota": 101}))
        .await
        .unwrap_err();
    assert!(error.contains("percentage"), "{error}");
}

fn dummy(kind: Kind, peer: &Peer) -> Value {
    match kind {
        Kind::Text | Kind::Secret => json!("x"),
        Kind::Path => json!("+alice/dummy.txt"),
        Kind::Pattern => json!("/dummy/"),
        Kind::Folder => json!(peer.root("dummy")),
        Kind::Bytes => json!(base64("dummy")),
        Kind::Rules => json!("follow /dummy/"),
        Kind::Number => json!(0),
        Kind::Time => json!("2026-01-01T00:00:00Z"),
        Kind::Choice(choices) => json!(choices[0]),
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
        json!({"name": "cheapmo", "member": "alice", "password": "pa", "root": peer.root("cheapmo")}),
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
async fn a_new_password_keeps_this_machine_in_and_leaving_takes_it_out() {
    let lookup = MemoryLookup::new();
    let peer = Peer::start(&lookup).await;
    peer.call(
        "group",
        "create",
        json!({"name": "cheapmo", "member": "alice", "password": "pa", "root": peer.root("cheapmo")}),
    )
    .await
    .unwrap();
    eventually("alice joined", async || peer.joined().await).await;
    let page = peer.page("/g/cheapmo/members").await;
    for verb in [
        "member/password",
        "member/reset",
        "member/exclude",
        "group/leave",
    ] {
        assert!(page.contains(&format!(r#"action="/act/{verb}""#)), "{verb}");
    }
    peer.call("member", "password", json!({"password": "new"}))
        .await
        .unwrap();
    assert!(peer.joined().await);
    let members = peer.call("member", "list", json!({})).await.unwrap();
    assert_eq!(members[0]["rebound"]["by"], "alice");
    peer.call("group", "leave", json!({})).await.unwrap();
    let status = peer.call("group", "status", json!({})).await.unwrap();
    assert_eq!(
        status["join"],
        json!({"state": "excluded", "reason": "alice left the group"})
    );
    let page = peer.page("/g/cheapmo").await;
    assert!(page.contains("alice left the group"));
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
        json!({"name": "cheapmo", "member": "alice", "password": "pa", "root": peer.root("cheapmo")}),
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
    assert!(page.contains("frozen copy"), "{page}");
    let page = peer.page("/g/cheapmo/files").await;
    assert!(page.contains(r#"data-pattern="/+alice/" data-state="unchecked">"#));
    peer.call("selection", "follow", json!({"pattern": "/+alice/"}))
        .await
        .unwrap();
    let rules = peer.call("selection", "list", json!({})).await.unwrap();
    assert_eq!(rules, json!("follow +alice/\nfollow /+alice/\n"));
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
        json!({"name": "cheapmo", "member": "alice", "password": "pa", "root": peer.root("cheapmo")}),
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

/// What the selection editor of the group cheapmo shows of `draft`.
async fn editor_preview(peer: &Peer, draft: &str) -> Value {
    let encoded = draft
        .replace('+', "%2B")
        .replace('/', "%2F")
        .replace('\n', "%0A")
        .replace(' ', "+");
    let answer = peer
        .send(
            "POST",
            "/g/cheapmo/selection/preview",
            vec![peer.cookie()],
            Some((
                "application/x-www-form-urlencoded".to_owned(),
                format!("rules={encoded}").into_bytes(),
            )),
        )
        .await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    serde_json::from_str(&answer.body).unwrap()
}

#[tokio::test]
async fn the_selection_editor_previews_a_draft_and_saves_it_whole() {
    let lookup = MemoryLookup::new();
    let peer = Peer::start(&lookup).await;
    peer.call(
        "group",
        "create",
        json!({"name": "cheapmo", "member": "alice", "password": "pa", "root": peer.root("cheapmo")}),
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
    let notes = std::path::Path::new(&peer.root("cheapmo")).join("+alice/notes.txt");
    eventually("the notes are on disk", async || notes.exists()).await;
    let page = peer.page("/g/cheapmo/selection").await;
    for part in [
        r#"<section id="editor" class="editor" data-keep"#,
        r#"<script src="/selection.js" defer></script>"#,
        r#"<option value="/+alice/notes.txt">"#,
        r#"<option value="/+alice/">"#,
        "<h2>Places</h2>",
    ] {
        assert!(page.contains(part), "{part}: {page}");
    }
    let script = peer
        .send("GET", "/selection.js", vec![peer.cookie()], None)
        .await;
    assert_eq!(script.status, 200);
    assert!(script.body.contains("/selection/preview"));

    let parts = editor_preview(&peer, "free +alice/\nkeep x\n").await;
    assert_eq!(
        parts["rows"][0],
        json!({"effect": "matches 1 file · decides 1 (6 B)", "masked": false})
    );
    assert!(
        parts["rows"][1]["error"]
            .as_str()
            .unwrap()
            .contains("not a mode")
    );
    assert_eq!(parts["save"], "Save: -6 B");
    assert!(parts["confirm"].is_string());
    let panel = parts["panel"].as_str().unwrap();
    assert!(
        panel.contains("warning: <code>+alice/</code> will no longer be here"),
        "{panel}"
    );
    assert!(panel.contains("free: -1 file, -6 B"), "{panel}");

    let preview = peer
        .call("selection", "preview", json!({"rules": "free +alice/\n"}))
        .await
        .unwrap();
    assert_eq!(preview["version"], parts["version"]);
    assert_eq!(preview["after"], json!({"files": 0, "bytes": 0}));
    let error = peer
        .call(
            "selection",
            "set",
            json!({"rules": "free +alice/\n", "version": "stale"}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("changed since"), "{error}");
    let error = peer
        .call("selection", "set", json!({"rules": "keep x\n"}))
        .await
        .unwrap_err();
    assert!(error.contains("line 1: "), "{error}");
    peer.call(
        "selection",
        "set",
        json!({"rules": "free +alice/\n", "version": preview["version"]}),
    )
    .await
    .unwrap();
    let rules = peer.call("selection", "list", json!({})).await.unwrap();
    assert_eq!(rules, json!("free +alice/\n"));
    assert!(!notes.exists());
    peer.call("selection", "set", json!({"rules": ""}))
        .await
        .unwrap();
    assert_eq!(
        peer.call("selection", "list", json!({})).await.unwrap(),
        json!("")
    );
}
