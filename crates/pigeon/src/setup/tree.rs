//! The tree in which `pigeon setup` lets one choose what this machine
//! follows: folders open and close, a box checks a file or all those in a
//! folder, the total shows what following them downloads, and each toggle
//! becomes the follow or unfollow rule the web UI's Files page makes.
//! Files inside the member's own `+name` folders stay followed.

use std::collections::BTreeSet;
use std::fmt::Write;

use anyhow::Result;
use dialoguer::console::{Key, Term};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{exact_pattern, folder_pattern};

use crate::render;

/// A file of the group as the tree shows it.
pub struct File {
    pub path: String,
    pub size: u64,
    pub followed: bool,
    pub held: bool,
    /// Whether it is the member's own, and so always followed.
    pub locked: bool,
}

/// What one chose: the rules the toggles made, in order, a pattern and
/// whether it follows, and what the followed files download.
pub struct Chosen {
    pub toggles: Vec<(String, bool)>,
    pub download: u64,
}

/// A visible line of the tree: a folder or a file.
struct Row {
    path: String,
    depth: usize,
    folder: bool,
}

/// The files, which folders are open, the line under the cursor, and the
/// rules the toggles made, in order: a pattern and whether it follows.
pub struct Tree {
    files: Vec<File>,
    open: BTreeSet<String>,
    cursor: usize,
    changes: Vec<(String, bool)>,
}

impl Tree {
    #[must_use]
    pub fn new(mut files: Vec<File>) -> Self {
        files.sort_by(|a, b| a.path.split('/').cmp(b.path.split('/')));
        Self {
            files,
            open: BTreeSet::new(),
            cursor: 0,
            changes: Vec::new(),
        }
    }

    /// The lines shown: each folder once, and what open folders hold.
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut shown = BTreeSet::new();
        for file in &self.files {
            let parts: Vec<&str> = file.path.split('/').collect();
            for depth in 0..parts.len() {
                let path = parts[..=depth].join("/");
                let folder = depth + 1 < parts.len();
                if !folder || shown.insert(path.clone()) {
                    rows.push(Row {
                        path: path.clone(),
                        depth,
                        folder,
                    });
                }
                if folder && !self.open.contains(&path) {
                    break;
                }
            }
        }
        rows
    }

    /// The files a line stands for: the file, or those in the folder.
    fn under<'a>(&'a self, row: &'a Row) -> impl Iterator<Item = &'a File> {
        self.files
            .iter()
            .filter(move |file| covers(row, &file.path))
    }

    /// The box of a line: every file followed, none, or some.
    fn check(&self, row: &Row) -> &'static str {
        let (followed, all) = self.under(row).fold((0, 0), |(followed, all), file| {
            (followed + usize::from(file.followed), all + 1)
        });
        match followed {
            0 => "[ ]",
            _ if followed == all => "[x]",
            _ => "[-]",
        }
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
    /// if all are followed, the member's own excepted.
    fn toggle(&mut self) {
        let Some(row) = self.rows().into_iter().nth(self.cursor) else {
            return;
        };
        let follow = self.under(&row).any(|file| !file.locked && !file.followed);
        let mut changed = false;
        for file in &mut self.files {
            if covers(&row, &file.path) && !file.locked {
                file.followed = follow;
                changed = true;
            }
        }
        if let (true, Ok(path)) = (changed, GroupPath::parse(&row.path)) {
            let pattern = if row.folder {
                folder_pattern(&path)
            } else {
                exact_pattern(&path)
            };
            self.changes.push((pattern, follow));
        }
    }

    /// Carries out a key: moves, opens or closes a folder, or toggles.
    fn press(&mut self, key: &Key) {
        let rows = self.rows();
        let Some(row) = rows.get(self.cursor) else {
            return;
        };
        match key {
            Key::ArrowUp => self.cursor = self.cursor.saturating_sub(1),
            Key::ArrowDown => self.cursor = (self.cursor + 1).min(rows.len() - 1),
            Key::ArrowRight if row.folder => {
                self.open.insert(row.path.clone());
            }
            Key::ArrowLeft if row.folder && self.open.contains(&row.path) => {
                self.open.remove(&row.path);
            }
            Key::ArrowLeft => {
                if let Some(parent) = rows[..self.cursor]
                    .iter()
                    .rposition(|above| above.folder && covers(above, &row.path))
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
        let rows = self.rows();
        let room = height.saturating_sub(6).max(1);
        let first = self
            .cursor
            .saturating_sub(room / 2)
            .min(rows.len().saturating_sub(room));
        let mut text = String::from(
            "What this machine follows\n↑↓ move · →← open, close · space check · Enter confirm · Esc skip\n\n",
        );
        for (index, row) in rows.iter().enumerate().skip(first).take(room) {
            let pointer = if index == self.cursor { '›' } else { ' ' };
            let arrow = match (row.folder, self.open.contains(&row.path)) {
                (false, _) => ' ',
                (true, true) => '▾',
                (true, false) => '▸',
            };
            let name = row.path.rsplit('/').next().unwrap_or_default();
            let slash = if row.folder { "/" } else { "" };
            let size: u64 = self.under(row).map(|file| file.size).sum();
            let own = if self.under(row).all(|file| file.locked) {
                "  (yours)"
            } else {
                ""
            };
            let _ = writeln!(
                text,
                "{pointer} {}{arrow} {} {name}{slash}  {}{own}",
                "  ".repeat(row.depth),
                self.check(row),
                render::size(size)
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

/// Whether the line `row` stands for the file `path`.
fn covers(row: &Row, path: &str) -> bool {
    path == row.path
        || (row.folder
            && path
                .strip_prefix(&row.path)
                .is_some_and(|rest| rest.starts_with('/')))
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
            locked: path.contains("+mario/"),
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
            tree.rows()
                .into_iter()
                .map(|row| row.path)
                .collect::<Vec<_>>()
        };
        assert_eq!(paths(&tree), ["docs", "readme"]);
        tree.press(&Key::ArrowRight);
        assert_eq!(
            paths(&tree),
            ["docs", "docs/+mario", "docs/b.txt", "docs/old", "readme"]
        );
        tree.press(&Key::ArrowDown);
        tree.press(&Key::ArrowLeft);
        assert_eq!(tree.cursor, 0);
        tree.press(&Key::ArrowLeft);
        assert_eq!(paths(&tree), ["docs", "readme"]);
    }

    #[test]
    fn toggling_a_folder_follows_or_unfollows_all_but_the_members_own() {
        let mut tree = tree();
        let docs = Row {
            path: "docs".into(),
            depth: 0,
            folder: true,
        };
        assert_eq!(tree.check(&docs), "[-]");
        tree.press(&Key::Char(' '));
        assert_eq!(tree.check(&docs), "[x]");
        assert_eq!(tree.to_download(), 320);
        tree.press(&Key::Char(' '));
        assert_eq!(tree.check(&docs), "[-]");
        assert_eq!(tree.to_download(), 0);
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
    fn a_folder_of_the_members_own_files_cannot_be_unfollowed() {
        let mut tree = tree();
        tree.press(&Key::ArrowRight);
        tree.press(&Key::ArrowDown);
        tree.press(&Key::Char(' '));
        assert!(tree.changes.is_empty());
        let drawn = tree.render(40);
        assert!(
            drawn.contains("\n›   ▸ [x] +mario/  5 B  (yours)\n"),
            "{drawn}"
        );
        assert!(drawn.ends_with("\nTo download: 0 B\n"), "{drawn}");
    }
}
