//! pigeon's network: binding an endpoint for a group, the sync protocol's
//! messages, and the node that keeps a session with every machine of the
//! group and moves blobs among them.

pub mod bind;
pub mod node;
pub mod wire;

pub use node::{Log, Node, Received};
