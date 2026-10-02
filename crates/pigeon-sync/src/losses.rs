//! The changes this machine published that did not last: refused by the
//! ledger, or replaced by another machine that had not seen them. Each
//! becomes a suggestion of this machine once, whatever its disk holds,
//! unless the group's version already shows what it did.

use anyhow::Result;
use pigeon_core::clock::{MachineId, Stamp};
use pigeon_core::ledger::Ledger;
use pigeon_core::patch::Change;
use pigeon_core::path::PathKey;
use pigeon_core::statement::{Reason, SuggestedChange, is_statement};

use crate::engine::{Inner, JoinState, Work};

/// A change of this machine that did not last, the patch that made it,
/// and why it did not.
struct Loss {
    stamp: Stamp,
    key: PathKey,
    change: Change,
    reason: Reason,
}

/// The changes the machine `me` published that did not last, but those
/// of statements, in stamp order.
fn losses(ledger: &Ledger, me: MachineId) -> Vec<Loss> {
    let mut losses = Vec::new();
    for signed in ledger
        .patches()
        .filter(|signed| signed.stamp().machine == me)
    {
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

impl Inner {
    /// Suggests once each change of this machine that did not last, those
    /// of one patch together, unless the group's version shows what it
    /// did. On a state where an older pigeon suggested what fell by
    /// itself, the changes lost by then are noted without being suggested.
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
            losses(&ledger, self.me())
                .into_iter()
                .map(|loss| {
                    let head = ledger.head(&loss.key).and_then(|version| version.content);
                    let shown = head == loss.change.content;
                    (loss, shown)
                })
                .unzip()
        };
        if !self.state.notes_losses()? {
            self.state
                .note_losses(losses.iter().map(|loss| (&loss.stamp, &loss.key)))?;
            return Ok(());
        }
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
