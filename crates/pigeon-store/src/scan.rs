//! Walking a group's root: every regular file pigeon may consider, with its
//! metadata, skipping what `.pigeonignore` files exclude, and the files
//! whose names no system-portable path can hold.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use pigeon_core::path::{GroupPath, PathKey};

use crate::disk::{Stat, TEMPORARY_PREFIX};

/// The file whose gitignore-syntax rules keep files out of publication.
pub const IGNORE_FILE: &str = ".pigeonignore";

/// A file found on disk under a portable name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Found {
    pub path: GroupPath,
    /// Where the file is, which may differ from the path by normalization.
    pub location: PathBuf,
    pub stat: Stat,
}

/// A file found on disk that cannot be published as it is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Skipped {
    pub location: PathBuf,
    pub reason: String,
}

/// Everything a walk of the root saw.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Scan {
    pub files: BTreeMap<PathKey, Found>,
    pub skipped: Vec<Skipped>,
}

/// Walks `root`, or only `under` it when given, following no link.
///
/// # Panics
///
/// Panics if the walk leaves `root`, which it cannot.
#[must_use]
pub fn scan(root: &Path, under: Option<&GroupPath>) -> Scan {
    let start = under.map_or_else(
        || root.to_path_buf(),
        |path| crate::disk::fs_path(root, path),
    );
    let mut result = Scan::default();
    if !start.exists() {
        return result;
    }
    let walk = WalkBuilder::new(&start)
        .standard_filters(false)
        .add_custom_ignore_filename(IGNORE_FILE)
        .parents(under.is_some())
        .follow_links(false)
        .filter_entry(|entry| {
            !entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(TEMPORARY_PREFIX))
        })
        .build();
    let mut found = Vec::new();
    for entry in walk {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                result.skipped.push(Skipped {
                    location: start.clone(),
                    reason: error.to_string(),
                });
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let location = entry.path().to_path_buf();
        let skip = |reason: String| Skipped {
            location: location.clone(),
            reason,
        };
        let relative = location
            .strip_prefix(root)
            .expect("the walk stays under the root");
        let Some(text) = relative.to_str() else {
            result
                .skipped
                .push(skip("the name is not valid Unicode".into()));
            continue;
        };
        let names: Vec<&str> = relative
            .components()
            .map(|part| part.as_os_str().to_str().expect("a valid Unicode path"))
            .collect();
        let path = match GroupPath::from_names(names) {
            Ok(path) => path,
            Err(error) => {
                result.skipped.push(skip(format!("{text}: {error}")));
                continue;
            }
        };
        match entry.metadata() {
            Ok(metadata) => found.push(Found {
                path,
                location,
                stat: Stat::of(&metadata),
            }),
            Err(error) => result.skipped.push(skip(error.to_string())),
        }
    }
    found.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    for file in found {
        let key = file.path.key();
        if let Some(first) = result.files.get(&key) {
            result.skipped.push(Skipped {
                reason: format!("{} differs only by case from {}", file.path, first.path),
                location: file.location,
            });
        } else {
            result.files.insert(key, file);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn paths(scan: &Scan) -> Vec<&str> {
        scan.files.values().map(|file| file.path.as_str()).collect()
    }

    #[test]
    fn finds_hidden_files_and_honours_pigeonignore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".pigeon/members/mario", "{}");
        write(root, "src/@mario/.pigeonignore", "target/\n*.log\n");
        write(root, "src/@mario/main.rs", "fn main() {}");
        write(root, "src/@mario/target/debug/app", "bin");
        write(root, "src/@mario/run.log", "log");
        write(root, "run.log", "not ignored here");
        write(root, ".gitignore", "*.rs\n");
        write(root, "src/.~pigeon-main.rs", "partial");
        let scan = scan(root, None);
        assert_eq!(
            paths(&scan),
            [
                ".gitignore",
                ".pigeon/members/mario",
                "run.log",
                "src/@mario/.pigeonignore",
                "src/@mario/main.rs",
            ]
        );
        assert!(scan.skipped.is_empty());
    }

    #[test]
    fn scans_only_under_a_prefix() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a/x", "");
        write(dir.path(), "b/y", "");
        write(dir.path(), "b/z.log", "");
        write(dir.path(), ".pigeonignore", "*.log\n");
        let under = GroupPath::parse("b").unwrap();
        assert_eq!(paths(&scan(dir.path(), Some(&under))), ["b/y"]);
        let missing = GroupPath::parse("c").unwrap();
        assert!(scan(dir.path(), Some(&missing)).files.is_empty());
    }

    #[test]
    fn skips_unportable_names() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "ok.txt", "");
        if cfg!(unix) {
            write(dir.path(), "what?.txt", "");
            write(dir.path(), "aux/file", "");
        }
        let scan = scan(dir.path(), None);
        assert_eq!(paths(&scan), ["ok.txt"]);
        if cfg!(unix) {
            assert_eq!(scan.skipped.len(), 2);
        }
    }

    #[test]
    fn skips_names_that_differ_only_by_case() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Readme.md", "a");
        write(dir.path(), "README.md", "b");
        let scan = scan(dir.path(), None);
        if scan.files.len() + scan.skipped.len() == 2 {
            assert_eq!(paths(&scan), ["README.md"]);
            assert!(scan.skipped[0].reason.contains("differs only by case"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn follows_no_symbolic_link() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "real/file", "");
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real/file"), dir.path().join("flink")).unwrap();
        assert_eq!(paths(&scan(dir.path(), None)), ["real/file"]);
    }
}
