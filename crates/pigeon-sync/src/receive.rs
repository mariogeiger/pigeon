//! Taking patches from peers, as the engine and the listener both do: the
//! new patches dated within the clock's drift, and not signed with this
//! machine's key since this run began, are stored all together before the
//! ledger folds them, so that the ledger never holds a patch a
//! restart would lose, and are refused all together when storing fails;
//! the machines to keep sessions with follow from the ledger.

use std::collections::BTreeSet;
use std::time::Duration;

use pigeon_core::clock::{Clock, MachineId, Stamp};
use pigeon_core::ledger::Ledger;
use pigeon_core::patch::SignedPatch;
use pigeon_core::path::PathKey;
use pigeon_net::Received;
use pigeon_net::wire::Patches;
use pigeon_store::group_key::GroupKey;

use crate::engine::SharedLedger;

/// What taking a peer's patches did.
#[derive(Default)]
pub(crate) struct Taken {
    /// The new patches, in stamp order, to pass on.
    pub fresh: Patches,
    /// The paths whose versions were computed anew.
    pub keys: BTreeSet<PathKey>,
    /// Why patches were refused, one line per reason.
    pub refusals: Vec<String>,
    /// Whether the state stored what was taken.
    pub stored: bool,
}

/// Stores with `store` and folds the new patches `received` holds, and
/// observes their times; `first` is the first stamp this machine's clock
/// made in this run, after which every patch this machine signed is in the
/// ledger already, so that another one signed with its key comes from a
/// copy of it.
pub(crate) fn take<E: std::fmt::Display>(
    ledger: &SharedLedger,
    store: impl FnOnce(&[SignedPatch]) -> Result<(), E>,
    clock: &Clock,
    first: &Stamp,
    received: Received,
) -> Taken {
    let from = received.from;
    let mut taken = Taken::default();
    let mut ahead: Vec<Duration> = Vec::new();
    let mut copied = 0;
    let mut ledger = ledger.lock();
    let candidates: Patches = received
        .patches
        .into_iter()
        .filter(|signed| {
            let stamp = signed.stamp();
            if ledger.patch(&stamp).is_some() {
                return false;
            }
            let own = stamp.machine == first.machine && stamp >= *first;
            if own && signed.verify(ledger.group()).is_ok() {
                copied += 1;
                return false;
            }
            clock
                .admits(&stamp)
                .map_err(|lead| ahead.push(lead))
                .is_ok()
        })
        .collect();
    if copied > 0 {
        taken.refusals.push(format!(
            "refused {copied} patches from {from} signed with this machine's key that this machine \
             never made: another machine runs with a copy of its key, such as a copied data folder \
             or a cloned system; that machine must leave the group and join it again"
        ));
    }
    if let Some(lead) = ahead.iter().max() {
        taken.refusals.push(format!(
            "refused {} patches from {from} dated up to {}s ahead of this machine's clock: one of \
             the two clocks is wrong; they come again once it is set right",
            ahead.len(),
            lead.as_secs()
        ));
    }
    let count = candidates.len();
    let inserted = match ledger.insert_all(candidates, store) {
        Ok(inserted) => inserted,
        Err(error) => {
            taken.refusals.push(format!(
                "could not store {count} patches from {from}, so none was taken; they come again: \
                 {error}"
            ));
            return taken;
        }
    };
    taken.stored = true;
    if let Some((_, error)) = inserted.refused.first() {
        taken.refusals.push(format!(
            "refused {} patches from {from} whose signatures do not hold: {error}",
            inserted.refused.len()
        ));
    }
    for stamp in &inserted.added {
        clock.observe(stamp.time);
    }
    taken.keys = inserted
        .folded
        .iter()
        .filter_map(|stamp| ledger.patch(stamp))
        .flat_map(|signed| signed.patch.changes.iter().map(|change| change.path.key()))
        .collect();
    taken.fresh = inserted
        .added
        .iter()
        .filter_map(|stamp| ledger.patch(stamp).cloned())
        .collect();
    taken
}

