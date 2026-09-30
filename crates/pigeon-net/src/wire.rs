//! The sync protocol's messages and their framing: each message is a
//! postcard value preceded by its length as four big-endian bytes.

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use iroh::endpoint::{RecvStream, SendStream};
use pigeon_core::clock::MachineId;
use pigeon_core::identity::GroupId;
use pigeon_core::patch::SignedPatch;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The protocol's name on the wire.
pub const SYNC_ALPN: &[u8] = b"pigeon/sync/1";

/// The largest message either side accepts.
pub const MAX_MESSAGE: usize = 64 << 20;

/// The latest patch time known from each machine.
pub type Vector = BTreeMap<MachineId, u64>;

/// The first message each side sends: which group it belongs to, a proof
/// that it knows the group secret, and which patches it already holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub group: GroupId,
    pub admission: [u8; 32],
    pub vector: Vector,
}

/// Every later message: patches the other side may lack.
pub type Patches = Vec<SignedPatch>;

/// Writes one message.
///
/// # Errors
///
/// Fails if the stream is closed or the message is too large.
///
/// # Panics
///
/// Panics if a message within the bound has no 32-bit length, which
/// cannot happen.
pub async fn write<T: Serialize>(send: &mut SendStream, message: &T) -> Result<()> {
    let bytes = postcard::to_stdvec(message).context("encoding a message")?;
    ensure!(
        bytes.len() <= MAX_MESSAGE,
        "message of {} bytes",
        bytes.len()
    );
    let length = u32::try_from(bytes.len()).expect("bounded by MAX_MESSAGE");
    send.write_all(&length.to_be_bytes()).await?;
    send.write_all(&bytes).await?;
    Ok(())
}

/// Reads one message.
///
/// # Errors
///
/// Fails if the stream ends, the message is too large, or it does not
/// decode.
pub async fn read<T: DeserializeOwned>(recv: &mut RecvStream) -> Result<T> {
    let mut length = [0; 4];
    recv.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    ensure!(length <= MAX_MESSAGE, "message of {length} bytes");
    let mut bytes = vec![0; length];
    recv.read_exact(&mut bytes).await?;
    postcard::from_bytes(&bytes).context("decoding a message")
}
