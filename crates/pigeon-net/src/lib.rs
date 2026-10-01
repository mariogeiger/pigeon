//! pigeon's network: binding an endpoint for a group, the sync protocol's
//! messages, the node that keeps a session with every machine of the group
//! and moves blobs among them, each from several machines at once, and the
//! relay a group may run for itself.

pub mod bind;
mod board;
pub mod node;
pub mod relay;
pub mod swarm;
pub mod wire;

pub use node::{Log, Node, Received};