/// The machines to keep sessions with: every other machine whose patches
/// `ledger` holds or that `key` names.
pub(crate) fn wanted(ledger: &Ledger, key: &GroupKey, me: MachineId) -> BTreeSet<MachineId> {
    ledger
        .digests()
        .into_keys()
        .chain(key.bootstrap.iter().copied())
        .filter(|machine| *machine != me)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use pigeon_core::clock::ntp_time;
    use pigeon_core::test_machines::{change, group, key, machine};

    use super::*;

    fn received(patches: Patches) -> Received {
        Received {
            from: machine("bob", 2).key.public(),
            patches,
        }
    }

    #[test]
    fn new_patches_are_stored_then_folded_and_passed_on() {
        let (mario, bob) = (machine("mario", 1), machine("bob", 2));
        let ledger = SharedLedger::new(Ledger::new(group()));
        let clock = Clock::new(mario.key.public(), Duration::from_secs(300));
        let first = clock.stamp();
        let patches = vec![bob.join(1), bob.patch(2, vec![change("x", Some(1), None)])];
        let mut kept = Vec::new();
        let taken = take(
            &ledger,
            |new: &[SignedPatch]| {
                kept.extend(new.iter().map(SignedPatch::stamp));
                Ok::<(), String>(())
            },
            &clock,
            &first,
            received(patches.clone()),
        );
        assert!(taken.stored && taken.refusals.is_empty());
        assert_eq!(taken.fresh, patches);
        assert_eq!(kept, [bob.stamp(1), bob.stamp(2)]);
        assert!(taken.keys.contains(&key("x")));
        let again = take(
            &ledger,
            |_: &[SignedPatch]| Ok::<(), String>(()),
            &clock,
            &first,
            received(patches),
        );
        assert!(again.fresh.is_empty() && again.refusals.is_empty());
    }

    #[test]
    fn a_batch_that_cannot_be_stored_is_not_taken() {
        let (mario, bob) = (machine("mario", 1), machine("bob", 2));
        let ledger = SharedLedger::new(Ledger::new(group()));
        let clock = Clock::new(mario.key.public(), Duration::from_secs(300));
        let first = clock.stamp();
        let taken = take(
            &ledger,
            |_: &[SignedPatch]| Err("disk full"),
            &clock,
            &first,
            received(vec![bob.join(1), bob.patch(2, vec![])]),
        );
        assert!(!taken.stored && taken.fresh.is_empty());
        assert!(
            taken.refusals[0].contains("could not store 2 patches"),
            "{:?}",
            taken.refusals
        );
        assert!(taken.refusals[0].contains("disk full"));
        assert_eq!(ledger.lock().patches().count(), 0);
    }

    #[test]
    fn patches_too_far_ahead_or_of_a_copy_of_this_machine_are_refused() {
        let (mario, bob) = (machine("mario", 1), machine("bob", 2));
        let ledger = SharedLedger::new(Ledger::new(group()));
        let clock = Clock::new(mario.key.public(), Duration::from_secs(300));
        let first = Stamp {
            time: 10,
            machine: mario.key.public(),
        };
        let ahead = ntp_time(SystemTime::now()) + (3600 << 32);
        let taken = take(
            &ledger,
            |_: &[SignedPatch]| Ok::<(), String>(()),
            &clock,
            &first,
            received(vec![
                mario.join(5),
                mario.patch(20, vec![]),
                bob.join(6),
                bob.patch(ahead, vec![]),
            ]),
        );
        let times: Vec<u64> = taken.fresh.iter().map(|patch| patch.stamp().time).collect();
        assert_eq!(times, [5, 6]);
        assert_eq!(taken.refusals.len(), 2, "{:?}", taken.refusals);
        assert!(taken.refusals[0].contains("copy of its key"));
        assert!(taken.refusals[1].contains("ahead of this machine's clock"));
    }
}
