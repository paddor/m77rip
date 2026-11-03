//! Suffix-array match finder of levels `3` and `4`.
//!
//! For each position of a block, the finder compares the input against its
//! nearest suffix-array neighbors that lie inside the distance window. A
//! three-level bitset of suffix-array ranks tracks which positions are in
//! the window.

use fearless_simd::Simd;
use m77rip_core::format::MIN_MATCH_LEN;

use crate::read::lcp;
use crate::sais::SaisWorkspace;

/// Run `f` with this thread's workspace in `key`, or with a fresh one when
/// it is in use.
#[cfg(feature = "std")]
pub(crate) fn with_workspace<W: Default, R>(
    key: &'static std::thread::LocalKey<core::cell::RefCell<W>>,
    f: impl FnOnce(&mut W) -> R,
) -> R {
    key.with(|cell| match cell.try_borrow_mut() {
        Ok(mut workspace) => f(&mut workspace),
        Err(_) => f(&mut W::default()),
    })
}

/// Match limits of one stream format.
pub(crate) trait Limits {
    const MIN_DISTANCE: usize;
    const MAX_DISTANCE: usize;
    const MAX_MATCH_LEN: usize;
    /// Longest extension measured past a match of `MAX_MATCH_LEN` bytes.
    const EXTEND_LIMIT: usize;
    /// Input past the block end the suffix array also sorts.
    const PAD_LEN: usize;
}

/// A block of positions to find matches for. Matches end by `hard_end`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Block {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) hard_end: usize,
}

impl Block {
    pub(crate) fn len(self) -> usize {
        self.end - self.start
    }
}

/// Buffers of the suffix-array match finder, reused across blocks and calls.
#[derive(Default)]
pub(crate) struct SaWorkspace {
    sorter: SaisWorkspace,
    sa: Vec<i32>,
    rank: Vec<u32>,
    live: LiveSet,
    /// Input position of suffix-array entry 0.
    seg_start: usize,
    /// Longest match per block position; below `MIN_MATCH_LEN` means none.
    pub(crate) max_len: Vec<u8>,
    /// Distance of that match.
    pub(crate) match_dis: Vec<u32>,
}

/// A match the finder carries to the next position: `len` bytes at `dis`.
#[derive(Clone, Copy)]
struct Carry {
    len: usize,
    dis: u32,
}

impl SaWorkspace {
    /// Fill `max_len` and `match_dis` for each position of `block`.
    #[inline(always)]
    pub(crate) fn find_block<S: Simd, L: Limits>(&mut self, simd: S, src: &[u8], block: Block) {
        let seg_start = block.start.saturating_sub(L::MAX_DISTANCE);
        let seg_end = src.len().min(block.end + L::PAD_LEN);
        let seg_len = seg_end - seg_start;
        debug_assert!(i32::try_from(seg_len).is_ok());
        self.sa.resize(seg_len, 0);
        self.rank.resize(seg_len, 0);
        self.sorter
            .suffix_array_with_rank(&src[seg_start..seg_end], &mut self.sa, &mut self.rank);
        self.max_len.resize(block.len(), 0);
        self.match_dis.resize(block.len(), 0);
        let live_end = (block.start + 1)
            .checked_sub(L::MIN_DISTANCE)
            .filter(|&end| end > seg_start)
            .map_or(0, |end| end - seg_start);
        self.live.build(&self.sa, live_end);
        self.seg_start = seg_start;
        self.scan::<S, L>(simd, src, block);
    }

    /// Find the match at each position of `block`. A match that reaches the
    /// length limit carries to the following positions.
    #[inline(always)]
    fn scan<S: Simd, L: Limits>(&mut self, simd: S, src: &[u8], block: Block) {
        let seg_start = self.seg_start;
        let mut carry = Carry { len: 0, dis: 0 };
        let enter_from = block.start.max(seg_start + L::MIN_DISTANCE);
        let leave_from = block.start.max(seg_start + L::MAX_DISTANCE + 1);
        for pos in block.start..block.end {
            let i = pos - block.start;
            if pos >= enter_from {
                let rank = self.rank[pos - L::MIN_DISTANCE - seg_start];
                self.live.set(rank as usize);
            }
            if pos >= leave_from {
                let rank = self.rank[pos - L::MAX_DISTANCE - 1 - seg_start];
                self.live.clear(rank as usize);
            }
            if pos + MIN_MATCH_LEN > block.hard_end {
                self.max_len[i] = 0;
                continue;
            }
            let limit = L::MAX_MATCH_LEN.min(block.hard_end - pos);
            if carry.len >= limit {
                self.max_len[i] = limit as u8;
                self.match_dis[i] = carry.dis;
                carry.len -= 1;
                continue;
            }
            let (len, dis) = self.neighbors(simd, src, pos, limit);
            if len < MIN_MATCH_LEN {
                self.max_len[i] = 0;
                carry.len = carry.len.saturating_sub(1);
                continue;
            }
            self.max_len[i] = len as u8;
            self.match_dis[i] = dis;
            carry = Carry { len: len - 1, dis };
            if len >= limit {
                let from = pos + limit;
                let room = (src.len() - from).min(L::EXTEND_LIMIT);
                carry.len += lcp(simd, src, from, from - dis as usize, room);
            }
        }
    }

