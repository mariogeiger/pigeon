//! pigeon's engine for one group on one machine: it watches the root,
//! publishes what the machine may write, materializes what the selection
//! holds, sets aside what it may not publish, applies requests, and rebinds
//! names.

mod actions;
mod disk_sync;
pub mod edit;
pub mod engine;
mod protect;
mod publish;
mod rebind;
pub mod reconcile;
mod set_aside;
mod statements;
pub mod views;
pub mod watch;

pub use edit::{Edit, Edited};
pub use engine::{Engine, JoinState, Network, Options};
