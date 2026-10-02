//! Walking a group's root as a [`Prober`] sees it: every regular file in
//! sight with its metadata, the names no portable path holds, the folders
//! that could not be read and the files pigeon was writing when it stopped.
//! A walk under a path also sees the names its folder holds that differ
//! from the path's last name only by case or spelling.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pigeon_core::path::{GroupPath, PathKey, name_key};

use crate::probe::{Found, Probe, Prober, Stop, Unportable, file_at, leads_into};

/// Everything a walk of the root saw.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Scan {
    pub files: BTreeMap<PathKey, Found>,
    pub unportable: Vec<Unportable>,
    /// Why folders, files or ignore files could not be read.
    pub errors: Vec<String>,
    /// The temporary files pigeon writes before moving them into place.
    pub temporaries: Vec<PathBuf>,
}

impl Prober {
    /// Walks the folder or file at `under`, or the whole root.
    pub fn scan(&mut self, under: Option<&GroupPath>) -> Scan {
        let mut scan = Scan::default();
        let names: Vec<&str> = under.map(|path| path.names().collect()).unwrap_or_default();
        match names.split_last() {
            None => self.walk(self.root.clone(), Vec::new(), &mut scan),
            Some((last, folders)) => match self.descend(folders) {
                Ok((folder, spelled)) => self.scan_entry(&folder, spelled, last, &mut scan),
                Err(Stop::Told(probe)) => scan.take(probe),
                Err(Stop::Missing(..)) => {}
            },
        }
        scan.errors.extend(self.rules.errors().iter().cloned());
        scan
    }

    /// Walks the entry `name` of `folder`, at `spelled`, with the names of
    /// the folder that no group path holds and that differ from `name`
    /// only by case or spelling.
    fn scan_entry(&mut self, folder: &Path, mut spelled: Vec<String>, name: &str, scan: &mut Scan) {
        if let Ok(listing) = self.listing(folder) {
            let key = name_key(name);
            scan.unportable.extend(
                listing
                    .unportable
                    .iter()
                    .filter(|unportable| name_key(&unportable.name) == key)
                    .map(|unportable| unportable.in_folder(&spelled)),
            );
        }
        match self.find(folder, name, true) {
            Ok(Some(entry)) => {
                spelled.push(entry.name.clone());
                let location = folder.join(&entry.written);
                if leads_into(&self.placed, &spelled, &entry) {
                    self.walk(location, spelled, scan);
                } else {
                    scan.take(file_at(&spelled, &location, &entry));
                }
            }
            Ok(None) => {}
            Err(probe) => scan.take(probe),
        }
    }

    /// Walks every folder pigeon goes into from `folder`, at `spelled`.
    fn walk(&mut self, folder: PathBuf, spelled: Vec<String>, scan: &mut Scan) {
        let mut folders = vec![(folder, spelled)];
        while let Some((folder, spelled)) = folders.pop() {
            let Self {
                listings,
                rules,
                placed,
                ..
            } = &mut *self;
            let listing = match crate::probe::listing(listings, rules, &folder) {
                Ok(listing) => listing,
                Err(error) => {
                    scan.errors.push(error.clone());
                    continue;
                }
            };
            scan.unportable.extend(
                listing
                    .unportable
                    .iter()
                    .map(|unportable| unportable.in_folder(&spelled)),
            );
            scan.temporaries.extend(listing.temporaries.iter().cloned());
            for entry in listing.entries.values() {
                let mut names = spelled.clone();
                names.push(entry.name.clone());
                let location = folder.join(&entry.written);
                if leads_into(placed, &names, entry) {
                    folders.push((location, names));
                } else {
                    scan.take(file_at(&names, &location, entry));
                }
            }
        }
    }
}

impl Scan {
    /// Keeps what a probe found: a file, or why nothing can be told.
    fn take(&mut self, probe: Probe) {
        match probe {
            Probe::Present(found) => {
                self.files.insert(found.path.key(), found);
            }
            Probe::Unknown(error) => self.errors.push(error),
            Probe::Absent | Probe::Ignored => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ignore_rules::IGNORE_FILE;
    use std::fs;
    use std::path::Path;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn scan(root: &Path, under: Option<&GroupPath>, placed: &[GroupPath]) -> Scan {
        Prober::new(root, placed).scan(under)
    }

    fn paths(scan: &Scan) -> Vec<&str> {
        scan.files.values().map(|file| file.path.as_str()).collect()
    }

    #[test]
    fn finds_hidden_files_and_honours_pigeonignore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".pigeon/members/mario", "{}");
        write(root, "src/+mario/.pigeonignore", "target/\n*.log\n");
        write(root, "src/+mario/main.rs", "fn main() {}");
        write(root, "src/+mario/target/debug/app", "bin");
        write(root, "src/+mario/run.log", "log");
        write(root, "run.log", "not ignored here");
        write(root, ".gitignore", "*.rs\n");
        write(root, "src/.~pigeon-main.rs", "partial");
        let scan = scan(root, None, &[]);
        assert_eq!(
            paths(&scan),
            [
                ".gitignore",
                ".pigeon/members/mario",
                "run.log",
                "src/+mario/.pigeonignore",
                "src/+mario/main.rs",
            ]
        );
        assert!(scan.unportable.is_empty());
        assert!(scan.errors.is_empty());
        assert_eq!(scan.temporaries, [root.join("src/.~pigeon-main.rs")]);
    }

