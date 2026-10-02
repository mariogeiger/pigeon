//! The root as the place of the group's files, marked by its `.pigeon`
//! folder: recorded where the disk was last brought; paused while the root
//! or its mark is missing though files were synced there, as when its disk
//! is not plugged in; and moved to another root only into a folder holding
//! the group's `.pigeon` folder, where syncing resumes, or into an empty
//! one, where it starts afresh and the old root stays as it was.

use std::path::Path;

use anyhow::{Context, Result, bail};
use pigeon_core::path::GroupPath;
use pigeon_store::disk::fs_path;
use pigeon_store::index::hash_file;
use pigeon_store::state::State;

use crate::engine::{Inner, Work};
use crate::watch::Rescan;

/// The folder that marks a root as the group's, which holds its
/// statements.
pub(crate) const MARK: &str = ".pigeon";

/// What a folder holds, to become a group's root.
enum Holding {
    /// The group's `.pigeon` folder, every statement as the disk last
    /// showed it.
    Group,
    /// Nothing, or no folder is there.
    Nothing,
    /// Something else, as this says.
    Other(String),
}

/// Readies `root` to hold the group whose disk `state` records. A first
/// root is made, and so is the recorded one before anything was synced;
/// another root is recorded where it holds the group's `.pigeon` folder,
/// and starts afresh, made, where it holds nothing.
///
/// # Errors
///
/// Fails if another root holds something else than the group's files, or
/// the root cannot be made or recorded.
pub(crate) fn open_root(state: &State, root: &Path) -> Result<()> {
    let synced = !state.index_is_empty()?;
    match state.applied_root()? {
        None if synced => {}
        None => make(root)?,
        Some(recorded) if same_folder(&recorded, root) => {
            if !synced {
                make(root)?;
            }
        }
        Some(_) => match holding(state, root)? {
            Holding::Group => {}
            Holding::Nothing => {
                state.forget_disk()?;
                make(root)?;
            }
            Holding::Other(what) => bail!(
                "{what}: choose an empty folder to start afresh there, or the folder \
                 the group's files moved to"
            ),
        },
    }
    state.set_applied_root(root)?;
    Ok(())
}

/// Makes `root` with its mark.
fn make(root: &Path) -> Result<()> {
    let mark = root.join(MARK);
    std::fs::create_dir_all(&mark).with_context(|| format!("creating {}", mark.display()))
}

/// Whether `a` and `b` name one folder.
fn same_folder(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .is_ok_and(|a| b.canonicalize().is_ok_and(|b| a == b))
}

/// What `root` holds, compared with the statements `state` saw on disk.
fn holding(state: &State, root: &Path) -> Result<Holding> {
    let empty = match std::fs::read_dir(root) {
        Ok(mut entries) => entries.next().is_none(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error).with_context(|| format!("reading {}", root.display())),
    };
    if empty {
        return Ok(Holding::Nothing);
    }
    if !root.join(MARK).is_dir() {
        return Ok(Holding::Other(format!(
            "{} holds files but no {MARK} folder of this group",
            root.display()
        )));
    }
    let statements = GroupPath::parse(MARK).expect("the mark is a group path");
    for entry in state.index(Some(&statements))? {
        let Some(seen) = entry.seen else {
            continue;
        };
        let location = fs_path(root, &entry.path);
        if !hash_file(&location).is_ok_and(|hash| hash == seen.content.hash) {
            return Ok(Holding::Other(format!(
                "{} holds another {} than this group's",
                root.display(),
                entry.path
            )));
        }
    }
    Ok(Holding::Group)
}

impl Inner {
    /// Why the group's files pause, if they do: files were synced, yet the
    /// root or its mark is missing.
    pub(crate) fn root_problem(&self) -> Option<String> {
        let root = &self.root;
        if root.join(MARK).is_dir() {
            return None;
        }
        match self.state.index_is_empty() {
            Ok(true) => None,
            Ok(false) if !root.is_dir() => Some(format!(
                "{} is not there: is its disk plugged in?",
                root.display()
            )),
            Ok(false) => Some(format!(
                "{} holds no {MARK} folder: is its disk plugged in? If the group's files \
                 really are there, create {} to resume",
                root.display(),
                root.join(MARK).display()
            )),
            Err(error) => Some(error.to_string()),
        }
    }

    /// Pauses the group while the root has a problem, which it reports,
    /// and resumes it once the problem is gone with a rescan of the whole
    /// root; whether the group pauses.
    pub(crate) fn pause_without_root(&self, work: &mut Work) -> bool {
        let problem = self.root_problem();
        if problem != work.root_problem {
            match &problem {
                Some(problem) => {
                    self.report(problem);
                    work.watcher = None;
                }
                None => {
                    let _ = self.rescans.send(Rescan::All);
                }
            }
            work.root_problem = problem;
        }
        work.root_problem.is_some()
    }
}
