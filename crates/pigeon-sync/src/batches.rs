//! How much one patch or one drafts announcement carries: changes or
//! drafts of at most [`BATCH_BYTES`] encoded bytes, which keeps each well
//! within one message of the protocol whatever the edits that settled at
//! once.

use pigeon_net::wire::MAX_MESSAGE;
use serde::Serialize;

/// The most encoded bytes of changes one patch carries, or of drafts one
/// announcement.
pub(crate) const BATCH_BYTES: usize = MAX_MESSAGE / 64;

const _: () = assert!(2 * BATCH_BYTES <= MAX_MESSAGE, "a batch fits one message");

/// The bytes `value` takes encoded.
pub(crate) fn encoded_size<T: Serialize>(value: &T) -> usize {
    postcard::experimental::serialized_size(value).unwrap_or(usize::MAX)
}

/// Splits `items`, in order, into the fewest consecutive batches whose
/// sizes, as `size` measures them, add up to at most `bound`, an item that
/// alone exceeds it in a batch of its own.
pub(crate) fn batches<T>(
    items: impl IntoIterator<Item = T>,
    size: impl Fn(&T) -> usize,
    bound: usize,
) -> Vec<Vec<T>> {
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    let mut total = 0usize;
    for item in items {
        let bytes = size(&item);
        if !batch.is_empty() && total.saturating_add(bytes) > bound {
            batches.push(std::mem::take(&mut batch));
            total = 0;
        }
        total = total.saturating_add(bytes);
        batch.push(item);
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_split_in_order_into_the_fewest_batches_within_the_bound() {
        let sizes = [3, 4, 2, 9, 1, 1, 5, 12, 2];
        let split = batches(sizes, |size| *size, 9);
        assert_eq!(
            split,
            [vec![3, 4, 2], vec![9], vec![1, 1, 5], vec![12], vec![2]]
        );
        for (batch, next) in split.iter().zip(&split[1..]) {
            let total: usize = batch.iter().sum();
            assert!(total <= 9 || batch.len() == 1);
            assert!(total + next[0] > 9, "a fuller batch fits");
        }
        assert!(batches(Vec::<usize>::new(), |size| *size, 9).is_empty());
    }
}
