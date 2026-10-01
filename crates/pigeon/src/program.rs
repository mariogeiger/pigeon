//! The program a daemon runs: the file it started from and the blake3 hash
//! that file had then, which tells whether an update has since installed
//! another program there; and replacing the process with the program now
//! at that path, by exec on Unix and by spawning it then exiting on
//! Windows.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// The program file this process started from.
#[derive(Clone, Debug)]
pub struct Program {
    pub path: PathBuf,
    /// The blake3 hash of the file when this process started.
    pub hash: blake3::Hash,
}

fn hash_file(path: &Path) -> Result<blake3::Hash> {
    let file = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(file)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(hasher.finalize())
}

impl Program {
    /// The program of this process.
    ///
    /// # Errors
    ///
    /// Fails if its file cannot be found or read.
    pub fn running() -> Result<Self> {
        let path = std::env::current_exe().context("finding the running program")?;
        let hash = hash_file(&path)?;
        Ok(Self { path, hash })
    }

    /// Whether the file at the path now holds another program.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read.
    pub fn replaced(&self) -> Result<bool> {
        Ok(hash_file(&self.path)? != self.hash)
    }

    /// Replaces this process with the program now at the path, run with
    /// this process's arguments; returns only why it could not.
    #[must_use]
    pub fn restart(&self) -> anyhow::Error {
        let mut command = Command::new(&self.path);
        command.args(std::env::args_os().skip(1));
        let error = replace_process(&mut command);
        anyhow::Error::new(error).context(format!("restarting {}", self.path.display()))
    }
}

#[cfg(unix)]
fn replace_process(command: &mut Command) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    command.exec()
}

#[cfg(not(unix))]
fn replace_process(command: &mut Command) -> std::io::Error {
    match command.spawn() {
        Ok(_) => std::process::exit(0),
        Err(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_replaced_once_its_file_holds_other_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pigeon");
        std::fs::write(&path, b"one build").unwrap();
        let program = Program {
            hash: hash_file(&path).unwrap(),
            path: path.clone(),
        };
        assert!(!program.replaced().unwrap());
        std::fs::write(&path, b"another build").unwrap();
        assert!(program.replaced().unwrap());
        std::fs::write(&path, b"one build").unwrap();
        assert!(!program.replaced().unwrap());
    }
}
