//! pigeon's network: binding an endpoint for a group, the sync protocol's
//! messages, and the node that keeps a session with every machine of the
//! group and moves blobs among them, each from several machines at once.

pub mod bind;
mod board;
pub mod node;
pub mod swarm;
pub mod wire;

pub use node::{Log, Node, Received};
