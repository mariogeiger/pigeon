//! Updating pigeon: git brings a clone kept between updates to the newest
//! release, the highest `vMAJOR.MINOR.PATCH` tag of the repository,
//! rewriting only the files that moved, and cargo builds it, or a local
//! checkout, in the caller's terminal, into a build folder kept between
//! updates, so that sources that did not move compile nothing; the
//! program it builds replaces the file the daemon runs, which keeps the
//! old one as the previous program; then the daemon is asked to restart
//! onto the new program. Going back to the previous program is the same
//! update without the build.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value};

use crate::client;
use crate::home::Home;
use crate::previous;
use crate::release::{self, Release};

/// The repository whose releases an update builds.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// The folder `name` of pigeon's cache, kept between updates.
fn cache_folder(name: &str) -> Result<PathBuf> {
    dirs::cache_dir()
        .map(|cache| cache.join("pigeon").join(name))
        .ok_or_else(|| anyhow!("this system names no cache folder to build pigeon in"))
}

/// The git options that make a transfer slower than 1000 bytes a second
/// for 30 seconds fail, so that a stalled connection ends the update
/// rather than hangs it.
const STALL: [&str; 4] = [
    "-c",
    "http.lowSpeedLimit=1000",
    "-c",
    "http.lowSpeedTime=30",
];

/// A git command that gives up on a stalled connection.
fn git() -> Command {
    let mut command = Command::new("git");
    command.args(STALL);
    command
}

/// Runs `command`, failing with `what` unless it succeeds.
fn run_git(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .status()
        .context("running git: install it from https://git-scm.com")?;
    if !status.success() {
        bail!(
            "git could not {what}: check the connection to {REPOSITORY} and run pigeon update again; the daemon keeps running the program it has"
        );
    }
    Ok(())
}

/// Brings the clone at `source` to the commit `release` tags, cloning it
/// first, showing git's progress.
fn fetch_release(source: &Path, release: &Release) -> Result<()> {
    eprintln!("pigeon: fetching {} from {REPOSITORY}", release.tag);
    if !source.join(".git").exists() {
        return run_git(
            git()
                .args(["-c", "advice.detachedHead=false", "clone", "--branch"])
                .arg(&release.tag)
                .arg(REPOSITORY)
                .arg(source),
            "clone pigeon",
        );
    }
    run_git(
        git().arg("-C").arg(source).args([
            "fetch",
            REPOSITORY,
            &format!("refs/tags/{0}:refs/tags/{0}", release.tag),
        ]),
        "fetch pigeon",
    )?;
    run_git(
        git().arg("-C").arg(source).args([
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "--quiet",
            "--force",
            "--detach",
            &release.tag,
        ]),
        "check out the release",
    )
}

/// The cargo command that builds pigeon from `checkout`, a clone of its
/// repository, compiling into `build` and installing into the folder
/// `stage` holds in `bin`.
fn install_command(checkout: &Path, build: &Path, stage: &Path) -> Command {
    let mut command = Command::new("cargo");
    command
        .args(["install", "--locked", "--target-dir"])
        .arg(build)
        .arg("--root")
        .arg(stage)
        .arg("--path")
        .arg(checkout.join("crates").join("pigeon"));
    command
}

/// Builds pigeon from `checkout` into a fresh folder of the cache and
/// returns the program.
fn build(checkout: &Path) -> Result<PathBuf> {
    let stage = cache_folder("stage")?;
    let _ = std::fs::remove_dir_all(&stage);
    let built = install_command(checkout, &cache_folder("build")?, &stage)
        .status()
        .context("running cargo: install Rust from https://rustup.rs")?;
    if !built.success() {
        bail!("cargo could not build pigeon; the daemon keeps running the program it has");
    }
    Ok(stage
        .join("bin")
        .join(format!("pigeon{}", std::env::consts::EXE_SUFFIX)))
}

/// What an update puts in the place of the program.
pub enum Update<'a> {
    /// The newest release.
    Newest,
    /// A build of the checkout in this folder.
    Checkout(&'a Path),
    /// The previous program.
    Previous,
}

/// The program file the daemon of `home` runs, or this one if it does not
/// answer: the file an update replaces.
fn program_to_replace(home: &Home) -> Result<PathBuf> {
    match client::call(home, "daemon", "program", &Map::new()) {
        Ok(answer) => answer["path"]
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("the daemon did not name its program")),
        Err(_) => std::env::current_exe().context("finding the running program"),
    }
}

/// Puts the program `update` names in the place of the one the daemon of
/// `home` runs, then asks the daemon to restart onto it.
///
/// # Errors
///
/// Fails if git cannot bring the release, the repository has none, cargo
/// cannot run or build pigeon, there is no previous program, the program
/// cannot be replaced, or the daemon does not answer.
pub fn run(home: &Home, update: &Update) -> Result<()> {
    let program = program_to_replace(home)?;
    match update {
        Update::Previous => previous::restore(&program)?,
        Update::Checkout(checkout) => previous::replace(&program, &build(checkout)?)?,
        Update::Newest => {
            let release = release::newest_of(REPOSITORY, &mut git())?;
            let source = cache_folder("source")?;
            fetch_release(&source, &release)?;
            previous::replace(&program, &build(&source)?)?;
        }
    }
    eprintln!("pigeon: installed {}", program.display());
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
    fn git_gives_up_on_a_stalled_connection() {
        assert_eq!(arguments(&git()), STALL);
    }

    #[test]
    fn cargo_builds_the_checkout_into_the_kept_folder_and_installs_into_a_stage() {
        let build = Path::new("/cache/pigeon/build");
        assert_eq!(
            arguments(&install_command(
                Path::new("/src/pigeon"),
                build,
                Path::new("/cache/pigeon/stage")
            )),
            [
                "install",
                "--locked",
                "--target-dir",
                "/cache/pigeon/build",
                "--root",
                "/cache/pigeon/stage",
                "--path",
                "/src/pigeon/crates/pigeon"
            ]
        );
    }
}
