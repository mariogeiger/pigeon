//! The tree in which `pigeon setup` lets one choose what this machine
//! follows: folders open and close, a box checks a file or all those in a
//! folder, the total shows what following them downloads, and each toggle
//! becomes the rule the web UI's Files page makes, follow or else pin or
//! free, on the same tree of folders that page draws, the member's own
//! files marked as theirs.

use std::collections::BTreeSet;
use std::fmt::Write;

use anyhow::Result;
use dialoguer::console::{Key, Term};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{exact_pattern, folder_pattern};

use crate::file_tree::{self, Facts, Folder, Followed, Leaf, Row};
use crate::render;

/// A file of the group as the tree shows it.
pub struct File {
    pub path: String,
    pub size: u64,
    pub followed: bool,
    pub held: bool,
    /// Whether the member owns it, as its personal path says.
    pub own: bool,
}

/// What one chose: the rules the toggles made, in order, a pattern and
/// whether it follows, what the followed files download, and how many
/// files this machine holds that one stopped following, and their bytes.
pub struct Chosen {
    pub toggles: Vec<(String, bool)>,
    pub download: u64,
    pub unchecked_here: usize,
    pub unchecked_bytes: u64,
}

/// A visible line of the tree: a folder or a file, what the files it
/// stands for add up to, and whether they are all the member's own.
struct Line {
    path: String,
    depth: usize,
    folder: bool,
    size: u64,
    followed: Option<Followed>,
    own: bool,
}

impl Line {
    /// Whether the line stands for the file `path`.
    fn covers(&self, path: &str) -> bool {
        file_tree::contains(&self.path, path)
    }

    /// Its box: every file followed, none, or some.
    fn check(&self) -> &'static str {
        match self.followed {
            Some(Followed::All) => "[x]",
            Some(Followed::Some) => "[-]",
            Some(Followed::None) | None => "[ ]",
        }
    }
}

impl Leaf for File {
    fn path(&self) -> &str {
        &self.path
    }

    fn facts(&self) -> Facts<'_> {
        Facts {
            size: self.size,
            followed: Some(self.followed),
            ..Facts::default()
        }
    }
}

/// The files, whether each was followed at first, which folders are open,
/// the line under the cursor, and the rules the toggles made, in order: a
/// pattern and whether it follows.
pub struct Tree {
    files: Vec<File>,
    followed_at_first: Vec<bool>,
    open: BTreeSet<String>,
    cursor: usize,
    changes: Vec<(String, bool)>,
}

impl Tree {
    #[must_use]
    pub fn new(files: Vec<File>) -> Self {
        Self {
            followed_at_first: files.iter().map(|file| file.followed).collect(),
            files,
            open: BTreeSet::new(),
            cursor: 0,
            changes: Vec::new(),
        }
    }

    /// The files this machine holds that were followed and no longer are.
    fn unchecked_here(&self) -> impl Iterator<Item = &File> {
        self.files
            .iter()
            .zip(&self.followed_at_first)
            .filter(|(file, was)| **was && !file.followed && file.held)
            .map(|(file, _)| file)
    }

    /// The lines shown: each folder, and what open folders hold.
    fn lines(&self) -> Vec<Line> {
        let tree = Folder::root(&self.files);
        tree.rows()
            .into_iter()
            .filter(|row| file_tree::ancestors(row.path()).all(|folder| self.open.contains(folder)))
            .map(|row| match row {
                Row::Folder { folder, depth } => Line {
                    path: folder.path.clone(),
                    depth,
                    folder: true,
                    size: folder.summary.size,
                    followed: folder.summary.followed(),
                    own: folder.leaves().all(|file| file.own),
                },
                Row::File { leaf, depth } => Line {
                    path: leaf.path.clone(),
                    depth,
                    folder: false,
                    size: leaf.size,
                    followed: Some(if leaf.followed {
                        Followed::All
                    } else {
                        Followed::None
                    }),
                    own: leaf.own,
                },
            })
            .collect()
    }

    /// What following the followed files downloads.
    #[must_use]
    pub fn to_download(&self) -> u64 {
        self.files
            .iter()
            .filter(|file| file.followed && !file.held)
            .map(|file| file.size)
            .sum()
    }

    /// Follows the files of the line under the cursor, or unfollows them
    /// if all are followed.
    fn toggle(&mut self) {
        let Some(line) = self.lines().into_iter().nth(self.cursor) else {
            return;
        };
        let follow = self
            .files
            .iter()
            .any(|file| line.covers(&file.path) && !file.followed);
        for file in &mut self.files {
            if line.covers(&file.path) {
                file.followed = follow;
            }
        }
        if let Ok(path) = GroupPath::parse(&line.path) {
            let pattern = if line.folder {
                folder_pattern(&path)
            } else {
                exact_pattern(&path)
            };
            self.changes.push((pattern, follow));
        }
    }

    /// Carries out a key: moves, opens or closes a folder, or toggles.
    fn press(&mut self, key: &Key) {
        let lines = self.lines();
        let Some(line) = lines.get(self.cursor) else {
            return;
        };
        match key {
            Key::ArrowUp => self.cursor = self.cursor.saturating_sub(1),
            Key::ArrowDown => self.cursor = (self.cursor + 1).min(lines.len() - 1),
            Key::ArrowRight if line.folder => {
                self.open.insert(line.path.clone());
            }
            Key::ArrowLeft if line.folder && self.open.contains(&line.path) => {
                self.open.remove(&line.path);
            }
            Key::ArrowLeft => {
                if let Some(parent) = lines[..self.cursor]
                    .iter()
                    .rposition(|above| above.folder && above.covers(&line.path))
                {
                    self.cursor = parent;
                }
            }
            Key::Char(' ') => self.toggle(),
            _ => {}
        }
    }

