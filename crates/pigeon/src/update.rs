//! Updating pigeon: git brings a clone kept between updates to the head of
//! the repository's main branch, rewriting only the files that moved, and
//! cargo builds it, or a local checkout, in the caller's terminal, into a
//! build folder kept between updates, so that sources that did not move
//! compile nothing; then the daemon is asked to restart onto the new
//! program.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value};

use crate::client;
use crate::home::Home;

/// The repository whose main branch an update builds.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// The folder `name` of pigeon's cache, kept between updates.
fn cache_folder(name: &str) -> Result<PathBuf> {
    dirs::cache_dir()
        .map(|cache| cache.join("pigeon").join(name))
        .ok_or_else(|| anyhow!("this system names no cache folder to build pigeon in"))
}

/// Runs `command`, failing with `what` unless it succeeds.
fn run_git(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .status()
        .context("running git: install it from https://git-scm.com")?;
    if !status.success() {
        bail!("git could not {what}; the daemon keeps running the program it has");
    }
    Ok(())
}

/// Brings the clone at `source` to the head of main, cloning it first.
fn fetch_main(source: &Path) -> Result<()> {
    if !source.join(".git").exists() {
        let mut clone = Command::new("git");
        clone
            .args(["clone", "--quiet", "--branch", "main", REPOSITORY])
            .arg(source);
        return run_git(&mut clone, "clone pigeon");
    }
    let git = || {
        let mut command = Command::new("git");
        command.arg("-C").arg(source);
        command
    };
    run_git(
        git().args(["fetch", "--quiet", REPOSITORY, "main"]),
        "fetch pigeon",
    )?;
    run_git(
        git().args(["checkout", "--quiet", "--force", "--detach", "FETCH_HEAD"]),
        "check out the head of main",
    )
}

/// The cargo command that installs pigeon from `checkout`, a clone of its
/// repository, compiling into `build`.
#[must_use]
pub fn install_command(checkout: &Path, build: &Path) -> Command {
    let mut command = Command::new("cargo");
    command
        .args(["install", "--locked", "--target-dir"])
        .arg(build)
        .arg("--path")
        .arg(checkout.join("crates").join("pigeon"));
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
/// Fails if git cannot bring the head of main, cargo cannot run or build
/// pigeon, or the daemon does not answer.
pub fn run(home: &Home, checkout: Option<&Path>) -> Result<()> {
    let build = cache_folder("build")?;
    let source = if let Some(checkout) = checkout {
        checkout.to_owned()
    } else {
        let source = cache_folder("source")?;
        fetch_main(&source)?;
        source
    };
    let program = std::env::current_exe().context("finding the running program")?;
    let aside = move_aside(&program)?;
    let installed = install_command(&source, &build).status();
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
    fn cargo_builds_the_checkout_into_the_kept_folder() {
        let build = Path::new("/cache/pigeon/build");
        assert_eq!(
            arguments(&install_command(Path::new("/src/pigeon"), build)),
            [
                "install",
                "--locked",
                "--target-dir",
                "/cache/pigeon/build",
                "--path",
                "/src/pigeon/crates/pigeon"
            ]
        );
    }
}
