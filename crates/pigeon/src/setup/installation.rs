//! What `pigeon setup` finds of pigeon's installation on this machine:
//! the Rust toolchain, whether the shell finds the running program, and
//! whether a browser can open the web UI here.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The version of cargo, if the shell finds it.
#[must_use]
pub fn cargo_version() -> Option<String> {
    let output = Command::new("cargo").arg("--version").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The first `pigeon` the shell finds on its `PATH`.
#[must_use]
pub fn on_path() -> Option<PathBuf> {
    let name = format!("pigeon{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|folder| folder.join(&name))
        .find(|program| program.is_file())
}

/// Whether `a` and `b` are the same file.
#[must_use]
pub fn same_file(a: &Path, b: &Path) -> bool {
    a.canonicalize()
        .is_ok_and(|a| b.canonicalize().ok() == Some(a))
}

/// Opens `link` in a browser when this terminal belongs to a graphical
/// session; returns whether it tried.
#[must_use]
pub fn open_in_browser(link: &str) -> bool {
    let remote = std::env::var_os("SSH_CONNECTION").is_some();
    let graphical = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|variable| std::env::var_os(variable).is_some());
    let opener = match std::env::consts::OS {
        "macos" if !remote => "open",
        "linux" if graphical => "xdg-open",
        _ => return false,
    };
    Command::new(opener)
        .arg(link)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}
