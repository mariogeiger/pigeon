//! A group's files as a tree of folders, which the web UI's Files page and
//! the tree of `pigeon setup` both draw: each folder sums the sizes under
//! it, keeps their latest time, counts the edits waiting to be published in
//! it and the changes waiting for someone, and tells
//! whether every file under it with a box is followed, some, or none. Rows
//! come folders first, then files, each by name; a row shows once every
//! folder above it is open.

use std::collections::BTreeMap;

/// How many of the files under a box are followed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Followed {
    All,
    Some,
    None,
}

/// What one file adds to the folders above it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts<'a> {
    pub size: u64,
    /// Whether the selection follows it; nothing for a file with no box.
    pub followed: Option<bool>,
    /// When its current version was made, in RFC 3339.
    pub time: Option<&'a str>,
    /// Whether an edit of it waits to be published.
    pub waiting: bool,
    /// How many changes of it wait for someone.
    pub changes: usize,
}

/// A file of the tree.
pub trait Leaf {
    /// Its path in the group, without a leading slash.
    fn path(&self) -> &str;
    fn facts(&self) -> Facts<'_>;
}

impl<T: Leaf> Leaf for &T {
    fn path(&self) -> &str {
        (*self).path()
    }

    fn facts(&self) -> Facts<'_> {
        (*self).facts()
    }
}

/// What the files under a folder add up to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub size: u64,
    pub time: Option<String>,
    pub waiting: usize,
    pub changes: usize,
    boxes: usize,
    followed: usize,
}

impl Summary {
    fn add(&mut self, facts: &Facts) {
        self.size += facts.size;
        if let Some(time) = facts.time
            && self.time.as_deref().is_none_or(|latest| latest < time)
        {
            self.time = Some(time.to_owned());
        }
        self.waiting += usize::from(facts.waiting);
        self.changes += facts.changes;
        if let Some(followed) = facts.followed {
            self.boxes += 1;
            self.followed += usize::from(followed);
        }
    }

    /// How many of the files with a box are followed; nothing when none
    /// has one.
    #[must_use]
    pub fn followed(&self) -> Option<Followed> {
        match self.followed {
            _ if self.boxes == 0 => None,
            0 => Some(Followed::None),
            followed if followed == self.boxes => Some(Followed::All),
            _ => Some(Followed::Some),
        }
    }
}

/// A folder: its path, empty for the group's root, what its files add up
/// to, its subfolders by name, and its own files by path.
#[derive(Debug)]
pub struct Folder<T> {
    pub path: String,
    pub summary: Summary,
    folders: BTreeMap<String, Folder<T>>,
    files: Vec<T>,
}

/// A line of the tree, `depth` folders below the root.
pub enum Row<'a, T> {
    Folder { folder: &'a Folder<T>, depth: usize },
    File { leaf: &'a T, depth: usize },
}

impl<T: Leaf> Row<'_, T> {
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Folder { folder, .. } => &folder.path,
            Self::File { leaf, .. } => leaf.path(),
        }
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        match self {
            Self::Folder { depth, .. } | Self::File { depth, .. } => *depth,
        }
    }
}

impl<T: Leaf> Folder<T> {
    fn empty(path: String) -> Self {
        Self {
            path,
            summary: Summary::default(),
            folders: BTreeMap::new(),
            files: Vec::new(),
        }
    }

    /// The tree of `leaves`, rooted at the group.
    pub fn root(leaves: impl IntoIterator<Item = T>) -> Self {
        let mut root = Self::empty(String::new());
        for leaf in leaves {
            root.insert(leaf);
        }
        root.sort();
        root
    }

    fn insert(&mut self, leaf: T) {
        self.summary.add(&leaf.facts());
        let rest = leaf.path()[self.path.len()..].trim_start_matches('/');
        let Some((name, _)) = rest.split_once('/') else {
            self.files.push(leaf);
            return;
        };
        let name = name.to_owned();
        let path = if self.path.is_empty() {
            name.clone()
        } else {
            format!("{}/{name}", self.path)
        };
        self.folders
            .entry(name)
            .or_insert_with(|| Self::empty(path))
            .insert(leaf);
    }

    fn sort(&mut self) {
        self.files.sort_by(|a, b| a.path().cmp(b.path()));
        for folder in self.folders.values_mut() {
            folder.sort();
        }
    }

