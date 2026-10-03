//! The `pigeon` program run as a shell runs it, on its own folder: the
//! command line calling the daemon it started, the daemon stopping on the
//! signal a service manager sends, an update putting another program in
//! the daemon's place and the daemon restarting onto it, and a file far
//! larger than the daemon's memory budget going in through the command
//! line and a web form and coming out of a download.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// A copy of the program with a folder of its own, and the daemon it runs.
struct Installed {
    dir: TempDir,
    daemon: Option<Child>,
}

impl Installed {
    fn program(&self) -> PathBuf {
        self.dir.path().join("pigeon")
    }

    fn marker(&self) -> PathBuf {
        self.dir.path().join("restarted")
    }

    /// The program run with `args` on the folder of this copy.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(self.program());
        command
            .args(args)
            .env("PIGEON_HOME", self.dir.path().join("home"))
            .stdin(Stdio::null());
        command
    }

    /// Starts the daemon on any free port and waits until it answers.
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        copy_outside_this_process(
            Path::new(env!("CARGO_BIN_EXE_pigeon")),
            &dir.path().join("pigeon"),
        );
        let mut installed = Self { dir, daemon: None };
        let daemon = installed
            .command(&["daemon", "--port", "0"])
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        installed.daemon = Some(daemon);
        installed.wait_until_answering();
        installed
    }

    fn answers(&self) -> bool {
        self.command(&["group", "list", "--json"])
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn wait_until_answering(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.answers() {
            assert!(Instant::now() < deadline, "the daemon never answered");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        if let Some(daemon) = &mut self.daemon {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
    }
}

/// Copies the file `from` to `to` with `cp`, so that this process never
/// holds `to` open for writing: a child another test spawns meanwhile
/// would inherit that descriptor until it runs its own program, and running
/// `to` would fail as busy.
fn copy_outside_this_process(from: &Path, to: &Path) {
    let copied = Command::new("cp").arg(from).arg(to).status().unwrap();
    assert!(copied.success(), "cp {} {}", from.display(), to.display());
}

fn make_executable(path: &Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn the_command_line_calls_the_daemon_it_started_and_the_daemon_stops_on_sigterm() {
    let mut installed = Installed::start();
    let listing = installed
        .command(&["group", "list", "--json"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&listing.stdout).trim(), "[]");
    let mut daemon = installed.daemon.take().unwrap();
    let signalled = Command::new("kill")
        .args(["-TERM", &daemon.id().to_string()])
        .status()
        .unwrap();
    assert!(signalled.success());
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = daemon.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the daemon ignored SIGTERM");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "the daemon stopped with {status}");
    assert!(!installed.answers());
}

#[test]
fn rolling_back_puts_the_previous_program_in_the_daemons_place_and_restarts_onto_it() {
    let installed = Installed::start();
    let previous = installed.dir.path().join("pigeon.previous");
    let script = installed.dir.path().join("previous.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ntouch '{}'\nexec '{}' \"$@\"\n",
            installed.marker().display(),
            env!("CARGO_BIN_EXE_pigeon")
        ),
    )
    .unwrap();
    copy_outside_this_process(&script, &previous);
    make_executable(&previous);
    let program_before = std::fs::read(installed.program()).unwrap();
    let rollback = installed
        .command(&["update", "--rollback"])
        .output()
        .unwrap();
    assert!(
        rollback.status.success(),
        "{}",
        String::from_utf8_lossy(&rollback.stderr)
    );
    assert_eq!(std::fs::read(&previous).unwrap(), program_before);
    assert!(
        std::fs::read_to_string(installed.program())
            .unwrap()
            .starts_with("#!/bin/sh")
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !installed.marker().exists() {
        assert!(
            Instant::now() < deadline,
            "the daemon never restarted onto the previous program"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    installed.wait_until_answering();
}

#[test]
fn rolling_back_with_no_previous_program_changes_nothing() {
    let installed = Installed::start();
    let before = std::fs::read(installed.program()).unwrap();
    let rollback = installed
        .command(&["update", "--rollback"])
        .output()
        .unwrap();
    assert!(!rollback.status.success());
    assert!(String::from_utf8_lossy(&rollback.stderr).contains("no previous program"));
    assert_eq!(std::fs::read(installed.program()).unwrap(), before);
    assert!(installed.answers());
}

/// A chunk of 1 MiB, which a big file repeats.
fn chunk() -> Vec<u8> {
    (0..1 << 20).map(|at: u32| (at % 251) as u8).collect()
}

/// Writes the file `path` of `size` bytes, a chunk at a time.
fn write_big(path: &Path, size: u64) {
    let mut file = std::fs::File::create(path).unwrap();
    let chunk = chunk();
    for _ in 0..size >> 20 {
        file.write_all(&chunk).unwrap();
    }
}

/// Whether the file at `path` holds what `write_big` wrote.
fn is_big(path: &Path, size: u64) -> bool {
    let mut file = std::fs::File::open(path).unwrap();
    let (chunk, mut read) = (chunk(), vec![0; 1 << 20]);
    (0..size >> 20).all(|_| file.read_exact(&mut read).is_ok() && read == chunk)
        && file.read(&mut read).unwrap() == 0
}

/// The most memory, in bytes, the process `pid` has ever used.
#[cfg(target_os = "linux")]
fn peak_memory(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let line = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .unwrap();
    let kilobytes: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    kilobytes << 10
}

const BIG: u64 = 64 << 20;

/// What a daemon may add to the most memory it has used while a file of
/// `BIG` bytes goes through it: far less than the file.
const BUDGET: u64 = 24 << 20;

/// A web form posting the file at `path` as the content of `group_path`,
/// its body read from the disk as it is sent.
fn post_file(installed: &Installed, group_path: &str, path: &Path) -> u16 {
    let home = pigeon::home::Home::new(installed.dir.path().join("home"));
    let boundary = "pigeonboundary";
    let field = |name: &str, value: &str| {
        format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
    };
    let head = format!(
        "{}{}--{boundary}\r\ncontent-disposition: form-data; name=\"content\"; filename=\"big.bin\"\r\ncontent-type: application/octet-stream\r\n\r\n",
        field("back", "/"),
        field("path", group_path),
    );
    let body = std::io::Cursor::new(head.into_bytes())
        .chain(std::fs::File::open(path).unwrap())
        .chain(std::io::Cursor::new(
            format!("\r\n--{boundary}--\r\n").into_bytes(),
        ));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into();
    agent
        .post(format!("http://{}/act/file/write", home.address().unwrap()))
        .header("cookie", format!("pigeon_token={}", home.token().unwrap()))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .send(ureq::SendBody::from_owned_reader(body))
        .unwrap()
        .status()
        .as_u16()
}

#[cfg(target_os = "linux")]
#[test]
fn a_file_far_larger_than_the_memory_budget_goes_in_and_out_of_the_daemon() {
    let installed = Installed::start();
    let root = installed.dir.path().join("family");
    let created = installed
        .command(&[
            "group",
            "create",
            "--name",
            "family",
            "--member",
            "alice",
            "--root",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let pid = installed.daemon.as_ref().unwrap().id();
    let big = installed.dir.path().join("big.bin");
    write_big(&big, BIG);
    let before = peak_memory(pid);

    let written = installed
        .command(&[
            "file",
            "write",
            "--path",
            "+alice/by-command.bin",
            "--content",
            big.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    let after_command = peak_memory(pid);

    assert_eq!(post_file(&installed, "+alice/by-web.bin", &big), 303);
    let after_web = peak_memory(pid);

    let deadline = Instant::now() + Duration::from_secs(60);
    for name in ["by-command.bin", "by-web.bin"] {
        let on_disk = root.join("+alice").join(name);
        while !on_disk.exists() || std::fs::metadata(&on_disk).unwrap().len() < BIG {
            assert!(Instant::now() < deadline, "{name} never reached the disk");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(is_big(&on_disk, BIG), "{name} differs from what was sent");
    }
    let home = pigeon::home::Home::new(installed.dir.path().join("home"));
    let download = ureq::get(format!(
        "http://{}/g/family/raw?path=%2Balice/by-web.bin",
        home.address().unwrap()
    ))
    .header("cookie", format!("pigeon_token={}", home.token().unwrap()))
    .call()
    .unwrap();
    let mut received = download.into_body();
    let copied = std::io::copy(&mut received.as_reader(), &mut std::io::sink()).unwrap();
    assert_eq!(copied, BIG);
    let after_download = peak_memory(pid);

    eprintln!(
        "peak memory: before {before}, command {after_command}, web {after_web}, download {after_download}"
    );
    assert!(
        after_command - before < BUDGET,
        "the command took {}",
        after_command - before
    );
    assert!(
        after_web - before < BUDGET,
        "the web form took {}",
        after_web - before
    );
    assert!(
        after_download - before < BUDGET,
        "the download took {}",
        after_download - before
    );
}
