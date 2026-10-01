//! Watching a group's root: the operating system reports changed paths,
//! which become the group paths whose subtrees pigeon rescans.

use std::path::{Path, PathBuf};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use pigeon_core::path::GroupPath;
use pigeon_store::disk::TEMPORARY_PREFIX;
use tokio::sync::mpsc;

/// What to rescan after a change on disk.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Rescan {
    /// The subtree at a path, or the file itself.
    Under(GroupPath),
    /// The whole root, when a change names no portable path or the system
    /// lost track of events.
    All,
}

/// The group path of a location under `root`, or of its deepest portable
/// folder; `None` for the root itself or pigeon's own temporary files.
#[must_use]
pub fn rescan_of(root: &Path, location: &Path) -> Option<Rescan> {
    let relative = location.strip_prefix(root).ok()?;
    let names: Vec<&str> = relative
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<_>>()
        .unwrap_or_default();
    if names.is_empty() {
        return Some(Rescan::All);
    }
    if names
        .last()
        .is_some_and(|name| name.starts_with(TEMPORARY_PREFIX))
    {
        return None;
    }
    (1..=names.len())
        .rev()
        .find_map(|depth| GroupPath::from_names(names[..depth].iter().copied()).ok())
        .map_or(Some(Rescan::All), |path| Some(Rescan::Under(path)))
}

/// Starts watching `root`, followed to the folder it may link to, since
/// some systems report changes under that folder's own path; the watcher
/// stops when dropped.
///
/// # Errors
///
/// Fails if the root does not exist or the system refuses to watch it.
pub fn watch(
    root: &Path,
    changes: mpsc::UnboundedSender<Rescan>,
) -> notify::Result<RecommendedWatcher> {
    let followed: PathBuf = root.canonicalize().map_err(notify::Error::io)?;
    let base = followed.clone();
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                if event.need_rescan() {
                    let _ = changes.send(Rescan::All);
                }
                for location in &event.paths {
                    if let Some(rescan) = rescan_of(&base, location) {
                        let _ = changes.send(rescan);
                    }
                }
            }
            Err(_) => {
                let _ = changes.send(Rescan::All);
            }
        })?;
    watcher.watch(&followed, RecursiveMode::Recursive)?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn locations_become_the_deepest_portable_path() {
        let root = Path::new("/g");
        let under = |text: &str| Some(Rescan::Under(GroupPath::parse(text).unwrap()));
        assert_eq!(rescan_of(root, Path::new("/g/a/b.txt")), under("a/b.txt"));
        assert_eq!(rescan_of(root, Path::new("/g/a/b?.txt")), under("a"));
        assert_eq!(rescan_of(root, Path::new("/g/con")), Some(Rescan::All));
        assert_eq!(rescan_of(root, Path::new("/g")), Some(Rescan::All));
        assert_eq!(rescan_of(root, Path::new("/g/a/.~pigeon-b.txt")), None);
        assert_eq!(rescan_of(root, Path::new("/elsewhere")), None);
    }

    #[tokio::test]
    async fn reports_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (sender, mut changes) = mpsc::unbounded_channel();
        let _watcher = watch(root, sender).unwrap();
        std::fs::write(root.join("new.txt"), "x").unwrap();
        let expected = Rescan::Under(GroupPath::parse("new.txt").unwrap());
        let seen = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(change) = changes.recv().await {
                if change == expected {
                    return true;
                }
            }
            false
        })
        .await;
        assert_eq!(seen, Ok(true));
    }
}