    /// Its last name, empty for the root.
    #[must_use]
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or_default()
    }

    /// Whether it holds neither files nor folders.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.files.is_empty()
    }

    /// Every file under it, at any depth.
    pub fn leaves(&self) -> Box<dyn Iterator<Item = &T> + '_> {
        Box::new(
            self.files
                .iter()
                .chain(self.folders.values().flat_map(Folder::leaves)),
        )
    }

    /// Every line under it, at any depth: each folder, then what it holds.
    #[must_use]
    pub fn rows(&self) -> Vec<Row<'_, T>> {
        let mut rows = Vec::new();
        self.push_rows(0, &mut rows);
        rows
    }

    fn push_rows<'a>(&'a self, depth: usize, rows: &mut Vec<Row<'a, T>>) {
        for folder in self.folders.values() {
            rows.push(Row::Folder { folder, depth });
            folder.push_rows(depth + 1, rows);
        }
        for leaf in &self.files {
            rows.push(Row::File { leaf, depth });
        }
    }
}

/// The folders above `path`, from the root's first down.
pub fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(at, _)| &path[..at])
}

/// Whether `path` is the folder `folder` or lies inside it.
#[must_use]
pub fn contains(folder: &str, path: &str) -> bool {
    path.strip_prefix(folder)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct File(&'static str, u64, Option<bool>, &'static str, bool);

    impl Leaf for File {
        fn path(&self) -> &str {
            self.0
        }

        fn facts(&self) -> Facts<'_> {
            Facts {
                size: self.1,
                followed: self.2,
                time: Some(self.3),
                waiting: self.4,
                changes: usize::from(self.4),
            }
        }
    }

    fn tree() -> Folder<File> {
        Folder::root([
            File("readme", 1, Some(false), "2026-01-01T00:00:00Z", false),
            File("docs/b.txt", 20, Some(true), "2026-03-01T00:00:00Z", true),
            File(
                "docs/a/c.txt",
                300,
                Some(false),
                "2026-02-01T00:00:00Z",
                false,
            ),
            File("docs/a/d.txt", 4, None, "2026-04-01T00:00:00Z", true),
            File("alpha", 5, Some(true), "2025-01-01T00:00:00Z", false),
        ])
    }

    #[test]
    fn rows_come_folders_first_then_files_each_by_name() {
        let tree = tree();
        let rows: Vec<(String, usize)> = tree
            .rows()
            .iter()
            .map(|row| (row.path().to_owned(), row.depth()))
            .collect();
        let expected = [
            ("docs", 0),
            ("docs/a", 1),
            ("docs/a/c.txt", 2),
            ("docs/a/d.txt", 2),
            ("docs/b.txt", 1),
            ("alpha", 0),
            ("readme", 0),
        ];
        assert_eq!(rows, expected.map(|(path, depth)| (path.to_owned(), depth)));
    }

    #[test]
    fn folders_sum_sizes_keep_the_latest_time_and_count_what_waits() {
        let tree = tree();
        let rows = tree.rows();
        let Row::Folder { folder: docs, .. } = &rows[0] else {
            panic!("docs is a folder");
        };
        let Row::Folder { folder: a, .. } = &rows[1] else {
            panic!("docs/a is a folder");
        };
        assert_eq!(docs.summary.size, 324);
        assert_eq!(docs.summary.time.as_deref(), Some("2026-04-01T00:00:00Z"));
        assert_eq!(docs.summary.waiting, 2);
        assert_eq!(docs.summary.changes, 2);
        assert_eq!(docs.summary.followed(), Some(Followed::Some));
        assert_eq!(a.summary.followed(), Some(Followed::None));
        assert_eq!(a.name(), "a");
        assert_eq!(a.leaves().count(), 2);
        assert_eq!(tree.summary.size, 330);
        let only_drafts = Folder::root([File("x/y", 1, None, "", false)]);
        assert_eq!(only_drafts.summary.followed(), None);
    }

    #[test]
    fn a_path_lies_under_each_folder_above_it() {
        assert_eq!(ancestors("a/b/c").collect::<Vec<_>>(), ["a", "a/b"]);
        assert!(contains("a/b", "a/b") && contains("a/b", "a/b/c"));
        assert!(!contains("a/b", "a/bc"));
    }
}
