//! The plan of one blob's fetch from several machines. The blob is cut into
//! at most `MAX_PIECES` pieces of a power-of-two number of chunks, at least
//! 1 MiB each. Every machine offers the chunks it holds, which grow while it
//! downloads. A lane to a machine takes the free piece that machine offers
//! and the fewest machines offer, ties going to the first after a rotation
//! particular to the fetching machine, so that machines fetching together
//! take different pieces from the source and then trade them. Once no free
//! piece fits, a lane that already finished a piece also fetches one that a
//! single other lane is fetching, and the first to finish wins.

use std::collections::{BTreeSet, HashMap};

use bao_tree::ChunkNum;
use iroh_blobs::api::proto::Bitfield;
use iroh_blobs::protocol::ChunkRanges;
use pigeon_core::clock::MachineId;

/// The fewest chunks in a piece: 1 MiB.
const MIN_PIECE: u64 = 1 << 10;
/// The most pieces a blob is cut into.
const MAX_PIECES: u64 = 1 << 10;

/// Where a fetch stands.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Every piece is held.
    Done,
    /// No machine left can deliver what is missing.
    Hopeless,
    /// Pieces are missing, and some machine may still deliver them.
    Waiting,
}

#[derive(Clone, Copy)]
struct Cut {
    chunks: u64,
    piece: u64,
    count: u64,
}

impl Cut {
    fn new(size: u64) -> Self {
        let chunks = ChunkNum::chunks(size).0;
        let piece = chunks
            .div_ceil(MAX_PIECES)
            .next_power_of_two()
            .max(MIN_PIECE);
        Self {
            chunks,
            piece,
            count: chunks.div_ceil(piece),
        }
    }

    fn ranges(self, index: u64) -> ChunkRanges {
        let start = index * self.piece;
        let end = (start + self.piece).min(self.chunks);
        ChunkRanges::from(ChunkNum(start)..ChunkNum(end))
    }
}

/// Which pieces are held, free or being fetched, and what each machine
/// offers: nothing known yet until it answers.
pub(crate) struct Board {
    rotation: u64,
    cut: Option<Cut>,
    held: ChunkRanges,
    offers: HashMap<MachineId, Option<ChunkRanges>>,
    free: BTreeSet<u64>,
    taken: HashMap<u64, usize>,
}

impl Board {
    /// A fetch from `machines` of a blob of which the store holds `held`.
    pub(crate) fn new(
        rotation: u64,
        held: &Bitfield,
        machines: impl IntoIterator<Item = MachineId>,
    ) -> Self {
        let mut board = Self {
            rotation,
            cut: None,
            held: held.ranges.clone(),
            offers: machines
                .into_iter()
                .map(|machine| (machine, None))
                .collect(),
            free: BTreeSet::new(),
            taken: HashMap::new(),
        };
        board.learn(held);
        board
    }

    /// Cuts the blob once some bitfield gives its size.
    fn learn(&mut self, bitfield: &Bitfield) {
        if self.cut.is_some() || bitfield.size() == 0 {
            return;
        }
        let cut = Cut::new(bitfield.size());
        self.free = (0..cut.count)
            .filter(|index| !cut.ranges(*index).is_subset(&self.held))
            .collect();
        self.cut = Some(cut);
    }

    /// Records that `machine` holds `bitfield` too, as it reports first what
    /// it holds and then each addition.
    pub(crate) fn offer(&mut self, machine: MachineId, bitfield: &Bitfield) {
        self.learn(bitfield);
        if let Some(offer) = self.offers.get_mut(&machine) {
            *offer.get_or_insert_default() |= bitfield.ranges.clone();
        }
    }

    /// Forgets `machine`, which can no longer deliver.
    pub(crate) fn lose(&mut self, machine: MachineId) {
        self.offers.remove(&machine);
    }

    /// Whether `machine` answered with what it holds.
    pub(crate) fn knows(&self, machine: MachineId) -> bool {
        self.offers.get(&machine).is_some_and(Option::is_some)
    }

    fn holders(&self, ranges: &ChunkRanges) -> usize {
        self.offers
            .values()
            .flatten()
            .filter(|offer| ranges.is_subset(offer))
            .count()
    }

    /// Takes a piece for a lane to `machine`, with the chunks of it still
    /// missing; a lane that `finished` a piece may duplicate one.
    pub(crate) fn take(
        &mut self,
        machine: MachineId,
        finished: bool,
    ) -> Option<(u64, ChunkRanges)> {
        let cut = self.cut?;
        let offer = self.offers.get(&machine)?.as_ref()?;
        let start = self.rotation % cut.count;
        let order = self.free.range(start..).chain(self.free.range(..start));
        let rarest = order
            .filter(|index| cut.ranges(**index).is_subset(offer))
            .min_by_key(|index| self.holders(&cut.ranges(**index)))
            .copied();
        let index = match rarest {
            Some(index) => {
                self.free.remove(&index);
                index
            }
            None if finished => self
                .taken
                .iter()
                .filter(|(index, lanes)| {
                    let ranges = cut.ranges(**index);
                    **lanes == 1 && ranges.is_subset(offer) && !ranges.is_subset(&self.held)
                })
                .map(|(index, _)| *index)
                .min()?,
            None => return None,
        };
        *self.taken.entry(index).or_default() += 1;
        Some((index, cut.ranges(index).difference(&self.held)))
    }

