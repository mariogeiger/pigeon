//! A group of machines on this host for scenario tests: each machine is a
//! daemon with its own pigeon folder and API server, driven through the
//! command line's client, reachable by the others only while it is online
//! and switched on, and edited on disk the ways people and their programs
//! edit files; and the family most scenarios play.

#![allow(dead_code)]

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use data_encoding::BASE64;
use iroh::address_lookup::MemoryLookup;
use pigeon::api::{App, serve};
use pigeon::client::call_at;
use pigeon::daemon::{Daemon, Stop};
use pigeon::home::Home;
use pigeon_sync::{Network, Options};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The group every scenario shares.
pub const GROUP: &str = "family";

/// The engine timings of every machine: quick, so that scenarios run in
/// seconds, with a full rescan only at start.
fn options(lookup: &MemoryLookup) -> Options {
    Options {
        network: Network::Local(lookup.clone()),
        settle_personal: Duration::from_millis(100),
        settle_drop: Duration::from_millis(400),
        rescan: Duration::from_secs(600),
        tick: Duration::from_millis(50),
        join_delay: Duration::from_millis(300),
        gc: Duration::from_millis(300),
        ..Options::default()
    }
}

/// A daemon and the API server in front of it.
struct Running {
    address: SocketAddr,
    token: String,
    app: Arc<App>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Running {
    async fn start(home: Home, lookup: &MemoryLookup) -> Self {
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
            app,
            server,
        }
    }

    async fn stop(self) {
        self.app.daemon.stop(Stop::Quit);
        self.server.await.unwrap().unwrap();
        let app = Arc::into_inner(self.app).expect("nothing else holds the daemon");
        app.daemon.shutdown().await;
    }
}

/// One machine: its pigeon folder and the root of the group in it, the
/// network it joins when online, and its daemon while switched on.
pub struct Machine {
    pub name: &'static str,
    dir: TempDir,
    internet: MemoryLookup,
    running: Option<Running>,
}

impl Machine {
    /// Switches on a new machine, online.
    pub async fn start(name: &'static str, internet: &MemoryLookup) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let running = Running::start(Home::new(dir.path().join("home")), internet).await;
        Self {
            name,
            dir,
            internet: internet.clone(),
            running: Some(running),
        }
    }

    fn running(&self) -> &Running {
        self.running
            .as_ref()
            .unwrap_or_else(|| panic!("{} is switched off", self.name))
    }

    /// Restarts the daemon, now reachable only through `lookup`.
    async fn restart_on(&mut self, lookup: &MemoryLookup) {
        if let Some(running) = self.running.take() {
            running.stop().await;
        }
        let home = Home::new(self.dir.path().join("home"));
        self.running = Some(Running::start(home, lookup).await);
    }

    /// Loses the network: the daemon goes on, alone.
    pub async fn go_offline(&mut self) {
        self.restart_on(&MemoryLookup::new()).await;
    }

    /// Finds the network again.
    pub async fn go_online(&mut self) {
        let internet = self.internet.clone();
        self.restart_on(&internet).await;
    }

