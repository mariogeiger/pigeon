//! Updating pigeon: cargo builds the head of the repository's main branch,
//! or a local checkout, in the caller's terminal, into a build folder kept
//! between updates, so that sources that did not move compile nothing;
//! then the daemon is asked to restart onto the new program.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value};

use crate::client;
use crate::home::Home;

/// The repository whose main branch an update builds.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// Where cargo keeps what it compiled between updates.
fn build_folder() -> Result<PathBuf> {
    dirs::cache_dir()
        .map(|cache| cache.join("pigeon").join("build"))
        .ok_or_else(|| anyhow!("this system names no cache folder to build pigeon in"))
}

/// The cargo command that installs pigeon from `checkout`, a clone of its
/// repository, or else from the head of main, compiling into `build`.
#[must_use]
pub fn install_command(checkout: Option<&Path>, build: &Path) -> Command {
    let mut command = Command::new("cargo");
    command
        .args(["install", "--locked", "--target-dir"])
        .arg(build);
    match checkout {
        Some(checkout) => command
            .arg("--path")
            .arg(checkout.join("crates").join("pigeon")),
        None => command.args(["--git", REPOSITORY, "--branch", "main", "pigeon"]),
    };
    command
}

/// Moves `program` aside, where Windows, which cannot replace the file of
/// a running program, lets cargo install over it; returns where it went.
fn move_aside(program: &Path) -> Result<Option<PathBuf>> {
    if !cfg!(windows) {
        return Ok(None);
    }
    let aside = program.with_extension("old.exe");
    let _ = std::fs::remove_file(&aside);
    std::fs::rename(program, &aside)
        .with_context(|| format!("moving {} aside to install over it", program.display()))?;
    Ok(Some(aside))
}

/// Puts `program` back from `aside` unless cargo installed another.
fn put_back(program: &Path, aside: Option<PathBuf>) -> Result<()> {
    match aside {
        Some(aside) if !program.exists() => std::fs::rename(&aside, program)
            .with_context(|| format!("putting {} back", program.display())),
        _ => Ok(()),
    }
}

/// Installs pigeon from `checkout`, or from the head of main, then asks
/// the daemon of `home` to restart onto it.
///
/// # Errors
///
/// Fails if cargo cannot run or build pigeon, or the daemon does not
/// answer.
pub fn run(home: &Home, checkout: Option<&Path>) -> Result<()> {
    let build = build_folder()?;
    let program = std::env::current_exe().context("finding the running program")?;
    let aside = move_aside(&program)?;
    let installed = install_command(checkout, &build).status();
    put_back(&program, aside)?;
    let installed = installed.context("running cargo: install Rust from https://rustup.rs")?;
    if !installed.success() {
        bail!("cargo could not install pigeon; the daemon keeps running the program it has");
    }
    let answer = client::call(home, "daemon", "restart", &Map::new())
        .context("pigeon is installed, but the daemon did not restart onto it")?;
    if answer["restarts"] == Value::Bool(true) {
        eprintln!("pigeon: the daemon restarts onto the new program");
    } else {
        eprintln!("pigeon: the daemon already runs this program");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn cargo_builds_main_or_the_checkout_into_the_kept_folder() {
        let build = Path::new("/cache/pigeon/build");
        assert_eq!(
            arguments(&install_command(None, build)),
            [
                "install",
                "--locked",
                "--target-dir",
                "/cache/pigeon/build",
                "--git",
                "https://github.com/mariogeiger/pigeon",
                "--branch",
                "main",
                "pigeon"
            ]
        );
        let local = install_command(Some(Path::new("/src/pigeon")), build);
        assert_eq!(
            arguments(&local)[4..],
            ["--path", "/src/pigeon/crates/pigeon"]
        );
    }
}
