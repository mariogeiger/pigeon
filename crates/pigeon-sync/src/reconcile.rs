//! What to do at one path, given how the disk changed since pigeon last
//! looked, which version the disk last matched, and which one the selection
//! says it should show: the whole disk-to-ledger policy as a pure function.

use pigeon_core::clock::Stamp;
use pigeon_core::statement::Reason;

/// How the disk compares with what pigeon last saw at the path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Disk {
    /// The disk still shows what pigeon last saw, or nothing where pigeon
    /// saw nothing.
    Unchanged,
    /// A file appeared or changed; `at` is its modification time in NTP64.
    Changed { at: u64 },
    /// The file pigeon last saw is gone; `at` is when pigeon noticed.
    Removed { at: u64 },
}

/// How a version this machine published fell.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Lost {
    Rejected(String),
    Superseded,
}

/// What the path looks like to the ledger and the selection.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct View {
    /// The version the disk last matched.
    pub synced: Option<Stamp>,
    /// The version the disk should show, `None` when the machine holds
    /// nothing at the path.
    pub target: Option<Stamp>,
    /// Whether this machine may publish a change at the path now.
    pub writable: bool,
    /// Why the synced version, if this machine published it, did not last.
    pub lost: Option<Lost>,
}

/// One step toward agreement between disk and ledger.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    /// Publish the disk's change once it stops changing.
    Settle,
    /// Keep the disk's content in the set-aside list.
    SetAsideDisk(Reason),
    /// Keep the synced version's content in the set-aside list.
    SetAsideSynced(Reason),
    /// Make the disk show the target.
    Materialize,
    /// Stop holding the path: its file was deleted by a machine that may
    /// not delete it for the group.
    Exclude,
}

/// The steps that bring the path into agreement, in order.
///
/// A change this machine may publish waits to settle, unless a version
/// published elsewhere since the disk last matched is later than the
/// change, in which case the later one wins. A change it may not publish is
/// set aside and the target restored. A deletion it may not publish only
/// stops the machine from holding the file.
#[must_use]
pub fn reconcile(disk: Disk, view: &View) -> Vec<Step> {
    let moved = view.target != view.synced;
    match disk {
        Disk::Changed { at } | Disk::Removed { at } if view.writable => {
            match view.target.filter(|_| moved) {
                Some(target) if at <= target.time => {
                    let mut steps = Vec::new();
                    if matches!(disk, Disk::Changed { .. }) {
                        steps.push(Step::SetAsideDisk(Reason::Superseded));
                    }
                    steps.push(Step::Materialize);
                    steps
                }
                _ => vec![Step::Settle],
            }
        }
        Disk::Removed { .. } => vec![Step::Exclude],
        Disk::Changed { .. } => vec![Step::SetAsideDisk(Reason::NotWritable), Step::Materialize],
        Disk::Unchanged if moved => {
            let mut steps = Vec::new();
            match &view.lost {
                Some(Lost::Rejected(why)) => {
                    steps.push(Step::SetAsideSynced(Reason::Rejected(why.clone())));
                }
                Some(Lost::Superseded) => steps.push(Step::SetAsideSynced(Reason::Superseded)),
                None => {}
            }
            steps.push(Step::Materialize);
            steps
        }
        Disk::Unchanged => Vec::new(),
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

    fn view(synced: Option<u64>, target: Option<u64>, writable: bool) -> View {
        View {
            synced: synced.map(stamp),
            target: target.map(stamp),
            writable,
            lost: None,
        }
    }

    #[test]
    fn an_agreeing_path_needs_nothing() {
        assert!(reconcile(Disk::Unchanged, &view(Some(1), Some(1), true)).is_empty());
        assert!(reconcile(Disk::Unchanged, &view(None, None, false)).is_empty());
    }

    #[test]
    fn a_remote_version_lands_on_an_untouched_disk() {
        assert_eq!(
            reconcile(Disk::Unchanged, &view(Some(1), Some(2), false)),
            [Step::Materialize]
        );
        assert_eq!(
            reconcile(Disk::Unchanged, &view(Some(1), None, false)),
            [Step::Materialize]
        );
    }

    #[test]
    fn a_writable_change_settles_before_publication() {
        let at = Disk::Changed { at: 5 };
        assert_eq!(reconcile(at, &view(None, None, true)), [Step::Settle]);
        assert_eq!(reconcile(at, &view(Some(1), Some(1), true)), [Step::Settle]);
        let removed = Disk::Removed { at: 5 };
        assert_eq!(
            reconcile(removed, &view(Some(1), Some(1), true)),
            [Step::Settle]
        );
    }

    #[test]
    fn between_concurrent_changes_of_one_member_the_later_wins() {
        let mine_later = reconcile(Disk::Changed { at: 9 }, &view(Some(1), Some(5), true));
        assert_eq!(mine_later, [Step::Settle]);
        let mine_earlier = reconcile(Disk::Changed { at: 3 }, &view(Some(1), Some(5), true));
        assert_eq!(
            mine_earlier,
            [Step::SetAsideDisk(Reason::Superseded), Step::Materialize]
        );
        let deletion_earlier = reconcile(Disk::Removed { at: 3 }, &view(Some(1), Some(5), true));
        assert_eq!(deletion_earlier, [Step::Materialize]);
    }

    #[test]
    fn an_unwritable_edit_is_set_aside_and_the_version_restored() {
        assert_eq!(
            reconcile(Disk::Changed { at: 9 }, &view(Some(1), Some(1), false)),
            [Step::SetAsideDisk(Reason::NotWritable), Step::Materialize]
        );
    }

    #[test]
    fn an_unwritable_deletion_only_stops_holding_the_file() {
        assert_eq!(
            reconcile(Disk::Removed { at: 9 }, &view(Some(1), Some(1), false)),
            [Step::Exclude]
        );
    }

    #[test]
    fn a_fallen_version_of_this_machine_is_set_aside() {
        let mut rejected = view(Some(1), Some(2), true);
        rejected.lost = Some(Lost::Rejected("claimed".into()));
        assert_eq!(
            reconcile(Disk::Unchanged, &rejected),
            [
                Step::SetAsideSynced(Reason::Rejected("claimed".into())),
                Step::Materialize
            ]
        );
        let mut superseded = view(Some(1), None, true);
        superseded.lost = Some(Lost::Superseded);
        assert_eq!(
            reconcile(Disk::Unchanged, &superseded),
            [Step::SetAsideSynced(Reason::Superseded), Step::Materialize]
        );
    }
}
