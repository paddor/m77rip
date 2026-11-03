//! Hash-table match finders of the greedy light levels.
//!
//! Tables store the low 16 bits of a position. Positions enter the tables
//! `HASHTAB_LAG` bytes late, so every candidate lies at least
//! `MIN_DISTANCE` bytes back and its 32-byte compare stays inside the input.

use fearless_simd::Simd;
use m77rip_core::format::{HASHTAB_LAG, MAX_MATCH_LEN, MIN_DISTANCE};

use crate::read::{HASH_SIZE, array, hash4, hash6, lcp_loaded, load, read_u32, read_u64};

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
    fn entry(&self, hash: usize) -> u16 {
        self.entries[hash]
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

/// The 32 bytes at a probed position and the input before it.
struct Probe<'a> {
    here: &'a [u8; MAX_MATCH_LEN],
    history: &'a [u8],
    /// Latest possible candidate: `MIN_DISTANCE` bytes back.
    latest: usize,
}

impl<'a> Probe<'a> {
    #[inline(always)]
    fn new(src: &'a [u8], pos: usize) -> Option<Self> {
        Some(Self {
            here: array(src, pos),
            history: &src[..pos],
            latest: pos.checked_sub(MIN_DISTANCE)?,
        })
    }

    /// Position of a table entry: the latest position at least
    /// `MIN_DISTANCE` bytes back whose low 16 bits are `entry`. Tables only
    /// hold earlier positions, so it never lies before the input.
    #[inline(always)]
    fn candidate(&self, entry: u16) -> Option<usize> {
        let back = (self.latest as u16).wrapping_sub(entry);
        self.latest.checked_sub(usize::from(back))
    }

    /// The 32 bytes at `candidate`. They end before the probed position,
    /// which the history slice shows the compiler.
    #[inline(always)]
    fn there(&self, candidate: usize) -> &'a [u8; MAX_MATCH_LEN] {
        array(self.history, candidate)
    }

    /// Compare against the candidate of `entry` if their first 4 bytes
    /// match. Returns (length, candidate).
    #[inline(always)]
    fn verify<S: Simd>(&self, simd: S, entry: u16) -> (usize, usize) {
        let Some(candidate) = self.candidate(entry) else {
            return (0, 0);
        };
        let there = self.there(candidate);
        if self.here[..4] != there[..4] {
            return (0, candidate);
        }
        (
            lcp_loaded(load(simd, self.here), load(simd, there)),
            candidate,
        )
    }
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
        let Some(probe) = Probe::new(src, pos) else {
            return (0, 0);
        };
        probe.verify(simd, self.table.entry(hash6(read_u64(probe.here, 0))))
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
        let Some(probe) = Probe::new(src, pos) else {
            return (0, 0);
        };
        let found = probe.verify(simd, self.table6.entry(hash6(read_u64(probe.here, 0))));
        if found.0 != 0 || pending {
            return found;
        }
        probe.verify(simd, self.table4.entry(hash4(read_u32(probe.here, 0))))
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
        let Some(probe) = Probe::new(src, pos) else {
            return (0, 0);
        };
        let reg = load(simd, probe.here);
        let bucket = &self.buckets[hash4(read_u32(probe.here, 0))];
        let mut best = (0, 0);
        for &entry in &bucket.entries {
            let Some(candidate) = probe.candidate(entry) else {
                continue;
            };
            let len = lcp_loaded(reg, load(simd, probe.there(candidate)));
            if len > best.0 {
                best = (len, candidate);
            }
        }
        best
    }
}
