//! The previous program: putting a program at the path of the one it
//! replaces while keeping that one beside it as `pigeon.previous`, so that
//! an update can be undone by putting the previous program back.

use std::env::consts::EXE_SUFFIX;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Where the program at `program` is kept once another replaces it.
#[must_use]
pub fn path_beside(program: &Path) -> PathBuf {
    program.with_file_name(format!("pigeon.previous{EXE_SUFFIX}"))
}

/// Puts a copy of the file `new` at the path `program`, which keeps what
/// was there as the previous program; the file is complete when it takes
/// the path, and a program that runs from the old one is not disturbed.
///
/// # Errors
///
/// Fails if a file cannot be copied or moved beside `program`.
pub fn replace(program: &Path, new: &Path) -> Result<()> {
    let previous = path_beside(program);
    let staged = program.with_file_name(format!("pigeon.new{EXE_SUFFIX}"));
    std::fs::copy(new, &staged).with_context(|| {
        format!(
            "copying {} to {}: is that folder writable?",
            new.display(),
            staged.display()
        )
    })?;
    let kept = program.exists();
    if kept {
        std::fs::rename(program, &previous)
            .with_context(|| format!("keeping {} as {}", program.display(), previous.display()))?;
    }
    std::fs::rename(&staged, program).map_err(|error| {
        if kept {
            let _ = std::fs::rename(&previous, program);
        }
        anyhow::Error::new(error).context(format!("installing {}", program.display()))
    })
}

/// Puts the previous program back at the path `program`, which keeps what
/// was there as the previous program.
///
/// # Errors
///
/// Fails if there is no previous program, or a file cannot be copied or
/// moved beside `program`.
pub fn restore(program: &Path) -> Result<()> {
    let previous = path_beside(program);
    if !previous.is_file() {
        bail!(
            "there is no previous program at {}: nothing to go back to",
            previous.display()
        );
    }
    replace(program, &previous)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn a_replaced_program_stays_beside_the_one_that_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join(format!("pigeon{EXE_SUFFIX}"));
        let build = dir.path().join("build");
        std::fs::write(&program, "one").unwrap();
        std::fs::write(&build, "two").unwrap();
        replace(&program, &build).unwrap();
        assert_eq!(read(&program), "two");
        assert_eq!(read(&path_beside(&program)), "one");
        assert_eq!(read(&build), "two");
        assert!(!dir.path().join(format!("pigeon.new{EXE_SUFFIX}")).exists());
    }

    #[test]
    fn restoring_swaps_the_program_with_the_previous_one_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join(format!("pigeon{EXE_SUFFIX}"));
        std::fs::write(&program, "two").unwrap();
        std::fs::write(path_beside(&program), "one").unwrap();
        restore(&program).unwrap();
        assert_eq!(read(&program), "one");
        assert_eq!(read(&path_beside(&program)), "two");
        restore(&program).unwrap();
        assert_eq!(read(&program), "two");
    }

    #[test]
    fn with_no_previous_program_there_is_nothing_to_go_back_to() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join(format!("pigeon{EXE_SUFFIX}"));
        std::fs::write(&program, "two").unwrap();
        assert!(
            restore(&program)
                .unwrap_err()
                .to_string()
                .contains("no previous program")
        );
        assert_eq!(read(&program), "two");
    }

    #[test]
    fn a_program_is_installed_where_there_was_none() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join(format!("pigeon{EXE_SUFFIX}"));
        let build = dir.path().join("build");
        std::fs::write(&build, "two").unwrap();
        replace(&program, &build).unwrap();
        assert_eq!(read(&program), "two");
        assert!(!path_beside(&program).exists());
    }
}
