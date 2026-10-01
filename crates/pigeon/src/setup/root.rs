//! Where `pigeon setup` puts a group's root folder: at the path every
//! machine shares, running once, if one agrees, the administrator command
//! that gives it to this user; or else at another folder, with a warning.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use dialoguer::console::Term;

use super::ask;
use crate::shared_root::{admin_command, create_root, shared_root};

/// Chooses and creates the root folder of `group`.
///
/// # Errors
///
/// Fails if the terminal cannot be used.
pub fn choose(term: &Term, group: &str) -> Result<PathBuf> {
    let shared = shared_root(group);
    if create_root(&shared).is_ok() {
        return Ok(shared);
    }
    let command = admin_command(&shared);
    term.write_line(&format!(
        "Only an administrator can create {}, the group's folder on every machine. The command:\n  {command}",
        shared.display()
    ))?;
    if ask::yes(term, "Run it with sudo?", true)? {
        run_in_terminal(&command)?;
        match create_root(&shared) {
            Ok(()) => return Ok(shared),
            Err(error) => term.write_line(&format!("✗ {error:#}"))?,
        }
    }
    term.write_line(&format!(
        "⚠ Anywhere but {}, the paths of the group's files differ from the other machines'.",
        shared.display()
    ))?;
    let home = dirs::home_dir().unwrap_or_default().join(group);
    loop {
        let other = ask::text(term, "Root folder", &home.display().to_string(), |text| {
            Path::new(text)
                .is_relative()
                .then(|| "an absolute path".to_owned())
        })?;
        let other = PathBuf::from(other);
        match create_root(&other) {
            Ok(()) => return Ok(other),
            Err(error) => term.write_line(&format!("✗ {error:#}"))?,
        }
    }
}

/// Runs `command` with this terminal, so that sudo can ask its password.
fn run_in_terminal(command: &str) -> Result<()> {
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    Command::new(shell)
        .args([flag, command])
        .status()
        .with_context(|| format!("running {command}"))?;
    Ok(())
}
