//! Watching a group's root and the destinations of its placed folders: the
//! operating system reports changed paths, which become the group paths
//! whose subtrees pigeon rescans; a path only opened or read changed
//! nothing, so that pigeon's own scans wake no other, and events the system
//! lost track of make one rescan of the whole root. A walk waits until the
//! watcher watches every folder it reported new, so that each file shows in
//! the walk or in a change.

use std::path::{Path, PathBuf};

use notify::event::{AccessKind, AccessMode, EventKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use pigeon_core::path::GroupPath;
use pigeon_core::places::Place;
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

/// A watched folder: the root, or the destination of a placed folder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Watched {
    /// Where the folder is, followed through any link.
    pub location: PathBuf,
    /// The group path it holds, `None` for the root.
    pub folder: Option<GroupPath>,
}

impl Watched {
    /// The folders to watch for `root` and `places`, leaving out the
    /// destinations that are not there.
    #[must_use]
    pub fn all(root: &Path, places: &[Place]) -> Vec<Self> {
        let mut watched: Vec<Self> = root
            .canonicalize()
            .ok()
            .map(|location| Self {
                location,
                folder: None,
            })
            .into_iter()
            .collect();
        watched.extend(places.iter().filter_map(|place| {
            Some(Self {
                location: place.destination.canonicalize().ok()?,
                folder: Some(place.folder.clone()),
            })
        }));
        watched
    }
}

/// The group path of a location under a watched folder, or of its deepest
/// portable folder; `None` for pigeon's own temporary files and what lies
/// elsewhere.
#[must_use]
pub fn rescan_of(watched: &Watched, location: &Path) -> Option<Rescan> {
    let relative = location.strip_prefix(&watched.location).ok()?;
    let mut names: Vec<&str> = watched
        .folder
        .as_ref()
        .map(|folder| folder.names().collect())
        .unwrap_or_default();
    match relative
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
    {
        Some(more) => names.extend(more),
        None => names.clear(),
    }
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

/// Whether an event of `kind` may come with a change on disk: anything
/// but opening, reading or closing what was not written.
fn changes_disk(kind: EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

/// What to rescan after `event` under `watched`: the whole root when the
/// system lost track of events, whatever the event says besides, and
/// otherwise the paths of an event that may change the disk.
fn rescans(watched: &[Watched], event: &notify::Event) -> Vec<Rescan> {
    if event.need_rescan() {
        return vec![Rescan::All];
    }
    if !changes_disk(event.kind) {
        return Vec::new();
    }
    event
        .paths
        .iter()
        .filter_map(|location| watched.iter().find_map(|one| rescan_of(one, location)))
        .collect()
}

/// Starts watching every folder of `watched`, each followed to the folder
/// it may link to, since some systems report changes under that folder's
/// own path; the watcher stops when dropped.
///
/// # Errors
///
/// Fails if the system refuses to watch a folder.
pub fn watch(
    watched: Vec<Watched>,
    changes: mpsc::UnboundedSender<Rescan>,
) -> notify::Result<RecommendedWatcher> {
    let locations: Vec<PathBuf> = watched.iter().map(|one| one.location.clone()).collect();
    let mut system =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                for rescan in rescans(&watched, &event) {
                    let _ = changes.send(rescan);
                }
            }
            Err(_) => {
                let _ = changes.send(Rescan::All);
            }
        })?;
    for location in &locations {
        system.watch(location, RecursiveMode::Recursive)?;
    }
    Ok(system)
}