    /// Longest match at `pos` among its live suffix-array neighbors, up to
    /// `limit` bytes. Returns (length, distance).
    #[inline(always)]
    fn neighbors<S: Simd>(&self, simd: S, src: &[u8], pos: usize, limit: usize) -> (usize, u32) {
        let seg_start = self.seg_start;
        let rank = self.rank[pos - seg_start] as usize;
        let mut best = (0, 0);
        for neighbor in [self.live.prev(rank), self.live.next(rank)]
            .into_iter()
            .flatten()
        {
            let candidate = seg_start + self.sa[neighbor] as u32 as usize;
            let len = lcp(simd, src, pos, candidate, limit);
            if len > best.0 {
                best = (len, (pos - candidate) as u32);
            }
        }
        best
    }
}

/// Set of suffix-array ranks in three bitset levels: bit `r` of `l0` marks
/// rank `r`, and each bit of `l1` and `l2` marks a nonzero word below.
#[derive(Default)]
struct LiveSet {
    l0: Vec<u64>,
    l1: Vec<u64>,
    l2: Vec<u64>,
}

impl LiveSet {
    /// Size the set for `sa` and mark the ranks of positions below `end`.
    fn build(&mut self, sa: &[i32], end: usize) {
        let len = sa.len();
        reset(&mut self.l0, len.div_ceil(64));
        reset(&mut self.l1, self.l0.len().div_ceil(64));
        reset(&mut self.l2, self.l1.len().div_ceil(64));
        if end == 0 {
            return;
        }
        for (word, ranks) in self.l0.iter_mut().zip(sa.chunks(64)) {
            *word = ranks.iter().enumerate().fold(0, |bits, (bit, &pos)| {
                bits | (u64::from((pos as usize) < end) << bit)
            });
        }
        summarize(&self.l0, &mut self.l1);
        summarize(&self.l1, &mut self.l2);
    }

    #[inline(always)]
    fn set(&mut self, index: usize) {
        let old0 = self.l0[index >> 6];
        self.l0[index >> 6] = old0 | (1 << (index & 63));
        if old0 != 0 {
            return;
        }
        let old1 = self.l1[index >> 12];
        self.l1[index >> 12] = old1 | (1 << ((index >> 6) & 63));
        if old1 != 0 {
            return;
        }
        self.l2[index >> 18] |= 1 << ((index >> 12) & 63);
    }

    #[inline(always)]
    fn clear(&mut self, index: usize) {
        self.l0[index >> 6] &= !(1 << (index & 63));
        if self.l0[index >> 6] != 0 {
            return;
        }
        self.l1[index >> 12] &= !(1 << ((index >> 6) & 63));
        if self.l1[index >> 12] != 0 {
            return;
        }
        self.l2[index >> 18] &= !(1 << ((index >> 12) & 63));
    }

    /// Highest member below `index`.
    #[inline(always)]
    fn prev(&self, index: usize) -> Option<usize> {
        let below = |bit: usize| (1u64 << bit).wrapping_sub(1);
        let bits0 = self.l0[index >> 6] & below(index & 63);
        if bits0 != 0 {
            return Some(((index >> 6) << 6) + high_bit(bits0));
        }
        let word0 = index >> 6;
        let bits1 = self.l1[word0 >> 6] & below(word0 & 63);
        if bits1 != 0 {
            let word0 = ((word0 >> 6) << 6) + high_bit(bits1);
            return Some((word0 << 6) + high_bit(self.l0[word0]));
        }
        let word1 = word0 >> 6;
        let word2 = word1 >> 6;
        let first = self.l2[word2] & below(word1 & 63);
        let (level2, bits2) = core::iter::once((word2, first))
            .chain((0..word2).rev().map(|w| (w, self.l2[w])))
            .find(|&(_, bits)| bits != 0)?;
        let word1 = (level2 << 6) + high_bit(bits2);
        let word0 = (word1 << 6) + high_bit(self.l1[word1]);
        Some((word0 << 6) + high_bit(self.l0[word0]))
    }

    /// Lowest member above `index`.
    #[inline(always)]
    fn next(&self, index: usize) -> Option<usize> {
        let above = |bit: usize| u64::MAX.checked_shl(bit as u32 + 1).unwrap_or(0);
        let bits0 = self.l0[index >> 6] & above(index & 63);
        if bits0 != 0 {
            return Some(((index >> 6) << 6) + low_bit(bits0));
        }
        let word0 = index >> 6;
        let bits1 = self.l1[word0 >> 6] & above(word0 & 63);
        if bits1 != 0 {
            let word0 = ((word0 >> 6) << 6) + low_bit(bits1);
            return Some((word0 << 6) + low_bit(self.l0[word0]));
        }
        let word1 = word0 >> 6;
        let word2 = word1 >> 6;
        let first = self.l2[word2] & above(word1 & 63);
        let (level2, bits2) = core::iter::once((word2, first))
            .chain((word2 + 1..self.l2.len()).map(|w| (w, self.l2[w])))
            .find(|&(_, bits)| bits != 0)?;
        let word1 = (level2 << 6) + low_bit(bits2);
        let word0 = (word1 << 6) + low_bit(self.l1[word1]);
        Some((word0 << 6) + low_bit(self.l0[word0]))
    }
}

fn reset(words: &mut Vec<u64>, len: usize) {
    words.clear();
    words.resize(len, 0);
}

/// Set bit `w` of `upper` for each nonzero word `w` of `lower`.
fn summarize(lower: &[u64], upper: &mut [u64]) {
    for (w, &bits) in lower.iter().enumerate() {
        if bits != 0 {
            upper[w >> 6] |= 1 << (w & 63);
        }
    }
}

#[inline(always)]
fn high_bit(bits: u64) -> usize {
    debug_assert_ne!(bits, 0);
    63 - bits.leading_zeros() as usize
}

#[inline(always)]
fn low_bit(bits: u64) -> usize {
    bits.trailing_zeros() as usize
}
