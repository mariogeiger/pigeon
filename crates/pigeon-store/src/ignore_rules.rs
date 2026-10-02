//! The rules of `.pigeonignore` files, in gitignore syntax: each file
//! read once, and whether its rules, the nearest file first, keep a
//! location of the root out of pigeon's sight.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// The file whose gitignore-syntax rules keep files out of publication.
pub const IGNORE_FILE: &str = ".pigeonignore";

/// The ignore files under one root, read as they are needed.
#[derive(Debug)]
pub struct IgnoreRules {
    root: PathBuf,
    /// The rules of each folder read, none where it holds no ignore file.
    folders: HashMap<PathBuf, Option<Gitignore>>,
    /// The lines of ignore files that hold no rule, each with why.
    errors: Vec<String>,
}

impl IgnoreRules {
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            folders: HashMap::new(),
            errors: Vec::new(),
        }
    }

    /// Whether the ignore files of the folders holding `location`, a
    /// folder when `folder`, keep it out: the nearest file with a rule
    /// matching it decides.
    pub fn excludes(&mut self, location: &Path, folder: bool) -> bool {
        let above: Vec<PathBuf> = location
            .ancestors()
            .skip(1)
            .take_while(|above| above.starts_with(&self.root))
            .map(Path::to_path_buf)
            .collect();
        for above in above {
            if let Some(rules) = self.rules(&above) {
                match rules.matched(location, folder) {
                    Match::Ignore(_) => return true,
                    Match::Whitelist(_) => return false,
                    Match::None => {}
                }
            }
        }
        false
    }

    /// Why lines of the ignore files read so far hold no rule.
    #[must_use]
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    fn rules(&mut self, folder: &Path) -> Option<&Gitignore> {
        if !self.folders.contains_key(folder) {
            let rules = self.read(folder);
            self.folders.insert(folder.to_path_buf(), rules);
        }
        self.folders.get(folder).and_then(Option::as_ref)
    }

    fn read(&mut self, folder: &Path) -> Option<Gitignore> {
        let file = folder.join(IGNORE_FILE);
        if !file.is_file() {
            return None;
        }
        let mut builder = GitignoreBuilder::new(folder);
        if let Some(error) = builder.add(&file) {
            self.errors.push(format!("{}: {error}", file.display()));
        }
        match builder.build() {
            Ok(rules) => Some(rules),
            Err(error) => {
                self.errors.push(format!("{}: {error}", file.display()));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_nearest_rule_decides_relative_to_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join(IGNORE_FILE), "*.log\n/build\n").unwrap();
        fs::write(root.join("a").join(IGNORE_FILE), "!keep.log\nbuild\n").unwrap();
        let mut rules = IgnoreRules::new(root);
        assert!(rules.excludes(&root.join("x.log"), false));
        assert!(rules.excludes(&root.join("a/b/x.log"), false));
        assert!(!rules.excludes(&root.join("a/b/keep.log"), false));
        assert!(rules.excludes(&root.join("build"), true));
        assert!(rules.excludes(&root.join("a/b/build"), true));
        assert!(!rules.excludes(&root.join("b/build"), true));
        assert!(!rules.excludes(&root.join("a/b/notes.txt"), false));
        assert!(rules.errors().is_empty());
    }
}
