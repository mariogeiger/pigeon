//! pigeon's engine for one group on one machine: it watches the root,
//! publishes what the machine may write, materializes what the selection
//! holds, sets aside what it may not publish, and applies requests.

mod actions;
mod disk_sync;
pub mod engine;
mod protect;
mod publish;
pub mod reconcile;
mod set_aside;
mod statements;
pub mod views;
pub mod watch;

pub use engine::{Engine, JoinState, Network, Options};
