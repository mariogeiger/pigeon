//! pigeon's engine for one group on one machine: it watches the root,
//! publishes what the rules let the machine publish by itself and suggests
//! the rest, announces its drafts and hears those of other machines,
//! materializes what the selection holds, lists the suggestions and
//! decides them, publishes actions and restorations, follows the group's
//! relay, and keeps placed folders at their destinations; and the listener
//! that hears a group before its machine chooses a member name.

mod actions;
mod disk_sync;
pub mod edit;
pub mod engine;
mod layout;
pub mod listen;
pub mod pending;
mod protect;
mod publish;
mod receive;
pub mod reconcile;
mod relay;
mod selection_change;
mod statements;
pub mod suggestions;
pub mod views;
pub mod watch;

pub use edit::{Edit, Edited};
pub use engine::{Engine, JoinState, Network, Options};
pub use layout::PlaceView;
pub use listen::{Listener, Names};
pub use pending::{PendingView, Rival};
pub use selection_change::{
    Amount, Changed, Delta, DeltaFiles, LISTED, OwnFreed, Preview, RuleEffect,
};
pub use suggestions::{SuggestedChangeView, SuggestionView};
