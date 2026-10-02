//! The sync protocol's messages and their framing: each message is a
//! postcard value preceded by its length as four big-endian bytes, and
//! patches go in as many messages as keep each within the bound.

use anyhow::{Context, Result, ensure};
use iroh::endpoint::{RecvStream, SendStream};
use pigeon_core::draft::SignedDrafts;
use pigeon_core::identity::GroupId;
use pigeon_core::ledger::Digests;
use pigeon_core::patch::SignedPatch;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The protocol's name on the wire.
pub const SYNC_ALPN: &[u8] = b"pigeon/sync/10";

/// The largest message either side accepts once the other is admitted.
pub const MAX_MESSAGE: usize = 64 << 20;

/// The largest hello either side reads, before it knows whether the
/// other belongs to the group.
pub const MAX_HELLO: usize = 1 << 20;

/// The first message each side sends: which group it belongs to, a proof
/// that it knows the group secret, and the digest of the patches it holds
/// of each machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub group: GroupId,
    pub admission: [u8; 32],
    pub digests: Digests,
}

/// Patches the other side may lack.
pub type Patches = Vec<SignedPatch>;

/// Every later message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    Patches(Patches),
    /// Every draft the sender's machine holds now, replacing those it
    /// announced before.
    Drafts(Box<SignedDrafts>),
    /// The digests of the patches the sender holds now, from which the
    /// receiver tells what it lacks.
    Holds(Digests),
}

/// At most what `Message::Patches` adds around its patches: the variant
/// and the count, as varints.
const ENVELOPE: usize = 1 + 10;

/// The messages that carry `patches`, in order, each within
/// [`MAX_MESSAGE`] unless a patch alone exceeds it.
#[must_use]
pub fn patch_messages(patches: Patches) -> Vec<Message> {
    split(patches, MAX_MESSAGE)
}

/// Splits `patches`, in order, into the fewest consecutive messages of at
/// most `bound` bytes, a patch that alone exceeds it in a message of its
/// own.
fn split(patches: Patches, bound: usize) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut batch = Vec::new();
    let mut size = ENVELOPE;
    for patch in patches {
        let bytes = postcard::experimental::serialized_size(&patch).unwrap_or(usize::MAX);
        if !batch.is_empty() && size.saturating_add(bytes) > bound {
            messages.push(Message::Patches(std::mem::take(&mut batch)));
            size = ENVELOPE;
        }
        size = size.saturating_add(bytes);
        batch.push(patch);
    }
    if !batch.is_empty() {
        messages.push(Message::Patches(batch));
    }
    messages
}

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

/// Reads one message of at most [`MAX_MESSAGE`] bytes.
///
/// # Errors
///
/// Fails if the stream ends, the message is too large, or it does not
/// decode.
pub async fn read<T: DeserializeOwned>(recv: &mut RecvStream) -> Result<T> {
    read_within(recv, MAX_MESSAGE).await
}

/// Reads one message of at most `bound` bytes.
///
/// # Errors
///
/// Fails if the stream ends, the message is larger than `bound`, or it
/// does not decode.
pub async fn read_within<T: DeserializeOwned>(recv: &mut RecvStream, bound: usize) -> Result<T> {
    let mut length = [0; 4];
    recv.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    ensure!(length <= bound, "message of {length} bytes, over {bound}");
    let mut bytes = vec![0; length];
    recv.read_exact(&mut bytes).await?;
    postcard::from_bytes(&bytes).context("decoding a message")
}

#[cfg(test)]
mod tests {
    use pigeon_core::test_machines::{change, machine};

    use super::*;

    fn size(message: &Message) -> usize {
        postcard::to_stdvec(message).unwrap().len()
    }

    #[test]
    fn patches_split_into_the_fewest_messages_within_the_bound() {
        let mario = machine("mario", 1);
        let patches: Patches = (1..=40)
            .map(|time| {
                let changes = (0..time % 7)
                    .map(|n| change(&format!("+mario/{time}/{n}"), Some(1), None))
                    .collect();
                mario.patch(time, changes)
            })
            .collect();
        let largest = patches
            .iter()
            .map(|patch| size(&Message::Patches(vec![patch.clone()])))
            .max()
            .unwrap();
        let bound = 3 * largest;
        let messages = split(patches.clone(), bound);
        assert!(messages.len() > 3);
        let batches: Vec<Patches> = messages
            .iter()
            .map(|message| match message {
                Message::Patches(batch) => batch.clone(),
                Message::Drafts(_) | Message::Holds(_) => unreachable!(),
            })
            .collect();
        for (message, next) in messages.iter().zip(&batches[1..]) {
            assert!(size(message) <= bound);
            let first = postcard::to_stdvec(&next[0]).unwrap().len();
            assert!(
                size(message) + first + ENVELOPE > bound,
                "a fuller batch fits"
            );
        }
        assert_eq!(batches.concat(), patches);
        assert!(split(Vec::new(), bound).is_empty());
    }

    #[test]
    fn a_patch_over_the_bound_goes_alone() {
        let mario = machine("mario", 1);
        let patches = vec![mario.join(1), mario.join(2), mario.join(3)];
        let messages = split(patches, 8);
        assert_eq!(messages.len(), 3);
    }
}
