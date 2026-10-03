//! The changes this machine published that did not last: refused by the
//! ledger, or replaced by another machine that had not seen them. Each
//! becomes a suggestion of this machine once, whatever its disk holds,
//! unless the group's version already shows what it did, or it had not
//! lasted already when the state came to hold its patch: kept by an older
//! pigeon, which suggested what fell by itself, or sent back by a peer
//! after this machine lost its state.

use anyhow::Result;
use pigeon_core::clock::{MachineId, Stamp};
use pigeon_core::ledger::Ledger;
use pigeon_core::patch::{Change, SignedPatch};
use pigeon_core::path::PathKey;
use pigeon_core::statement::{Reason, SuggestedChange, is_statement};
use pigeon_store::state::State;

use crate::engine::{Inner, JoinState, Work};

/// A change of this machine that did not last, the patch that made it,
/// and why it did not.
struct Loss {
    stamp: Stamp,
    key: PathKey,
    change: Change,
    reason: Reason,
}

/// The patches of the machine `me`, in stamp order.
fn patches_of(ledger: &Ledger, me: MachineId) -> impl Iterator<Item = &SignedPatch> {
    ledger
        .patches()
        .filter(move |signed| signed.stamp().machine == me)
}

/// The changes of the patches `made` that did not last, but those of
/// statements, in the order of `made`.
fn losses<'a>(ledger: &Ledger, made: impl IntoIterator<Item = &'a SignedPatch>) -> Vec<Loss> {
    let mut losses = Vec::new();
    for signed in made {
        let stamp = signed.stamp();
        let rejection = match ledger.outcome(&stamp) {
            Some(Err(rejection)) => Some(rejection.to_string()),
            _ => None,
        };
        for change in &signed.patch.changes {
            let key = change.path.key();
            if is_statement(&key) {
                continue;
            }
            let reason = match &rejection {
                Some(rejection) => Reason::Rejected(rejection.clone()),
                None if ledger
                    .unseen_versions(&key)
                    .iter()
                    .any(|version| version.stamp == stamp) =>
                {
                    Reason::Superseded
                }
                None => continue,
            };
            losses.push(Loss {
                stamp,
                key,
                change: change.clone(),
                reason,
            });
        }
    }
    losses
}

/// The changes of the patches `made` that did not last, each as the
/// stamp of its patch and the key it changed.
pub(crate) fn standing_losses<'a>(
    ledger: &Ledger,
    made: impl IntoIterator<Item = &'a SignedPatch>,
) -> Vec<(Stamp, PathKey)> {
    losses(ledger, made)
        .into_iter()
        .map(|loss| (loss.stamp, loss.key))
        .collect()
}

/// Notes the changes of `me` that the stored `ledger` shows did not last,
/// without suggesting them, on a state that never noted losses: none on a
/// fresh state, and those of its time on the state of an older pigeon.
pub(crate) fn note_stored_losses(state: &State, ledger: &Ledger, me: MachineId) -> Result<()> {
    if !state.notes_losses()? {
        let standing = standing_losses(ledger, patches_of(ledger, me));
        state.note_losses(standing.iter().map(|(stamp, key)| (stamp, key)))?;
    }
    Ok(())
}

impl Inner {
    /// Suggests once each change of this machine that did not last, those
    /// of one patch together, unless it is noted or the group's version
    /// shows what it did.
    pub(crate) async fn suggest_losses(&self, work: &mut Work) {
        if work.join != JoinState::Joined {
            return;
        }
        if let Err(error) = self.suggest_new_losses(work).await {
            self.report(format!(
                "suggesting the changes that did not last: {error:#}"
            ));
        }
    }

    async fn suggest_new_losses(&self, work: &mut Work) -> Result<()> {
        let (losses, shown): (Vec<Loss>, Vec<bool>) = {
            let ledger = self.ledger.lock();
            losses(&ledger, patches_of(&ledger, self.me()))
                .into_iter()
                .map(|loss| {
                    let head = ledger.head(&loss.key).and_then(|version| version.content);
                    let shown = head == loss.change.content;
                    (loss, shown)
                })
                .unzip()
        };
        let mut units: Vec<(Vec<&Loss>, Vec<SuggestedChange>)> = Vec::new();
        for (loss, shown) in losses.iter().zip(shown) {
            if self.state.loss_noted(&loss.stamp, &loss.key)? {
                continue;
            }
            if shown {
                self.state.note_losses([(&loss.stamp, &loss.key)])?;
                continue;
            }
            let change = SuggestedChange::from(&loss.change);
            match units.last_mut() {
                Some((unit, changes))
                    if unit[0].stamp == loss.stamp && unit[0].reason == loss.reason =>
                {
                    unit.push(loss);
                    changes.push(change);
                }
                _ => units.push((vec![loss], vec![change])),
            }
        }
        for (unit, changes) in units {
            self.suggest(work, changes, unit[0].reason.clone()).await?;
            self.state
                .note_losses(unit.iter().map(|loss| (&loss.stamp, &loss.key)))?;
        }
        Ok(())
    }
}
