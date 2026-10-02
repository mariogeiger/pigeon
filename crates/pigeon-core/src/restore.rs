//! Restoring files to a past time: the changes that give every file a
//! pattern matches what its history held then, bringing back what was
//! deleted since and deleting what was created since, while a file that
//! moved since keeps its new path. Statements are never restored.

use std::collections::{BTreeMap, HashSet};

use crate::ledger::{Ledger, Version};
use crate::patch::{Change, VersionRef};
use crate::path::GroupPath;
use crate::statement::is_statement;

/// The changes that bring every file matching `matches` back to its state
/// at `time`, in path order, each replacing the head it changes.
#[must_use]
pub fn restore(ledger: &Ledger, matches: impl Fn(&GroupPath) -> bool, time: u64) -> Vec<Change> {
    let at = |head: &Version| -> Option<VersionRef> {
        std::iter::successors(Some(head), |version| ledger.parent(version))
            .find(|version| version.stamp.time <= time)
            .filter(|version| version.is_live())
            .map(Version::reference)
    };
    let restorable = |path: &GroupPath| matches(path) && !is_statement(&path.key());
    let mut heads: Vec<&Version> = ledger
        .keys()
        .filter_map(|key| ledger.head(key))
        .filter(|head| restorable(&head.path))
        .collect();
    heads.sort_by_key(|head| head.path.key());
    let mut changes = BTreeMap::new();
    for head in heads.iter().filter(|head| head.is_live()) {
        let past = at(head).and_then(|past| ledger.version(&past));
        if past.is_some_and(|past| past.content == head.content) {
            continue;
        }
        changes.insert(
            head.path.key(),
            Change {
                path: head.path.clone(),
                content: past.and_then(|past| past.content),
                replaces: Some(head.stamp),
                continues: None,
            },
        );
    }
    let descended: HashSet<VersionRef> = ledger
        .live()
        .flat_map(|head| ledger.lineage(&head.path.key()))
        .map(Version::reference)
        .collect();
    let mut returning: BTreeMap<VersionRef, &Version> = BTreeMap::new();
    for head in heads.iter().filter(|head| !head.is_live()) {
        let Some(past) = at(head).filter(|past| !descended.contains(past)) else {
            continue;
        };
        let at_home = |dead: &Version| dead.path.key() == past.path.key();
        returning
            .entry(past.clone())
            .and_modify(|chosen| {
                if at_home(head) && !at_home(chosen) {
                    *chosen = head;
                }
            })
            .or_insert(head);
    }
    for (past, dead) in returning {
        let content = ledger.version(&past).and_then(|version| version.content);
        let moved = past.path.key() != dead.path.key();
        changes.insert(
            dead.path.key(),
            Change {
                path: dead.path.clone(),
                content,
                replaces: Some(dead.stamp),
                continues: moved.then_some(past),
            },
        );
    }
    changes.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_machines::*;

    fn restored(ledger: &mut Ledger, by: &Machine, time: u64, at: u64, under: &str) {
        let under = GroupPath::parse(under).unwrap();
        let changes = restore(ledger, |path| path.is_within(&under), at);
        let patch = by.patch(time, changes);
        ledger.insert(patch.clone()).unwrap();
        assert!(ledger.outcome(&patch.stamp()).unwrap().is_ok());
    }

    fn files(ledger: &Ledger) -> Vec<(String, u8)> {
        let mut files: Vec<(String, u8)> = ledger
            .live()
            .filter(|version| !is_statement(&version.path.key()))
            .map(|version| {
                let content = version.content.unwrap().hash.0[0];
                (version.path.as_str().to_owned(), content)
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn a_restore_brings_back_each_file_as_it_was_and_adds_to_history() {
        let mario = machine("mario", 1);
        let bob = machine("bob", 2);
        let mut ledger = Ledger::new(group());
        ledger.insert(mario.join(1)).unwrap();
        ledger.insert(bob.join(2)).unwrap();
        let kept = mario.patch(3, vec![change("docs/kept", Some(1), None)]);
        let edited = mario.patch(4, vec![change("docs/edited", Some(2), None)]);
        let deleted = bob.patch(5, vec![change("docs/deleted", Some(3), None)]);
        let outside = bob.patch(6, vec![change("other/x", Some(4), None)]);
        for patch in [&kept, &edited, &deleted, &outside] {
            ledger.insert(patch.clone()).unwrap();
        }
        for patch in [
            bob.patch(
                10,
                vec![change("docs/edited", Some(5), Some(edited.stamp()))],
            ),
            mario.patch(
                11,
                vec![change("docs/deleted", None, Some(deleted.stamp()))],
            ),
            bob.patch(12, vec![change("docs/created", Some(6), None)]),
            bob.patch(13, vec![change("other/x", None, Some(outside.stamp()))]),
        ] {
            ledger.insert(patch).unwrap();
        }
        let before = ledger.versions(&key("docs/edited")).len();
        restored(&mut ledger, &bob, 20, 7, "docs");
        assert_eq!(
            files(&ledger),
            [
                ("docs/deleted".to_owned(), 3),
                ("docs/edited".to_owned(), 2),
                ("docs/kept".to_owned(), 1),
            ]
        );
        assert_eq!(ledger.versions(&key("docs/edited")).len(), before + 1);
        assert_eq!(ledger.versions(&key("docs/kept")).len(), 1);
        restored(&mut ledger, &mario, 21, 12, "docs");
        assert!(ledger.head(&key("docs/created")).unwrap().is_live());
        assert_eq!(
            ledger.head(&key("docs/edited")).unwrap().content,
            Some(content(5))
        );
    }

    #[test]
    fn a_file_that_moved_since_keeps_its_path_and_takes_its_past_content() {
        let mario = machine("mario", 1);
        let mut ledger = Ledger::new(group());
        ledger.insert(mario.join(1)).unwrap();
        let stays = mario.patch(3, vec![change("docs/a", Some(1), None)]);
        let leaves = mario.patch(4, vec![change("docs/b", Some(2), None)]);
        ledger.insert(stays.clone()).unwrap();
        ledger.insert(leaves.clone()).unwrap();
        let moved_a = mario.patch(10, moved("docs/a", stays.stamp(), "docs/c", 5));
        let moved_b = mario.patch(11, moved("docs/b", leaves.stamp(), "docs/d", 2));
        ledger.insert(moved_a.clone()).unwrap();
        ledger.insert(moved_b.clone()).unwrap();
        ledger
            .insert(mario.patch(12, vec![change("docs/d", None, Some(moved_b.stamp()))]))
            .unwrap();
        restored(&mut ledger, &mario, 20, 7, "docs");
        assert_eq!(
            files(&ledger),
            [("docs/b".to_owned(), 2), ("docs/c".to_owned(), 1)]
        );
        let lineage: Vec<&str> = ledger
            .lineage(&key("docs/c"))
            .iter()
            .map(|version| version.path.as_str())
            .collect();
        assert_eq!(lineage, ["docs/c", "docs/c", "docs/a"]);
    }

    #[test]
    fn statements_are_never_restored() {
        let mario = machine("mario", 1);
        let mut ledger = Ledger::new(group());
        ledger.insert(mario.join(1)).unwrap();
        let suggestion = crate::statement::suggestion_path(&mario.stamp(2));
        let made = mario.patch(2, vec![change(suggestion.as_str(), Some(1), None)]);
        ledger.insert(made.clone()).unwrap();
        ledger
            .insert(mario.patch(
                3,
                vec![change(suggestion.as_str(), None, Some(made.stamp()))],
            ))
            .unwrap();
        assert!(restore(&ledger, |_| true, 2).is_empty());
        assert!(restore(&ledger, |_| true, 0).is_empty());
    }
}