/// Waits until `watcher` handled every change it reported so far. A system
/// that watches each folder apart, as inotify does, reports a new folder
/// before it watches it, so that a file made there meanwhile shows in no
/// change: a walk of the folder begun after this sees it.
pub fn catch_up(watcher: &RecommendedWatcher) {
    let _ = watcher.watched_paths();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::time::Duration;

    #[test]
    fn locations_become_the_deepest_portable_path() {
        let root = &Watched {
            location: PathBuf::from("/g"),
            folder: None,
        };
        let under = |text: &str| Some(Rescan::Under(GroupPath::parse(text).unwrap()));
        assert_eq!(rescan_of(root, Path::new("/g/a/b.txt")), under("a/b.txt"));
        assert_eq!(rescan_of(root, Path::new("/g/a/b?.txt")), under("a"));
        assert_eq!(rescan_of(root, Path::new("/g/con")), Some(Rescan::All));
        assert_eq!(rescan_of(root, Path::new("/g")), Some(Rescan::All));
        assert_eq!(rescan_of(root, Path::new("/g/a/.~pigeon-b.txt")), None);
        assert_eq!(rescan_of(root, Path::new("/elsewhere")), None);
        let disk = &Watched {
            location: PathBuf::from("/disk/videos"),
            folder: Some(GroupPath::parse("a/videos").unwrap()),
        };
        assert_eq!(
            rescan_of(disk, Path::new("/disk/videos/v.mp4")),
            under("a/videos/v.mp4")
        );
        assert_eq!(
            rescan_of(disk, Path::new("/disk/videos")),
            under("a/videos")
        );
        assert_eq!(rescan_of(disk, Path::new("/disk")), None);
    }

    #[test]
    fn events_the_system_lost_track_of_rescan_the_whole_root() {
        use notify::event::{CreateKind, Flag};
        let watched = [Watched {
            location: PathBuf::from("/g"),
            folder: None,
        }];
        let event = |kind| notify::Event::new(kind).add_path(PathBuf::from("/g/a.txt"));
        let read = EventKind::Access(AccessKind::Read);
        let created = EventKind::Create(CreateKind::File);
        for kind in [EventKind::Other, read, created] {
            let lost = event(kind).set_flag(Flag::Rescan);
            assert_eq!(rescans(&watched, &lost), [Rescan::All]);
        }
        let overflow = notify::Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert_eq!(rescans(&watched, &overflow), [Rescan::All]);
        assert_eq!(rescans(&watched, &event(read)), []);
        let under = Rescan::Under(GroupPath::parse("a.txt").unwrap());
        assert_eq!(rescans(&watched, &event(created)), [under]);
    }

    #[tokio::test]
    async fn reading_changes_nothing_while_writing_does() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("read.txt"), "x").unwrap();
        let (sender, mut changes) = mpsc::unbounded_channel();
        let _watcher = watch(Watched::all(root, &[]), sender).unwrap();
        let under = |text: &str| Rescan::Under(GroupPath::parse(text).unwrap());
        let mut next = async || {
            tokio::time::timeout(Duration::from_secs(5), changes.recv())
                .await
                .unwrap()
                .unwrap()
        };
        std::fs::write(root.join("marker.txt"), "m").unwrap();
        while next().await != under("marker.txt") {}
        for entry in std::fs::read_dir(root).unwrap() {
            std::fs::read(entry.unwrap().path()).unwrap();
        }
        std::fs::write(root.join("written.txt"), "y").unwrap();
        let mut seen = Vec::new();
        loop {
            match next().await {
                change if change == under("written.txt") => break,
                change if change == under("marker.txt") => {}
                change => seen.push(change),
            }
        }
        assert_eq!(seen, []);
    }

    #[tokio::test]
    async fn reports_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (sender, mut changes) = mpsc::unbounded_channel();
        let _watcher = watch(Watched::all(root, &[]), sender).unwrap();
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

    /// The paths of the files under `location`, relative to `root`.
    fn files_under(root: &Path, location: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(location) else {
            return Vec::new();
        };
        entries
            .flat_map(|entry| {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files_under(root, &path)
                } else {
                    let relative = path.strip_prefix(root).unwrap();
                    vec![relative.to_str().unwrap().replace('\\', "/")]
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn a_walk_after_catching_up_sees_every_file_no_change_reports() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (sender, mut changes) = mpsc::unbounded_channel();
        let watcher = watch(Watched::all(&root, &[]), sender).unwrap();
        let written: BTreeSet<String> = (0..100)
            .flat_map(|folder| (0..100).map(move |n| format!("d{folder}/a/b/{n}.txt")))
            .collect();
        let writer = std::thread::spawn({
            let (root, written) = (root.clone(), written.clone());
            move || {
                for file in &written {
                    let location = root.join(file);
                    std::fs::create_dir_all(location.parent().unwrap()).unwrap();
                    std::fs::write(location, "x").unwrap();
                }
                std::fs::write(root.join("marker"), "m").unwrap();
            }
        });
        let mut seen = BTreeSet::new();
        while !seen.contains("marker") {
            let change = tokio::time::timeout(Duration::from_secs(5), changes.recv())
                .await
                .unwrap()
                .unwrap();
            let location = match &change {
                Rescan::Under(path) => pigeon_store::disk::fs_path(&root, path),
                Rescan::All => root.clone(),
            };
            if location.is_dir() {
                catch_up(&watcher);
                seen.extend(files_under(&root, &location));
            } else if let Rescan::Under(path) = change {
                seen.insert(path.as_str().to_owned());
            }
        }
        writer.join().unwrap();
        let missed: Vec<_> = written.difference(&seen).collect();
        assert!(
            missed.is_empty(),
            "{} files missed: {missed:?}",
            missed.len()
        );
    }
}