    /// Stops the daemon, as switching the machine off does.
    pub async fn switch_off(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop().await;
        }
    }

    /// Starts the daemon again, online.
    pub async fn switch_on(&mut self) {
        self.go_online().await;
    }

    /// Runs `pigeon <noun> <verb>` with `args` in the group.
    pub async fn call(&self, noun: &str, verb: &str, args: Value) -> Result<Value, String> {
        let Value::Object(args) = args else {
            panic!("arguments are an object")
        };
        let running = self.running();
        let (address, token) = (running.address, running.token.clone());
        let (noun, verb) = (noun.to_owned(), verb.to_owned());
        tokio::task::spawn_blocking(move || call_at(address, &token, &noun, &verb, &args))
            .await
            .unwrap()
            .map_err(|error| error.to_string())
    }

    /// Like `call`, failing the scenario if pigeon refuses.
    pub async fn run(&self, noun: &str, verb: &str, args: Value) -> Value {
        match self.call(noun, verb, args.clone()).await {
            Ok(value) => value,
            Err(error) => panic!("{}: pigeon {noun} {verb} {args}: {error}", self.name),
        }
    }

    /// Creates the group as `member` and returns its key.
    pub async fn create(&self, member: &str) -> String {
        let created = self
            .run(
                "group",
                "create",
                json!({"name": GROUP, "member": member, "root": self.root()}),
            )
            .await;
        created["key"].as_str().unwrap().to_owned()
    }

    /// Joins the group as `member`.
    pub async fn join(&self, key: &str, member: &str) {
        self.run(
            "group",
            "join",
            json!({"key": key, "member": member, "root": self.root()}),
        )
        .await;
        eventually(
            &format!("{} joins as {member}", self.name),
            &[],
            async || self.run("group", "status", json!({})).await["join"]["state"] == "joined",
        )
        .await;
    }

    /// The group's root on this machine.
    pub fn root(&self) -> PathBuf {
        self.dir.path().join(GROUP)
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root().join(relative)
    }

    pub fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.path(relative)).ok()
    }

    /// Whether the disk shows `text` at `relative`.
    pub fn shows(&self, relative: &str, text: &str) -> bool {
        self.read(relative).as_deref() == Some(text)
    }

    /// Writes `text` into the file in place, as most programs save,
    /// creating its folders; panics if the system refuses.
    pub fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text)
            .unwrap_or_else(|error| panic!("{}: writing {relative}: {error}", self.name));
    }

    /// Saves `text` as editors that keep a backup do: into a temporary file
    /// beside it, renamed over the file.
    pub fn save_atomically(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        let temporary = path.with_file_name(".goutputstream-PIGEON");
        std::fs::write(&temporary, text).unwrap();
        std::fs::rename(&temporary, &path).unwrap();
    }

    pub fn remove(&self, relative: &str) {
        std::fs::remove_file(self.path(relative)).unwrap();
    }

    /// Moves a file or folder within the root, as a file manager does.
    pub fn rename(&self, from: &str, to: &str) {
        let to = self.path(to);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::rename(self.path(from), to).unwrap();
    }

    /// Dates the file's last modification `ago` back.
    pub fn date_back(&self, relative: &str, ago: Duration) {
        let file = std::fs::File::options()
            .read(true)
            .open(self.path(relative))
            .unwrap();
        file.set_modified(SystemTime::now() - ago).unwrap();
    }

    /// The suggestions `pigeon suggestion list` shows, oldest first.
    pub async fn suggestions(&self) -> Vec<Value> {
        let list = self.run("suggestion", "list", json!({})).await;
        list.as_array().unwrap().clone()
    }

    /// The suggestions that change `path`.
    pub async fn suggested(&self, path: &str) -> Vec<Value> {
        let suggestions = self.suggestions().await;
        suggestions
            .into_iter()
            .filter(|suggestion| {
                suggestion["changes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|change| change["path"] == path)
            })
            .collect()
    }

    /// The one suggestion that changes `path`, waiting until there is one.
    pub async fn suggestion_at(&self, path: &str) -> Value {
        eventually(
            &format!("{} suggests {path}", self.name),
            &[self],
            async || self.suggested(path).await.len() == 1,
        )
        .await;
        self.suggested(path).await.remove(0)
    }

    /// Validates the suggestions `ids`, at `to` when given.
    pub async fn validate(&self, ids: &[&Value], to: Option<&str>) {
        let ids: Vec<&str> = ids.iter().map(|id| id.as_str().unwrap()).collect();
        self.run(
            "suggestion",
            "validate",
            json!({"suggestions": ids.join(" "), "to": to}),
        )
        .await;
    }

    /// Discards the suggestions `ids`.
    pub async fn discard(&self, ids: &[&Value]) {
        let ids: Vec<&str> = ids.iter().map(|id| id.as_str().unwrap()).collect();
        self.run(
            "suggestion",
            "discard",
            json!({"suggestions": ids.join(" ")}),
        )
        .await;
    }

    /// What this machine holds, for a failing scenario to show: the files
    /// it lists with their authors, those on its disk, the suggestions it
    /// lists, and its latest errors.
    pub async fn describe(&self) -> String {
        let mut lines = vec![format!("{}:", self.name)];
        let files = self.run("file", "list", json!({})).await;
        for file in files.as_array().unwrap() {
            let path = file["path"].as_str().unwrap();
            if !path.starts_with(".pigeon/") {
                lines.push(format!(
                    "  listed {path} by {} held {} cutoff {}",
                    file["author"], file["held"], file["cutoff"]
                ));
            }
        }
        let mut folders = vec![self.root()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                let path = entry.path();
                let relative = path
                    .strip_prefix(self.root())
                    .unwrap()
                    .display()
                    .to_string();
                if relative.starts_with(".pigeon") {
                    continue;
                }
                if path.is_dir() {
                    folders.push(path);
                } else {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    lines.push(format!("  disk {relative}: {text:?}"));
                }
            }
        }
        for suggestion in self.suggestions().await {
            lines.push(format!(
                "  suggestion by {} as {}: {}",
                suggestion["author"], suggestion["reason"], suggestion["changes"]
            ));
        }
        let status = self.run("group", "status", json!({})).await;
        for error in status["errors"].as_array().into_iter().flatten() {
            lines.push(format!("  error {error}"));
        }
        lines.join("\n")
    }

    /// The contents of a file's versions as `content` gives them, oldest
    /// first, deletions as none.
    pub async fn history(&self, path: &str) -> Vec<Option<Value>> {
        let history = self.run("file", "history", json!({"path": path})).await;
        history
            .as_array()
            .unwrap()
            .iter()
            .map(|version| Some(version["content"].clone()).filter(|content| !content.is_null()))
            .collect()
    }
}

/// A family whose alice created the group on her machine and whose papy
/// joined on his desktop and his laptop.
pub async fn family(internet: &MemoryLookup) -> (Machine, Machine, Machine) {
    let alice = Machine::start("alice's machine", internet).await;
    let key = alice.create("alice").await;
    let desktop = Machine::start("papy's desktop", internet).await;
    desktop.join(&key, "papy").await;
    let laptop = Machine::start("papy's laptop", internet).await;
    laptop.join(&key, "papy").await;
    (alice, desktop, laptop)
}

/// `text` as pigeon describes a file's content, to compare with versions.
pub fn content(text: &str) -> Value {
    json!({
        "hash": blake3::hash(text.as_bytes()).as_bytes(),
        "size": text.len(),
        "executable": false,
    })
}

/// Waits up to thirty seconds for `condition`, then fails showing what
/// `machines` hold.
pub async fn eventually<F, Fut>(what: &str, machines: &[&Machine], mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !condition().await {
        if tokio::time::Instant::now() >= deadline {
            let mut shown = Vec::new();
            for machine in machines {
                shown.push(machine.describe().await);
            }
            panic!("timed out: {what}\n{}", shown.join("\n"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Waits until `machine` knows `versions` versions of `path`, as once it
/// published its own edit of it.
pub async fn published(machine: &Machine, path: &str, versions: usize) {
    let what = format!("{} publishes {path}", machine.name);
    eventually(&what, &[machine], async || {
        machine.history(path).await.len() == versions
    })
    .await;
}

/// Waits long enough for the engines to settle and publish what they saw.
pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(1500)).await;
}

pub fn base64(text: &str) -> String {
    BASE64.encode(text.as_bytes())
}
