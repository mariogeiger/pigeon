//! The `pigeon` program run as a shell runs it, on its own folder: the
//! command line calling the daemon it started, the daemon stopping on the
//! signal a service manager sends, and an update putting another program
//! in the daemon's place and the daemon restarting onto it.
#![cfg(unix)]

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
        std::fs::copy(env!("CARGO_BIN_EXE_pigeon"), dir.path().join("pigeon")).unwrap();
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
    std::fs::write(
        &previous,
        format!(
            "#!/bin/sh\ntouch '{}'\nexec '{}' \"$@\"\n",
            installed.marker().display(),
            env!("CARGO_BIN_EXE_pigeon")
        ),
    )
    .unwrap();
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