    /// Records that a lane ended its fetch of piece `index` with the store
    /// holding `held`, and returns whether the piece is held.
    pub(crate) fn finish(&mut self, index: u64, held: &Bitfield) -> bool {
        self.held |= held.ranges.clone();
        let done = self.holds(index);
        if let Some(lanes) = self.taken.get_mut(&index) {
            *lanes -= 1;
            if *lanes == 0 {
                self.taken.remove(&index);
                if !done {
                    self.free.insert(index);
                }
            }
        }
        done
    }

    /// Whether every chunk of piece `index` is held.
    pub(crate) fn holds(&self, index: u64) -> bool {
        self.cut
            .is_some_and(|cut| cut.ranges(index).is_subset(&self.held))
    }

    /// Where the fetch stands: done once every chunk is held, even while
    /// a lane still fetches a piece another lane finished, and hopeless
    /// once no machine is left: a machine holding nothing of the blob yet
    /// may still come to hold it, as the machine between two others does,
    /// for as long as the fetch does not stall.
    pub(crate) fn verdict(&self) -> Verdict {
        let whole = |cut: Cut| ChunkRanges::from(..ChunkNum(cut.chunks)).is_subset(&self.held);
        if self.cut.is_some_and(whole) {
            return Verdict::Done;
        }
        if self.offers.is_empty() {
            return Verdict::Hopeless;
        }
        Verdict::Waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    fn machine(byte: u8) -> MachineId {
        SecretKey::from_bytes(&[byte; 32]).public()
    }

    fn chunks(range: std::ops::Range<u64>) -> ChunkRanges {
        ChunkRanges::from(ChunkNum(range.start)..ChunkNum(range.end))
    }

    const SIZE: u64 = 4 << 20;

    fn partial(range: std::ops::Range<u64>) -> Bitfield {
        Bitfield::new(chunks(range), SIZE)
    }

    #[test]
    fn a_blob_is_cut_into_bounded_aligned_pieces() {
        let small = Cut::new(3 << 20);
        assert_eq!((small.piece, small.count), (1024, 3));
        let huge = Cut::new(100 << 30);
        assert!(huge.count <= MAX_PIECES && huge.piece.is_power_of_two());
        assert_eq!(
            huge.ranges(huge.count - 1),
            chunks((huge.count - 1) * huge.piece..huge.chunks)
        );
    }

    #[test]
    fn a_lane_takes_only_offered_pieces_rarest_first() {
        let (a, b) = (machine(1), machine(2));
        let mut board = Board::new(0, &Bitfield::empty(), [a, b]);
        board.offer(a, &Bitfield::complete(SIZE));
        board.offer(b, &partial(0..2048));
        assert_eq!(board.take(a, false).unwrap().0, 2);
        assert_eq!(board.take(a, false).unwrap().0, 3);
        assert_eq!(board.take(b, false).unwrap().0, 0);
        assert_eq!(board.take(b, false).unwrap().0, 1);
        assert_eq!(board.take(b, false), None);
        board.offer(b, &partial(3072..4096));
        assert_eq!(board.take(b, false), None);
        board.offer(b, &partial(2048..3072));
        assert_eq!(board.take(b, false), None);
        assert!(!board.finish(2, &Bitfield::empty()));
        assert_eq!(board.take(b, false).unwrap().0, 2);
    }

    #[test]
    fn rotations_spread_machines_over_the_pieces() {
        let a = machine(1);
        let firsts: Vec<u64> = (0..4)
            .map(|rotation| {
                let mut board = Board::new(rotation, &Bitfield::empty(), [a]);
                board.offer(a, &Bitfield::complete(SIZE));
                board.take(a, false).unwrap().0
            })
            .collect();
        assert_eq!(firsts, [0, 1, 2, 3]);
    }

    #[test]
    fn only_missing_chunks_are_asked_and_failed_pieces_come_back() {
        let a = machine(1);
        let mut board = Board::new(0, &partial(0..100), [a]);
        board.offer(a, &Bitfield::complete(SIZE));
        assert_eq!(board.take(a, false), Some((0, chunks(100..1024))));
        assert!(!board.finish(0, &partial(0..500)));
        assert_eq!(board.take(a, false), Some((0, chunks(500..1024))));
    }

    #[test]
    fn a_lane_that_finished_duplicates_a_lone_piece_and_the_first_wins() {
        let (a, b) = (machine(1), machine(2));
        let mut board = Board::new(0, &partial(0..3072), [a, b]);
        board.offer(a, &Bitfield::complete(SIZE));
        board.offer(b, &Bitfield::complete(SIZE));
        assert_eq!(board.take(a, false).unwrap().0, 3);
        assert_eq!(board.take(b, false), None);
        assert_eq!(board.take(b, true).unwrap().0, 3);
        assert_eq!(board.take(a, true), None);
        assert!(board.finish(3, &Bitfield::complete(SIZE)));
        assert_eq!(board.verdict(), Verdict::Done);
        assert_eq!(board.take(b, true), None);
    }

    #[test]
    fn a_fetch_is_hopeless_only_once_no_machine_is_left() {
        let (a, b) = (machine(1), machine(2));
        let mut board = Board::new(0, &Bitfield::empty(), [a, b]);
        assert_eq!(board.verdict(), Verdict::Waiting);
        board.offer(a, &Bitfield::empty());
        board.offer(b, &partial(0..10));
        assert_eq!(board.verdict(), Verdict::Waiting);
        board.lose(b);
        assert_eq!(board.verdict(), Verdict::Waiting);
        board.lose(a);
        assert_eq!(board.verdict(), Verdict::Hopeless);
    }
}