    #[test]
    fn scans_only_under_a_prefix_with_the_rules_above_it() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a/x", "");
        write(dir.path(), "b/y", "");
        write(dir.path(), "b/z.log", "");
        write(dir.path(), "b/build/out", "");
        write(dir.path(), IGNORE_FILE, "*.log\n/b/build\n");
        let under = GroupPath::parse("b").unwrap();
        assert_eq!(paths(&scan(dir.path(), Some(&under), &[])), ["b/y"]);
        let file = GroupPath::parse("B/Y").unwrap();
        assert_eq!(paths(&scan(dir.path(), Some(&file), &[])), ["b/y"]);
        let missing = GroupPath::parse("c").unwrap();
        assert!(scan(dir.path(), Some(&missing), &[]).files.is_empty());
    }

    fn proposals(scan: &Scan) -> Vec<(String, String)> {
        let mut proposals: Vec<(String, String)> = scan
            .unportable
            .iter()
            .map(|unportable| {
                let proposed = unportable.proposed().unwrap().to_string();
                (unportable.path(), proposed)
            })
            .collect();
        proposals.sort();
        proposals
    }

    #[test]
    fn keeps_out_unportable_names_each_with_a_free_portable_name() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "ok.txt", "");
        if cfg!(unix) {
            write(dir.path(), "sub/what?.txt", "");
            write(dir.path(), "sub/what_.txt", "");
            write(dir.path(), "aux/file", "");
            write(dir.path(), "aux/other", "");
        }
        let scan = scan(dir.path(), None, &[]);
        if cfg!(unix) {
            assert_eq!(paths(&scan), ["ok.txt", "sub/what_.txt"]);
            let expected = [("aux", "aux_"), ("sub/what?.txt", "sub/what_ (2).txt")];
            let expected = expected.map(|(path, to)| (path.to_owned(), to.to_owned()));
            assert_eq!(proposals(&scan), expected);
        } else {
            assert_eq!(paths(&scan), ["ok.txt"]);
        }
    }

    #[test]
    fn keeps_the_smallest_spelling_of_names_that_differ_only_by_case() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Readme.md", "a");
        write(dir.path(), "README.md", "b");
        write(dir.path(), "Docs/a", "a");
        write(dir.path(), "docs/b", "b");
        let all = scan(dir.path(), None, &[]);
        if all.unportable.len() == 2 {
            assert_eq!(paths(&all), ["Docs/a", "README.md"]);
            assert!(all.unportable[0].reason.contains("differs only by case"));
            let expected = [("Readme.md", "Readme (2).md"), ("docs", "docs (2)")];
            let expected = expected.map(|(path, to)| (path.to_owned(), to.to_owned()));
            assert_eq!(proposals(&all), expected);
            let file = GroupPath::parse("readme.md").unwrap();
            let under = scan(dir.path(), Some(&file), &[]);
            assert_eq!(paths(&under), ["README.md"]);
            assert_eq!(proposals(&under), expected[..1]);
            assert!(under.unportable.iter().all(|one| one.lies_within(&file)));
            let docs = GroupPath::parse("DOCS").unwrap();
            assert_eq!(
                proposals(&scan(dir.path(), Some(&docs), &[])),
                expected[1..]
            );
            let readme = all.unportable.iter().find(|one| one.name == "Readme.md");
            assert!(!readme.unwrap().lies_within(&docs));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_name_spelled_otherwise_than_in_nfc_is_proposed_in_nfc() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "notes/caf\u{65}\u{301}.txt", "");
        if dir.path().join("notes/caf\u{e9}.txt").exists() {
            return;
        }
        let file = GroupPath::parse("notes/caf\u{e9}.txt").unwrap();
        let scan = scan(dir.path(), Some(&file), &[]);
        assert!(scan.files.is_empty());
        let proposed = [("notes/caf\u{65}\u{301}.txt".to_owned(), file.to_string())];
        assert_eq!(proposals(&scan), proposed);
    }

    #[cfg(unix)]
    #[test]
    fn reports_a_folder_it_cannot_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "open/a", "");
        write(dir.path(), "closed/b", "");
        let closed = dir.path().join("closed");
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&closed).is_ok();
        let scan = scan(dir.path(), None, &[]);
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            assert_eq!(paths(&scan), ["open/a"]);
            assert_eq!(scan.errors.len(), 1, "{:?}", scan.errors);
        }
    }

    #[cfg(unix)]
    #[test]
    fn follows_no_symbolic_link() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "real/file", "");
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real/file"), dir.path().join("flink")).unwrap();
        assert_eq!(paths(&scan(dir.path(), None, &[])), ["real/file"]);
    }

    #[cfg(unix)]
    #[test]
    fn follows_the_links_of_placed_folders_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join(IGNORE_FILE), "*.log\n").unwrap();
        fs::create_dir_all(dir.path().join("disk/videos")).unwrap();
        fs::write(dir.path().join("disk/videos/v.mp4"), "").unwrap();
        fs::write(dir.path().join("disk/videos/v.log"), "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("disk/videos"), root.join("a/videos")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("disk"), root.join("other")).unwrap();
        let placed = [GroupPath::parse("a/videos").unwrap()];
        let paths = |scan: Scan| -> Vec<String> {
            scan.files
                .values()
                .map(|file| file.path.to_string())
                .collect()
        };
        assert_eq!(
            paths(scan(&root, None, &placed)),
            [".pigeonignore", "a/videos/v.mp4"]
        );
        assert_eq!(paths(scan(&root, None, &[])), [".pigeonignore"]);
        let under = GroupPath::parse("a").unwrap();
        assert_eq!(
            paths(scan(&root, Some(&under), &placed)),
            ["a/videos/v.mp4"]
        );
        let inside = GroupPath::parse("a/videos").unwrap();
        assert_eq!(
            paths(scan(&root, Some(&inside), &placed)),
            ["a/videos/v.mp4"]
        );
    }
}
