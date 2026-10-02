//! Taking up what pigeon 0.6 left on this machine, once: its read-only
//! files become writable, and each item of its set-aside list becomes a
//! suggestion, which the group decides as any other.

use anyhow::Result;
use pigeon_core::statement::{Reason, SuggestedChange};
use pigeon_store::disk::{self, fs_path};
use pigeon_store::state_v1::ReasonV1;

use crate::engine::{Inner, Work};

/// Why a suggestion waits, from why pigeon 0.6 set its item aside.
fn reason(reason: ReasonV1) -> Reason {
    match reason {
        ReasonV1::NotWritable => Reason::OutsideRules,
        ReasonV1::Unportable(why) => Reason::Unportable(why),
        ReasonV1::Rejected(why) => Reason::Rejected(why),
        ReasonV1::Superseded => Reason::Superseded,
    }
}

impl Inner {
    /// Makes the files pigeon 0.6 froze writable and suggests every item it
    /// set aside, then forgets what it left; nothing when the state never
    /// held pigeon 0.6's tables.
    pub(crate) async fn upgrade_v1(&self, work: &mut Work) -> Result<()> {
        let Some(items) = self.state.left_by_v1()? else {
            return Ok(());
        };
        for entry in self.state.index(None)? {
            if entry.seen.is_some()
                && let Err(error) = disk::unfreeze(&fs_path(&self.root, &entry.path))
            {
                self.report(format!("{}: {error:#}", entry.path));
            }
        }
        for item in items {
            let change = SuggestedChange {
                path: item.path,
                content: item.content,
                replaces: item.replaces,
                continues: None,
            };
            self.suggest(work, vec![change], reason(item.reason))
                .await?;
        }
        Ok(self.state.forget_v1()?)
    }
}
