//! Hash-table match finders of the greedy light levels.
//!
//! Tables store the low 16 bits of a position. Positions enter the tables
//! `HASHTAB_LAG` bytes late, so every candidate lies at least
//! `MIN_DISTANCE` bytes back and its 32-byte compare stays inside the input.

use fearless_simd::Simd;
use m77rip_core::format::{HASHTAB_LAG, MAX_MATCH_LEN};

use crate::read::{
    HASH_SIZE, array, hash4, hash6, lcp_loaded, load, read_u32, read_u64, recover_entry_pos,
};

/// Positions inserted per batch.
const BATCH: usize = 8;
/// Bytes a batch reads: one 8-byte hash input per position.
const BATCH_READ: usize = BATCH + 7;

/// A greedy match finder.
pub(crate) trait Matcher {
    fn new() -> Self;

    /// Insert positions in batches while the latest batch stays
    /// `HASHTAB_LAG` bytes behind `pos`.
    fn insert_until(&mut self, src: &[u8], pos: usize);

    /// Longest match at `pos` as (length, position). `pending` is set while
    /// a deferred candidate waits; some finders probe less then.
    fn find<S: Simd>(&self, simd: S, src: &[u8], pos: usize, pending: bool) -> (usize, usize);
}

/// Table of the latest position per hash.
struct LatestTable {
    entries: Box<[u16; HASH_SIZE]>,
}

impl LatestTable {
    fn new() -> Self {
        Self {
            entries: vec![0; HASH_SIZE].into_boxed_slice().try_into().unwrap(),
        }
    }

    #[inline(always)]
    fn insert(&mut self, hash: usize, pos: usize) {
        self.entries[hash] = pos as u16;
    }

    #[inline(always)]
    fn recover(&self, hash: usize, pos: usize) -> usize {
        recover_entry_pos(self.entries[hash], pos)
    }
}

/// Run `insert` for each position of the batches due before `pos`. It gets
/// the batch's bytes and the position's offset in them.
#[inline(always)]
fn insert_batches(
    src: &[u8],
    next: &mut usize,
    pos: usize,
    mut insert: impl FnMut(&[u8; BATCH_READ], usize, usize),
) {
    while pos >= *next + HASHTAB_LAG + BATCH {
        let bytes = array::<BATCH_READ>(src, *next);
        for i in 0..BATCH {
            insert(bytes, i, *next + i);
        }
        *next += BATCH;
    }
}

/// Compare the 32 bytes at `pos` and `candidate` if their first 4 bytes
/// match. Returns (length, candidate).
#[inline(always)]
fn verify<S: Simd>(
    simd: S,
    here: &[u8; MAX_MATCH_LEN],
    src: &[u8],
    candidate: usize,
) -> (usize, usize) {
    let there = array::<MAX_MATCH_LEN>(src, candidate);
    if here[..4] != there[..4] {
        return (0, candidate);
    }
    (lcp_loaded(load(simd, here), load(simd, there)), candidate)
}

/// Level `-1`: latest position per 6-byte hash.
pub(crate) struct Latest6 {
    table: LatestTable,
    next: usize,
}

impl Matcher for Latest6 {
    fn new() -> Self {
        Self {
            table: LatestTable::new(),
            next: 0,
        }
    }

    #[inline(always)]
    fn insert_until(&mut self, src: &[u8], pos: usize) {
        let table = &mut self.table;
        insert_batches(src, &mut self.next, pos, |bytes, i, at| {
            table.insert(hash6(read_u64(bytes, i)), at);
        });
    }

    #[inline(always)]
    fn find<S: Simd>(&self, simd: S, src: &[u8], pos: usize, _pending: bool) -> (usize, usize) {
        let here = array::<MAX_MATCH_LEN>(src, pos);
        let candidate = self.table.recover(hash6(read_u64(here, 0)), pos);
        verify(simd, here, src, candidate)
    }
}

/// Level `0`: latest position per 6-byte hash, with a 4-byte hash of even
/// positions as fallback while no candidate is pending.
pub(crate) struct Swift {
    table6: LatestTable,
    table4: LatestTable,
    next: usize,
}

impl Matcher for Swift {
    fn new() -> Self {
        Self {
            table6: LatestTable::new(),
            table4: LatestTable::new(),
            next: 0,
        }
    }

    #[inline(always)]
    fn insert_until(&mut self, src: &[u8], pos: usize) {
        let (table6, table4) = (&mut self.table6, &mut self.table4);
        insert_batches(src, &mut self.next, pos, |bytes, i, at| {
            table6.insert(hash6(read_u64(bytes, i)), at);
            if at & 1 == 0 {
                table4.insert(hash4(read_u32(bytes, i)), at);
            }
        });
    }

    #[inline(always)]
    fn find<S: Simd>(&self, simd: S, src: &[u8], pos: usize, pending: bool) -> (usize, usize) {
        let here = array::<MAX_MATCH_LEN>(src, pos);
        let found = verify(
            simd,
            here,
            src,
            self.table6.recover(hash6(read_u64(here, 0)), pos),
        );
        if found.0 != 0 || pending {
            return found;
        }
        verify(
            simd,
            here,
            src,
            self.table4.recover(hash4(read_u32(here, 0)), pos),
        )
    }
}

/// Levels `1` and `2`: the last `WIDTH` positions per 4-byte hash.
pub(crate) struct Ring<const WIDTH: usize> {
    buckets: Box<[RingBucket<WIDTH>]>,
    next: usize,
}

#[derive(Clone, Copy)]
struct RingBucket<const WIDTH: usize> {
    entries: [u16; WIDTH],
    next: u8,
}

/// Ring width of level `1`.
pub(crate) const LOOSE_WIDTH: usize = 16;
/// Ring width of level `2`.
pub(crate) const KEEN_WIDTH: usize = 20;

impl<const WIDTH: usize> Ring<WIDTH> {
    /// The ring cursor is a `u8`.
    const CURSOR_FITS: () = assert!(WIDTH <= u8::MAX as usize + 1);
}

impl<const WIDTH: usize> Matcher for Ring<WIDTH> {
    fn new() -> Self {
        let () = Self::CURSOR_FITS;
        let empty = RingBucket {
            entries: [0; WIDTH],
            next: 0,
        };
        Self {
            buckets: vec![empty; HASH_SIZE].into_boxed_slice(),
            next: 0,
        }
    }

    #[inline(always)]
    fn insert_until(&mut self, src: &[u8], pos: usize) {
        let buckets = &mut self.buckets;
        insert_batches(src, &mut self.next, pos, |bytes, i, at| {
            let bucket = &mut buckets[hash4(read_u32(bytes, i))];
            let slot = usize::from(bucket.next);
            bucket.entries[slot] = at as u16;
            bucket.next = if slot == WIDTH - 1 { 0 } else { slot as u8 + 1 };
        });
    }

    #[inline(always)]
    fn find<S: Simd>(&self, simd: S, src: &[u8], pos: usize, _pending: bool) -> (usize, usize) {
        let here = array::<MAX_MATCH_LEN>(src, pos);
        let reg = load(simd, here);
        let bucket = &self.buckets[hash4(read_u32(here, 0))];
        let mut best = (0, 0);
        for &entry in &bucket.entries {
            let candidate = recover_entry_pos(entry, pos);
            let len = lcp_loaded(reg, load(simd, array(src, candidate)));
            if len > best.0 {
                best = (len, candidate);
            }
        }
        best
    }
}
