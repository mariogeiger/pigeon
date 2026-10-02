//! What to do at one path, given how the disk changed since pigeon last
//! looked, which version the disk last matched, which one the selection
//! says it should show, and whether the disk keeps a suggestion of this
//! machine there: the whole disk-to-ledger policy as a pure function.

use pigeon_core::clock::Stamp;

/// How the disk compares with what pigeon last saw at the path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Disk {
    /// The disk still shows what pigeon last saw, or nothing where pigeon
    /// saw nothing.
    Unchanged,
    /// A file appeared or changed.
    Changed,
    /// The file pigeon last saw is gone.
    Removed,
}

/// Whether the disk shows what a suggestion of this machine holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeptSuggestion {
    /// It does not.
    No,
    /// It does, and the suggestion waits for the group.
    Waiting,
    /// It does, and the group decided the suggestion.
    Decided,
}

/// What the path looks like to the ledger and the selection.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct View {
    /// The version the disk last matched.
    pub synced: Option<Stamp>,
    /// The version the disk should show, `None` when the machine holds
    /// nothing at the path.
    pub target: Option<Stamp>,
    /// Whether the disk shows a suggestion of this machine.
    pub kept: KeptSuggestion,
    /// Whether only pigeon writes the path, a statement's.
    pub statement: bool,
}

/// One step toward agreement between disk and ledger.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    /// Publish the disk's change, or suggest it, once it stops changing.
    Settle,
    /// Make the disk show the target.
    Materialize,
}

/// The step that brings the path into agreement, if it needs one.
///
/// The disk keeps what a waiting suggestion of this machine holds, and
/// shows the target again once the group decided it. A change of the disk
/// waits to settle, and publishing decides whether the rules publish it or
/// it becomes a suggestion; a change of a statement is undone.
#[must_use]
pub fn reconcile(disk: Disk, view: &View) -> Option<Step> {
    let moved = view.target != view.synced;
    match view.kept {
        KeptSuggestion::Waiting => None,
        KeptSuggestion::Decided => Some(Step::Materialize),
        KeptSuggestion::No => match disk {
            Disk::Changed | Disk::Removed if view.statement => Some(Step::Materialize),
            Disk::Changed | Disk::Removed => Some(Step::Settle),
            Disk::Unchanged => moved.then_some(Step::Materialize),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    fn stamp(time: u64) -> Stamp {
        Stamp {
            time,
            machine: SecretKey::from_bytes(&[1; 32]).public(),
        }
    }

    fn view(synced: Option<u64>, target: Option<u64>) -> View {
        View {
            synced: synced.map(stamp),
            target: target.map(stamp),
            kept: KeptSuggestion::No,
            statement: false,
        }
    }

    #[test]
    fn an_agreeing_path_needs_nothing() {
        assert_eq!(reconcile(Disk::Unchanged, &view(Some(1), Some(1))), None);
        assert_eq!(reconcile(Disk::Unchanged, &view(None, None)), None);
    }

    #[test]
    fn a_remote_version_lands_on_an_untouched_disk() {
        for target in [Some(2), None] {
            assert_eq!(
                reconcile(Disk::Unchanged, &view(Some(1), target)),
                Some(Step::Materialize)
            );
        }
    }

    #[test]
    fn every_change_of_the_disk_settles_before_anything_else() {
        for disk in [Disk::Changed, Disk::Removed] {
            for (synced, target) in [(None, None), (Some(1), Some(1)), (Some(1), Some(5))] {
                assert_eq!(reconcile(disk, &view(synced, target)), Some(Step::Settle));
            }
        }
    }

    #[test]
    fn a_changed_statement_is_undone() {
        let statement = View {
            statement: true,
            ..view(Some(1), Some(1))
        };
        assert_eq!(
            reconcile(Disk::Changed, &statement),
            Some(Step::Materialize)
        );
        assert_eq!(
            reconcile(Disk::Removed, &statement),
            Some(Step::Materialize)
        );
    }

    #[test]
    fn the_disk_keeps_a_waiting_suggestion_until_the_group_decides() {
        for disk in [Disk::Changed, Disk::Removed, Disk::Unchanged] {
            for (synced, target) in [(Some(1), Some(1)), (Some(1), Some(5))] {
                let waiting = View {
                    kept: KeptSuggestion::Waiting,
                    ..view(synced, target)
                };
                assert_eq!(reconcile(disk, &waiting), None);
                let decided = View {
                    kept: KeptSuggestion::Decided,
                    ..view(synced, target)
                };
                assert_eq!(reconcile(disk, &decided), Some(Step::Materialize));
            }
        }
    }
}
