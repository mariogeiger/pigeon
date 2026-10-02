//! What to do at one path, given how the disk changed since pigeon last
//! looked, which version the disk last matched, which one the selection
//! says it should show, and whether the disk keeps a suggestion of this
//! machine there: the whole disk-to-ledger policy as a pure function.

use pigeon_core::clock::Stamp;
use pigeon_core::statement::Reason;

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
    /// Why the synced version, if this machine published it, did not
    /// last: a rejection or a concurrent change, the reason its content is
    /// then suggested for.
    pub fell: Option<Reason>,
}

/// One step toward agreement between disk and ledger.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    /// Publish the disk's change, or suggest it, once it stops changing.
    Settle,
    /// Suggest the synced version's content, which the disk keeps.
    SuggestSynced(Reason),
    /// Make the disk show the target.
    Materialize,
}

/// The step that brings the path into agreement, if it needs one.
///
/// The disk keeps what a waiting suggestion of this machine holds, and
/// shows the target again once the group decided it. A change of the disk
/// waits to settle, and publishing decides whether the rules publish it or
/// it becomes a suggestion; a change of a statement is undone. A version
/// this machine published that fell becomes a suggestion, which the disk
/// keeps.
#[must_use]
pub fn reconcile(disk: Disk, view: &View) -> Option<Step> {
    let moved = view.target != view.synced;
    match view.kept {
        KeptSuggestion::Waiting => None,
        KeptSuggestion::Decided => Some(Step::Materialize),
        KeptSuggestion::No => match disk {
            Disk::Changed | Disk::Removed if view.statement => Some(Step::Materialize),
            Disk::Changed | Disk::Removed => Some(Step::Settle),
            Disk::Unchanged if !moved => None,
            Disk::Unchanged => Some(match &view.fell {
                Some(reason) => Step::SuggestSynced(reason.clone()),
                None => Step::Materialize,
            }),
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
            fell: None,
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

    #[test]
    fn a_fallen_version_of_this_machine_becomes_a_suggestion_the_disk_keeps() {
        let rejected = View {
            fell: Some(Reason::Rejected("claimed".into())),
            ..view(Some(1), Some(2))
        };
        assert_eq!(
            reconcile(Disk::Unchanged, &rejected),
            Some(Step::SuggestSynced(Reason::Rejected("claimed".into())))
        );
        let superseded = View {
            fell: Some(Reason::Superseded),
            ..view(Some(1), None)
        };
        assert_eq!(
            reconcile(Disk::Unchanged, &superseded),
            Some(Step::SuggestSynced(Reason::Superseded))
        );
    }
}