    /// The tree as drawn on a terminal `height` lines high.
    fn render(&self, height: usize) -> String {
        let lines = self.lines();
        let room = height.saturating_sub(6).max(1);
        let first = self
            .cursor
            .saturating_sub(room / 2)
            .min(lines.len().saturating_sub(room));
        let mut text = String::from(
            "What this machine follows\n↑↓ move · →← open, close · space check · Enter confirm · Esc skip\n\n",
        );
        for (index, line) in lines.iter().enumerate().skip(first).take(room) {
            let pointer = if index == self.cursor { '›' } else { ' ' };
            let arrow = match (line.folder, self.open.contains(&line.path)) {
                (false, _) => ' ',
                (true, true) => '▾',
                (true, false) => '▸',
            };
            let name = line.path.rsplit('/').next().unwrap_or_default();
            let slash = if line.folder { "/" } else { "" };
            let own = if line.own { "  (yours)" } else { "" };
            let _ = writeln!(
                text,
                "{pointer} {}{arrow} {} {name}{slash}  {}{own}",
                "  ".repeat(line.depth),
                line.check(),
                render::size(line.size)
            );
        }
        text + &format!("\nTo download: {}\n", render::size(self.to_download()))
    }

    /// Lets one choose on `term`; returns nothing when one skips.
    ///
    /// # Errors
    ///
    /// Fails if the terminal cannot be used.
    pub fn choose(mut self, term: &Term) -> Result<Option<Chosen>> {
        term.hide_cursor()?;
        let chosen = loop {
            term.clear_screen()?;
            term.write_str(&self.render(usize::from(term.size().0)))?;
            match term.read_key() {
                Ok(Key::Enter) => {
                    break Ok(Some(Chosen {
                        download: self.to_download(),
                        unchecked_here: self.unchecked_here().count(),
                        unchecked_bytes: self.unchecked_here().map(|file| file.size).sum(),
                        toggles: std::mem::take(&mut self.changes),
                    }));
                }
                Ok(Key::Escape) => break Ok(None),
                Ok(key) => self.press(&key),
                Err(error) => break Err(error.into()),
            }
        };
        term.show_cursor()?;
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64, followed: bool) -> File {
        File {
            path: path.into(),
            size,
            followed,
            held: followed,
            own: path.contains("+mario/"),
        }
    }

    fn tree() -> Tree {
        Tree::new(vec![
            file("docs/b.txt", 20, false),
            file("docs/+mario/a.txt", 5, true),
            file("readme", 1, false),
            file("docs/old/c.txt", 300, false),
        ])
    }

    #[test]
    fn closed_folders_hide_what_they_hold() {
        let mut tree = tree();
        let paths = |tree: &Tree| {
            tree.lines()
                .into_iter()
                .map(|line| line.path)
                .collect::<Vec<_>>()
        };
        assert_eq!(paths(&tree), ["docs", "readme"]);
        tree.press(&Key::ArrowRight);
        assert_eq!(
            paths(&tree),
            ["docs", "docs/+mario", "docs/old", "docs/b.txt", "readme"]
        );
        tree.press(&Key::ArrowDown);
        tree.press(&Key::ArrowLeft);
        assert_eq!(tree.cursor, 0);
        tree.press(&Key::ArrowLeft);
        assert_eq!(paths(&tree), ["docs", "readme"]);
    }

    #[test]
    fn toggling_a_folder_follows_or_unfollows_everything_in_it() {
        let mut tree = tree();
        let docs = |tree: &Tree| tree.lines()[0].check();
        assert_eq!(docs(&tree), "[-]");
        tree.press(&Key::Char(' '));
        assert_eq!(docs(&tree), "[x]");
        assert_eq!(tree.to_download(), 320);
        tree.press(&Key::Char(' '));
        assert_eq!(docs(&tree), "[ ]");
        assert_eq!(tree.to_download(), 0);
        assert_eq!(tree.unchecked_here().count(), 1);
        tree.press(&Key::ArrowDown);
        tree.press(&Key::Char(' '));
        assert_eq!(tree.to_download(), 1);
        assert_eq!(
            tree.changes,
            [
                ("/docs/".to_owned(), true),
                ("/docs/".to_owned(), false),
                ("/readme".to_owned(), true)
            ]
        );
    }

    #[test]
    fn the_members_own_folder_is_marked_and_toggles_like_any_other() {
        let mut tree = tree();
        tree.press(&Key::ArrowRight);
        tree.press(&Key::ArrowDown);
        let drawn = tree.render(40);
        assert!(
            drawn.contains("\n›   ▸ [x] +mario/  5 B  (yours)\n"),
            "{drawn}"
        );
        tree.press(&Key::Char(' '));
        assert_eq!(tree.changes, [("/docs/+mario/".to_owned(), false)]);
        assert!(
            tree.render(40)
                .contains("\n›   ▸ [ ] +mario/  5 B  (yours)\n")
        );
        let unchecked: Vec<&str> = tree
            .unchecked_here()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(unchecked, ["docs/+mario/a.txt"]);
        tree.press(&Key::Char(' '));
        assert_eq!(tree.unchecked_here().count(), 0);
    }
}
