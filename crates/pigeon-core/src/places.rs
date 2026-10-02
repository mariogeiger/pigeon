//! Places: the folders of the tree a machine keeps at other destinations on
//! its disks, leaving a link at the folder's place in the root. Neither the
//! folders nor the destinations nest, in each other or in the root, so that
//! every file keeps its group path on every machine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::path::GroupPath;
use crate::statement::is_statement;

/// One folder kept at a destination.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Place {
    pub folder: GroupPath,
    pub destination: PathBuf,
}

/// Why a folder cannot go to a destination.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlaceError {
    #[error("{0} holds the group's statements, which stay in the root")]
    Statements(GroupPath),
    #[error("{} is not an absolute path", .0.display())]
    Relative(PathBuf),
    #[error("{} and the root {} lie one inside the other", .destination.display(), .root.display())]
    Root { destination: PathBuf, root: PathBuf },
    #[error("{folder} lies inside or around {other}, which has its own destination")]
    NestedFolder { folder: GroupPath, other: GroupPath },
    #[error("{} lies inside or around {}, the destination of {other}", .destination.display(), .taken.display())]
    NestedDestination {
        destination: PathBuf,
        taken: PathBuf,
        other: GroupPath,
    },
}

/// Whether `a` and `b` are one folder or lie one inside the other, without
/// regard to case.
fn nested(a: &GroupPath, b: &GroupPath) -> bool {
    let (a, b) = (a.key(), b.key());
    a.is_within(&b) || b.is_within(&a)
}

/// Whether `a` and `b` are one path or lie one inside the other.
fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// A machine's places, by folder.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Places(BTreeMap<GroupPath, PathBuf>);

impl Places {
    /// The places `list` names, once each, as stored.
    pub fn new(list: impl IntoIterator<Item = Place>) -> Self {
        Self(
            list.into_iter()
                .map(|place| (place.folder, place.destination))
                .collect(),
        )
    }

    /// Keeps `folder` at `destination` instead of under `root`, replacing
    /// any destination the folder had.
    ///
    /// # Errors
    ///
    /// Returns why the folder cannot go there; nothing changes then.
    pub fn set(
        &mut self,
        root: &Path,
        folder: GroupPath,
        destination: PathBuf,
    ) -> Result<(), PlaceError> {
        if is_statement(&folder.key()) {
            return Err(PlaceError::Statements(folder));
        }
        if !destination.is_absolute() {
            return Err(PlaceError::Relative(destination));
        }
        if overlap(&destination, root) {
            return Err(PlaceError::Root {
                destination,
                root: root.to_path_buf(),
            });
        }
        for (other, taken) in &self.0 {
            if other.key() == folder.key() {
                continue;
            }
            if nested(&folder, other) {
                return Err(PlaceError::NestedFolder {
                    folder,
                    other: other.clone(),
                });
            }
            if overlap(&destination, taken) {
                return Err(PlaceError::NestedDestination {
                    destination,
                    taken: taken.clone(),
                    other: other.clone(),
                });
            }
        }
        self.0.retain(|other, _| other.key() != folder.key());
        self.0.insert(folder, destination);
        Ok(())
    }

    /// Keeps `folder` under the root again; whether it had a destination.
    pub fn remove(&mut self, folder: &GroupPath) -> bool {
        let before = self.0.len();
        self.0.retain(|other, _| other.key() != folder.key());
        self.0.len() != before
    }

    /// The destination of exactly `folder`.
    #[must_use]
    pub fn get(&self, folder: &GroupPath) -> Option<&Path> {
        self.0
            .iter()
            .find(|(other, _)| other.key() == folder.key())
            .map(|(_, destination)| destination.as_path())
    }

    /// The placed folder that holds `path`, the folder itself included.
    #[must_use]
    pub fn holding(&self, path: &GroupPath) -> Option<&GroupPath> {
        let key = path.key();
        self.0.keys().find(|folder| key.is_within(&folder.key()))
    }

    /// Every place, by folder.
    pub fn iter(&self) -> impl Iterator<Item = Place> + '_ {
        self.0.iter().map(|(folder, destination)| Place {
            folder: folder.clone(),
            destination: destination.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(text: &str) -> GroupPath {
        GroupPath::parse(text).unwrap()
    }

    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            "C:\\cheapmo"
        } else {
            "/cheapmo"
        })
    }

    fn absolute(text: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!("D:{}", text.replace('/', "\\")))
        } else {
            PathBuf::from(text)
        }
    }

    #[test]
    fn folders_and_destinations_never_nest() {
        let mut places = Places::default();
        places
            .set(&root(), folder("videos"), absolute("/disk/videos"))
            .unwrap();
        for (path, destination) in [
            ("videos/old", "/disk/other"),
            ("docs", "/disk/videos/docs"),
            ("docs", "/disk"),
            (".pigeon/members", "/disk/members"),
            ("docs", "relative"),
        ] {
            let destination = if destination.starts_with('/') {
                absolute(destination)
            } else {
                PathBuf::from(destination)
            };
            assert!(
                places
                    .set(&root(), folder(path), destination.clone())
                    .is_err(),
                "{path} at {}",
                destination.display()
            );
        }
        let inside_root = root().join("docs");
        assert!(matches!(
            places.set(&root(), folder("docs"), inside_root),
            Err(PlaceError::Root { .. })
        ));
        places
            .set(&root(), folder("docs"), absolute("/disk/docs"))
            .unwrap();
        assert_eq!(places.iter().count(), 2);
    }

    #[test]
    fn a_folder_moves_and_returns() {
        let mut places = Places::default();
        places
            .set(&root(), folder("videos"), absolute("/a/videos"))
            .unwrap();
        places
            .set(&root(), folder("Videos"), absolute("/b/videos"))
            .unwrap();
        assert_eq!(places.iter().count(), 1);
        assert_eq!(
            places.get(&folder("videos")),
            Some(absolute("/b/videos").as_path())
        );
        assert_eq!(
            places.holding(&folder("videos/2026/a.mp4")),
            Some(&folder("Videos"))
        );
        assert_eq!(places.holding(&folder("videos2/a.mp4")), None);
        assert!(places.remove(&folder("VIDEOS")));
        assert!(!places.remove(&folder("videos")));
        assert_eq!(places, Places::default());
    }
}
