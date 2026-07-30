use crate::sais::SaisWorkspace;
use m77rip_core::Error;
use m77rip_core::format::*;

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
use core::arch::x86_64::{__m256i, _mm256_cmpeq_epi8, _mm256_lddqu_si256, _mm256_movemask_epi8};
#[cfg(not(feature = "paranoid"))]
use core::ptr;
use fearless_simd::{Simd, prelude::*, u8x32};

const HASH_SIZE: usize = 1 << 16;
const HASH_MUL: u32 = 2654435761;
const HASH6_MUL: u64 = 0x9E37_79B1_85EB_CA87;
const HASH6_MASK: u64 = (1u64 << 48) - 1;
const LATEST_BATCH: usize = 8;
const RING_BATCH: usize = 8;
const LOOSE_RING_WIDTH: usize = 16;
const KEEN_RING_WIDTH: usize = 20;
const BLITZ_PROBE_STEP: usize = 2;
const SKIP_SHIFT: usize = 6;
const LIGHT_REGIME_CAP: i32 = 64;
const LIGHT_REGIME_THRESHOLD: i32 = 32;
const LIGHT_OPTIMAL_BLOCK_SIZE: usize = 3 << 17;
const LIGHT_OPTIMAL_PAD_LEN: usize = MAX_MATCH_LEN + 1;
const LIGHT_OPTIMAL_DP_INF: usize = usize::MAX;
const HEAVY_BLOCK_SIZE: usize = 1 << 21;
const HEAVY_PAD_LEN: usize = HEAVY_MAX_MATCH_LEN + 1;
const HEAVY_DP_INF: usize = usize::MAX;
#[cfg(not(feature = "paranoid"))]
const HEAVY_NO_RANK: usize = usize::MAX;
const HEAVY_COND_FLAG_THRESH_NUM: usize = 14;
const HEAVY_COND_FLAG_THRESH_DEN: usize = 100;

const _: () = assert!(LATEST_BATCH == 8);
const _: () = assert!(RING_BATCH == 8);
const _: () = assert!(KEEN_RING_WIDTH <= u8::MAX as usize + 1);
const _: () = assert!(LOOSE_RING_WIDTH <= u8::MAX as usize + 1);
const _: () = assert!(LIGHT_OPTIMAL_BLOCK_SIZE <= u32::MAX as usize);
const _: () = assert!(HEAVY_MIN_DISTANCE > VECTOR_WIDTH);
const _: () = assert!(HEAVY_BLOCK_SIZE <= u32::MAX as usize);
const _: () = assert!(HEAVY_MAX_DISTANCE <= u32::MAX as usize);
const _: () = assert!(HEAVY_MAX_MATCH_LEN <= u8::MAX as usize);

#[inline(always)]
fn hash4(val: u32) -> usize {
    (val.wrapping_mul(HASH_MUL) >> 16) as usize
}

#[inline(always)]
fn hash6(val: u64) -> usize {
    (((val & HASH6_MASK).wrapping_mul(HASH6_MUL)) >> 48) as usize
}

#[inline(always)]
fn recover_entry_pos(entry: u16, pos: usize) -> usize {
    let d = (pos as u16)
        .wrapping_sub(entry)
        .wrapping_sub((HASHTAB_LAG + 1) as u16);
    pos.wrapping_sub(MAX_MATCH_LEN + 1).wrapping_sub(d as usize)
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn read_u32_le(src: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(src[pos..pos + 4].try_into().unwrap())
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn read_u64_le(src: &[u8], pos: usize) -> u64 {
    u64::from_le_bytes(src[pos..pos + 8].try_into().unwrap())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn read_u64_le(src: &[u8], pos: usize) -> u64 {
    debug_assert!(pos + 8 <= src.len());
    // SAFETY: caller guarantees `pos..pos + 8` is inside `src`; unaligned
    // reads are allowed.
    unsafe { ptr::read_unaligned(src.as_ptr().add(pos).cast::<u64>()).to_le() }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn read_u32_le(src: &[u8], pos: usize) -> u32 {
    debug_assert!(pos + 4 <= src.len());
    // SAFETY: caller guarantees `pos..pos + 4` is inside `src`; unaligned
    // reads are allowed.
    unsafe { ptr::read_unaligned(src.as_ptr().add(pos).cast::<u32>()).to_le() }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn load_u8x32_unchecked<S: Simd>(simd: S, src: &[u8], pos: usize) -> u8x32<S> {
    debug_assert!(pos + MAX_MATCH_LEN <= src.len());
    // SAFETY: caller guarantees `pos..pos + 32` is inside `src`. `[u8; 32]`
    // has alignment 1, so this reference is valid at any byte address.
    let lanes = unsafe { &*src.as_ptr().add(pos).cast::<[u8; MAX_MATCH_LEN]>() };
    simd.load_array_ref_u8x32(lanes)
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn load_u8x32<S: Simd>(simd: S, src: &[u8], pos: usize) -> u8x32<S> {
    // SAFETY: forwarded from this function's caller.
    unsafe { load_u8x32_unchecked(simd, src, pos) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn load_u8x32_checked<S: Simd>(simd: S, src: &[u8], pos: usize) -> u8x32<S> {
    let lanes: &[u8; VECTOR_WIDTH] = src[pos..pos + VECTOR_WIDTH].try_into().unwrap();
    simd.load_array_ref_u8x32(lanes)
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn load_u8x32<S: Simd>(simd: S, src: &[u8], pos: usize) -> u8x32<S> {
    load_u8x32_checked(simd, src, pos)
}

#[inline(always)]
fn lcp_loaded<S: Simd>(a: u8x32<S>, b: u8x32<S>) -> usize {
    let eq = a.simd_eq(b).to_bitmask() as u32;
    let diff = !eq;
    if diff != 0 {
        diff.trailing_zeros() as usize
    } else {
        MAX_MATCH_LEN
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn lcp_portable(src: &[u8], a: usize, b: usize, limit: usize) -> usize {
    let mut off = 0;
    while off + 8 <= limit {
        let xa = u64::from_le_bytes(src[a + off..a + off + 8].try_into().unwrap());
        let xb = u64::from_le_bytes(src[b + off..b + off + 8].try_into().unwrap());
        let diff = xa ^ xb;
        if diff != 0 {
            return off + (diff.trailing_zeros() as usize >> 3);
        }
        off += 8;
    }
    while off < limit && src[a + off] == src[b + off] {
        off += 1;
    }
    off
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn lcp_heavy<S: Simd>(simd: S, src: &[u8], a: usize, b: usize, limit: usize) -> usize {
    debug_assert!(a + limit <= src.len());
    debug_assert!(b + limit <= src.len());

    let mut off = 0usize;
    while off + VECTOR_WIDTH <= limit {
        let av = paranoid_unsafe_call!(load_u8x32_unchecked(simd, src, a + off));
        let bv = paranoid_unsafe_call!(load_u8x32_unchecked(simd, src, b + off));
        let len = lcp_loaded(av, bv);
        off += len;
        if len < VECTOR_WIDTH {
            return off;
        }
    }
    while off + 8 <= limit {
        let diff = paranoid_unsafe_call!(read_u64_le(src, a + off))
            ^ paranoid_unsafe_call!(read_u64_le(src, b + off));
        if diff != 0 {
            return off + (diff.trailing_zeros() as usize >> 3);
        }
        off += 8;
    }
    while off < limit && src[a + off] == src[b + off] {
        off += 1;
    }
    off
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn lcp_heavy(src: &[u8], a: usize, b: usize, limit: usize) -> usize {
    lcp_portable(src, a, b, limit)
}

/// Returns the maximum compressed size for a given input size.
pub fn compress_bound(src_size: usize) -> usize {
    compress_bound_light(src_size).max(compress_bound_heavy(src_size))
}

/// Returns the maximum compressed size for a given input size and level.
pub fn compress_bound_level(src_size: usize, level: i8) -> Result<usize, Error> {
    validate_level(level)?;
    Ok(compress_bound_for_level(src_size, level))
}

#[inline]
fn compress_bound_for_level(src_size: usize, level: i8) -> usize {
    if level >= HEAVY_LEVEL_MIN {
        compress_bound_heavy(src_size)
    } else {
        compress_bound_light(src_size)
    }
}

#[inline]
fn compress_bound_light(src_size: usize) -> usize {
    if src_size <= SMALL_LIM {
        return HEADER_SIZE.saturating_add(src_size);
    }
    EXT_HEADER_SIZE
        .saturating_add(src_size)
        .saturating_add(src_size / 255)
        .saturating_add(16)
}

#[inline]
fn compress_bound_heavy(src_size: usize) -> usize {
    EXT_HEADER_SIZE
        .saturating_add(src_size)
        .saturating_add(src_size / 32)
        .saturating_add(64)
}

/// Compresses `input` into the misa77 stream format (level 1, default).
pub fn compress(input: &[u8]) -> Vec<u8> {
    let mut dst = vec![0u8; compress_bound_for_level(input.len(), DEFAULT_LEVEL)];
    let written = compress_dispatch(input, &mut dst, DEFAULT_LEVEL);
    dst.truncate(written);
    dst
}

/// Compresses `input` at the given level (-1..=4). Levels below 4 emit the
/// light format; level 4 emits the heavy format.
///
/// Returns [`Error::InvalidLevel`](m77rip_core::Error::InvalidLevel) for any
/// other level.
pub fn compress_level(input: &[u8], level: i8) -> Result<Vec<u8>, Error> {
    validate_level(level)?;
    let mut dst = vec![0u8; compress_bound_for_level(input.len(), level)];
    let written = compress_dispatch(input, &mut dst, level);
    dst.truncate(written);
    Ok(dst)
}

/// Compresses `input` into `dst` (level 1, default).
///
/// Returns the number of bytes written to `dst`.
pub fn compress_into(input: &[u8], dst: &mut [u8]) -> Result<usize, Error> {
    let bound = compress_bound_for_level(input.len(), DEFAULT_LEVEL);
    if dst.len() < bound {
        return Err(Error::OutputTooSmall {
            need: bound,
            have: dst.len(),
        });
    }
    Ok(compress_dispatch(input, dst, DEFAULT_LEVEL))
}

/// Compresses `input` into `dst` at the given level.
///
/// Returns the number of bytes written to `dst`.
pub fn compress_into_level(input: &[u8], dst: &mut [u8], level: i8) -> Result<usize, Error> {
    validate_level(level)?;
    let bound = compress_bound_for_level(input.len(), level);
    if dst.len() < bound {
        return Err(Error::OutputTooSmall {
            need: bound,
            have: dst.len(),
        });
    }
    Ok(compress_dispatch(input, dst, level))
}

#[inline]
fn validate_level(level: i8) -> Result<(), Error> {
    if (MIN_LEVEL..=MAX_LEVEL).contains(&level) {
        Ok(())
    } else {
        Err(Error::InvalidLevel { level })
    }
}

#[cfg(not(feature = "paranoid"))]
fn compress_dispatch(src: &[u8], dst: &mut [u8], level: i8) -> usize {
    match level {
        -1 => compress_dispatch_level_blitz(src, dst),
        0 => compress_dispatch_level_swift(src, dst),
        1 => compress_dispatch_level_loose(src, dst),
        2 => compress_dispatch_level_keen(src, dst),
        3 => compress_dispatch_level_light_optimal(src, dst),
        4 => compress_dispatch_level_heavy(src, dst),
        _ => unreachable!(),
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(never)]
fn compress_dispatch_level_blitz(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => blitz_compress(simd, src, dst))
}

#[cfg(not(feature = "paranoid"))]
#[inline(never)]
fn compress_dispatch_level_swift(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => swift_compress(simd, src, dst))
}

#[cfg(not(feature = "paranoid"))]
#[inline(never)]
fn compress_dispatch_level_loose(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => loose_compress(simd, src, dst))
}

#[cfg(not(feature = "paranoid"))]
#[inline(never)]
fn compress_dispatch_level_keen(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => keen_compress(simd, src, dst))
}

#[cfg(not(feature = "paranoid"))]
#[inline(never)]
fn compress_dispatch_level_light_optimal(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => light_optimal_compress(simd, src, dst))
}

#[cfg(not(feature = "paranoid"))]
#[inline(never)]
fn compress_dispatch_level_heavy(src: &[u8], dst: &mut [u8]) -> usize {
    #[cfg(all(feature = "std", target_arch = "x86_64"))]
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: Runtime feature check guarantees AVX2. Heavy match-finder
        // callers keep all vector reads inside `src`.
        return unsafe { heavy_compress_avx2(src, dst) };
    }

    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => heavy_compress(simd, src, dst))
}

#[cfg(feature = "paranoid")]
fn compress_dispatch(src: &[u8], dst: &mut [u8], level: i8) -> usize {
    match level {
        -1 => compress_dispatch_level_blitz(src, dst),
        0 => compress_dispatch_level_swift(src, dst),
        1 => compress_dispatch_level_loose(src, dst),
        2 => compress_dispatch_level_keen(src, dst),
        3 => compress_dispatch_level_light_optimal(src, dst),
        4 => heavy_compress(src, dst),
        _ => unreachable!(),
    }
}

#[cfg(feature = "paranoid")]
#[inline(never)]
fn compress_dispatch_level_blitz(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => blitz_compress(simd, src, dst))
}

#[cfg(feature = "paranoid")]
#[inline(never)]
fn compress_dispatch_level_swift(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => swift_compress(simd, src, dst))
}

#[cfg(feature = "paranoid")]
#[inline(never)]
fn compress_dispatch_level_loose(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => loose_compress(simd, src, dst))
}

#[cfg(feature = "paranoid")]
#[inline(never)]
fn compress_dispatch_level_keen(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => keen_compress(simd, src, dst))
}

#[cfg(feature = "paranoid")]
#[inline(never)]
fn compress_dispatch_level_light_optimal(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => light_optimal_compress(simd, src, dst))
}

struct LatestHashTable {
    entries: [u16; HASH_SIZE],
}

impl LatestHashTable {
    fn new() -> Self {
        Self {
            entries: [0u16; HASH_SIZE],
        }
    }

    #[inline(always)]
    fn insert(&mut self, hsh: usize, pos: usize) {
        self.entries[hsh] = pos as u16;
    }

    #[inline(always)]
    fn recover_pos(&self, hsh: usize, pos: usize) -> usize {
        recover_entry_pos(self.entries[hsh], pos)
    }
}

#[derive(Clone, Copy)]
struct RingBucket<const WIDTH: usize> {
    entries: [u16; WIDTH],
    next: u8,
}

impl<const WIDTH: usize> RingBucket<WIDTH> {
    fn new() -> Self {
        Self {
            entries: [0u16; WIDTH],
            next: 0,
        }
    }
}

struct RingHashTable<const WIDTH: usize> {
    buckets: Box<[RingBucket<WIDTH>]>,
}

impl<const WIDTH: usize> RingHashTable<WIDTH> {
    fn new() -> Self {
        Self {
            buckets: vec![RingBucket::<WIDTH>::new(); HASH_SIZE].into_boxed_slice(),
        }
    }

    #[inline(always)]
    fn insert(&mut self, hsh: usize, pos: usize) {
        let bucket = &mut self.buckets[hsh];
        let next = bucket.next as usize;
        bucket.entries[next] = pos as u16;
        bucket.next = if next == WIDTH - 1 {
            0
        } else {
            (next + 1) as u8
        };
    }

    #[inline(always)]
    fn bucket(&self, hsh: usize) -> &RingBucket<WIDTH> {
        &self.buckets[hsh]
    }
}

#[derive(Default)]
struct HeavyLiveSet {
    l0: Vec<u64>,
    l1: Vec<u64>,
    l2: Vec<u64>,
}

impl HeavyLiveSet {
    fn build(&mut self, sa: &[i32], len: usize, limit: u32) {
        self.l0.clear();
        self.l0.resize(len.div_ceil(64), 0);
        self.l1.clear();
        self.l1.resize(self.l0.len().div_ceil(64), 0);
        self.l2.clear();
        self.l2.resize(self.l1.len().div_ceil(64), 0);

        if limit == 0 {
            return;
        }

        for word in 0..self.l0.len() {
            let base = word << 6;
            let top = 64.min(len - base);
            let mut bits = 0u64;
            for bit in 0..top {
                bits |= u64::from((sa[base + bit] as u32) < limit) << bit;
            }
            self.l0[word] = bits;
        }
        for word in 0..self.l0.len() {
            if self.l0[word] != 0 {
                self.l1[word >> 6] |= 1u64 << (word & 63);
            }
        }
        for word in 0..self.l1.len() {
            if self.l1[word] != 0 {
                self.l2[word >> 6] |= 1u64 << (word & 63);
            }
        }
    }

    #[inline(always)]
    fn set(&mut self, index: usize) {
        let word0 = index >> 6;
        let old0 = self.l0[word0];
        self.l0[word0] = old0 | (1u64 << (index & 63));
        if old0 != 0 {
            return;
        }

        let word1 = index >> 12;
        let old1 = self.l1[word1];
        self.l1[word1] = old1 | (1u64 << ((index >> 6) & 63));
        if old1 != 0 {
            return;
        }

        self.l2[index >> 18] |= 1u64 << ((index >> 12) & 63);
    }

    #[inline(always)]
    fn clear(&mut self, index: usize) {
        let word0 = index >> 6;
        self.l0[word0] &= !(1u64 << (index & 63));
        if self.l0[word0] != 0 {
            return;
        }

        let word1 = index >> 12;
        self.l1[word1] &= !(1u64 << ((index >> 6) & 63));
        if self.l1[word1] != 0 {
            return;
        }

        self.l2[index >> 18] &= !(1u64 << ((index >> 12) & 63));
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    unsafe fn set_unchecked(&mut self, index: usize) {
        debug_assert!(index < self.l0.len() * 64);
        let word0 = index >> 6;
        // SAFETY: caller supplies a suffix-array rank within the live-set
        // universe; derived words are therefore inside all levels.
        unsafe {
            let l0 = self.l0.as_mut_ptr();
            let word0_ptr = l0.add(word0);
            let old0 = *word0_ptr;
            *word0_ptr = old0 | (1u64 << (index & 63));
            if old0 != 0 {
                return;
            }

            let word1 = index >> 12;
            let l1 = self.l1.as_mut_ptr();
            let word1_ptr = l1.add(word1);
            let old1 = *word1_ptr;
            *word1_ptr = old1 | (1u64 << ((index >> 6) & 63));
            if old1 != 0 {
                return;
            }

            *self.l2.as_mut_ptr().add(index >> 18) |= 1u64 << ((index >> 12) & 63);
        }
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    unsafe fn clear_unchecked(&mut self, index: usize) {
        debug_assert!(index < self.l0.len() * 64);
        let word0 = index >> 6;
        let word1 = index >> 12;
        let word2 = index >> 18;
        // SAFETY: same rank-in-universe contract as `set_unchecked`.
        unsafe {
            let l0 = self.l0.as_mut_ptr();
            let word0_ptr = l0.add(word0);
            *word0_ptr &= !(1u64 << (index & 63));
            if *word0_ptr != 0 {
                return;
            }

            let l1 = self.l1.as_mut_ptr();
            let word1_ptr = l1.add(word1);
            *word1_ptr &= !(1u64 << ((index >> 6) & 63));
            if *word1_ptr != 0 {
                return;
            }

            *self.l2.as_mut_ptr().add(word2) &= !(1u64 << ((index >> 12) & 63));
        }
    }

    #[inline(always)]
    fn prev(&self, index: usize) -> Option<usize> {
        let mut word0 = index >> 6;
        let bit0 = index & 63;
        let mask0 = if bit0 == 0 { 0 } else { (1u64 << bit0) - 1 };
        let bits0 = self.l0[word0] & mask0;
        if bits0 != 0 {
            return Some((word0 << 6) + high_bit_index(bits0));
        }

        let word1 = word0 >> 6;
        let bit1 = word0 & 63;
        let mask1 = if bit1 == 0 { 0 } else { (1u64 << bit1) - 1 };
        let bits1 = self.l1[word1] & mask1;
        if bits1 != 0 {
            word0 = (word1 << 6) + high_bit_index(bits1);
            let bits = self.l0[word0];
            return Some((word0 << 6) + high_bit_index(bits));
        }

        let word2 = word1 >> 6;
        let bit2 = word1 & 63;
        let mask2 = if bit2 == 0 { 0 } else { (1u64 << bit2) - 1 };
        for level2 in (0..=word2).rev() {
            let bits2 = self.l2[level2] & if level2 == word2 { mask2 } else { u64::MAX };
            if bits2 == 0 {
                continue;
            }
            let next1 = (level2 << 6) + high_bit_index(bits2);
            let bits1 = self.l1[next1];
            let next0 = (next1 << 6) + high_bit_index(bits1);
            let bits0 = self.l0[next0];
            return Some((next0 << 6) + high_bit_index(bits0));
        }
        None
    }

    #[inline(always)]
    fn next(&self, index: usize) -> Option<usize> {
        let mut word0 = index >> 6;
        let bit0 = index & 63;
        let mask0 = if bit0 == 63 {
            0
        } else {
            u64::MAX << (bit0 + 1)
        };
        let bits0 = self.l0[word0] & mask0;
        if bits0 != 0 {
            return Some((word0 << 6) + bits0.trailing_zeros() as usize);
        }

        let word1 = word0 >> 6;
        let bit1 = word0 & 63;
        let mask1 = if bit1 == 63 {
            0
        } else {
            u64::MAX << (bit1 + 1)
        };
        let bits1 = self.l1[word1] & mask1;
        if bits1 != 0 {
            word0 = (word1 << 6) + bits1.trailing_zeros() as usize;
            return Some((word0 << 6) + self.l0[word0].trailing_zeros() as usize);
        }

        let word2 = word1 >> 6;
        let bit2 = word1 & 63;
        let mask2 = if bit2 == 63 {
            0
        } else {
            u64::MAX << (bit2 + 1)
        };
        for level2 in word2..self.l2.len() {
            let bits2 = self.l2[level2] & if level2 == word2 { mask2 } else { u64::MAX };
            if bits2 == 0 {
                continue;
            }
            let next1 = (level2 << 6) + bits2.trailing_zeros() as usize;
            let next0 = (next1 << 6) + self.l1[next1].trailing_zeros() as usize;
            return Some((next0 << 6) + self.l0[next0].trailing_zeros() as usize);
        }
        None
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    unsafe fn prev_unchecked(&self, index: usize) -> usize {
        debug_assert!(index < self.l0.len() * 64);
        let mut word0 = index >> 6;
        let bit0 = index & 63;
        let mask0 = if bit0 == 0 { 0 } else { (1u64 << bit0) - 1 };
        // SAFETY: caller supplies a rank in this live set; all traversed
        // summary bits were created from valid lower-level words.
        unsafe {
            let l0 = self.l0.as_ptr();
            let l1 = self.l1.as_ptr();
            let l2 = self.l2.as_ptr();

            let bits0 = *l0.add(word0) & mask0;
            if bits0 != 0 {
                return (word0 << 6) + high_bit_index(bits0);
            }

            let word1 = word0 >> 6;
            let bit1 = word0 & 63;
            let mask1 = if bit1 == 0 { 0 } else { (1u64 << bit1) - 1 };
            let bits1 = *l1.add(word1) & mask1;
            if bits1 != 0 {
                word0 = (word1 << 6) + high_bit_index(bits1);
                let bits = *l0.add(word0);
                return (word0 << 6) + high_bit_index(bits);
            }

            let word2 = word1 >> 6;
            let bit2 = word1 & 63;
            let mask2 = if bit2 == 0 { 0 } else { (1u64 << bit2) - 1 };
            let mut level2 = word2 + 1;
            while level2 != 0 {
                level2 -= 1;
                let bits2 = *l2.add(level2) & if level2 == word2 { mask2 } else { u64::MAX };
                if bits2 == 0 {
                    continue;
                }
                let next1 = (level2 << 6) + high_bit_index(bits2);
                let bits1 = *l1.add(next1);
                let next0 = (next1 << 6) + high_bit_index(bits1);
                let bits0 = *l0.add(next0);
                return (next0 << 6) + high_bit_index(bits0);
            }
        }
        HEAVY_NO_RANK
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    unsafe fn next_unchecked(&self, index: usize) -> usize {
        debug_assert!(index < self.l0.len() * 64);
        let mut word0 = index >> 6;
        let bit0 = index & 63;
        let mask0 = if bit0 == 63 {
            0
        } else {
            u64::MAX << (bit0 + 1)
        };
        // SAFETY: same rank-in-universe contract as `prev_unchecked`.
        unsafe {
            let l0 = self.l0.as_ptr();
            let l1 = self.l1.as_ptr();
            let l2 = self.l2.as_ptr();

            let bits0 = *l0.add(word0) & mask0;
            if bits0 != 0 {
                return (word0 << 6) + bits0.trailing_zeros() as usize;
            }

            let word1 = word0 >> 6;
            let bit1 = word0 & 63;
            let mask1 = if bit1 == 63 {
                0
            } else {
                u64::MAX << (bit1 + 1)
            };
            let bits1 = *l1.add(word1) & mask1;
            if bits1 != 0 {
                word0 = (word1 << 6) + bits1.trailing_zeros() as usize;
                return (word0 << 6) + (*l0.add(word0)).trailing_zeros() as usize;
            }

            let word2 = word1 >> 6;
            let bit2 = word1 & 63;
            let mask2 = if bit2 == 63 {
                0
            } else {
                u64::MAX << (bit2 + 1)
            };
            let l2_len = self.l2.len();
            let mut level2 = word2;
            while level2 < l2_len {
                let bits2 = *l2.add(level2) & if level2 == word2 { mask2 } else { u64::MAX };
                if bits2 == 0 {
                    level2 += 1;
                    continue;
                }
                let next1 = (level2 << 6) + bits2.trailing_zeros() as usize;
                let next0 = (next1 << 6) + (*l1.add(next1)).trailing_zeros() as usize;
                return (next0 << 6) + (*l0.add(next0)).trailing_zeros() as usize;
            }
        }
        HEAVY_NO_RANK
    }
}

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[inline(always)]
fn high_bit_index(bits: u64) -> usize {
    debug_assert_ne!(bits, 0);
    let out: usize;
    // SAFETY: callers pass nonzero words after checking the live-set summary.
    unsafe {
        core::arch::asm!(
            "bsr {out}, {bits}",
            out = lateout(reg) out,
            bits = in(reg) bits,
            options(pure, nomem, nostack)
        );
    }
    out
}

#[cfg(any(feature = "paranoid", not(target_arch = "x86_64")))]
#[inline(always)]
fn high_bit_index(bits: u64) -> usize {
    debug_assert_ne!(bits, 0);
    bits.leading_zeros() as usize ^ 63
}

#[inline(always)]
fn heavy_literal_extras(run: usize) -> usize {
    if run < HEAVY_TOKEN_LIT_MAX {
        0
    } else {
        1 + (run - HEAVY_TOKEN_LIT_MAX) / 255
    }
}

#[inline(always)]
fn find_latest_match6<S: Simd>(
    simd: S,
    src: &[u8],
    ht: &LatestHashTable,
    pos: usize,
) -> (usize, usize) {
    debug_assert!(pos > HASHTAB_LAG);
    debug_assert!(pos + MAX_MATCH_LEN <= src.len());
    let cur = paranoid_unsafe_call!(read_u32_le(src, pos));
    let hsh = hash6(paranoid_unsafe_call!(read_u64_le(src, pos)));
    let lst = ht.recover_pos(hsh, pos);
    debug_assert!(lst + MAX_MATCH_LEN <= src.len());

    if paranoid_unsafe_call!(read_u32_le(src, lst)) != cur {
        return (0, lst);
    }

    let reg = paranoid_unsafe_call!(load_u8x32(simd, src, pos));
    let ireg = paranoid_unsafe_call!(load_u8x32(simd, src, lst));
    (lcp_loaded(reg, ireg), lst)
}

#[inline(always)]
fn find_swift_match<S: Simd>(
    simd: S,
    src: &[u8],
    ht6: &LatestHashTable,
    ht4: &LatestHashTable,
    pos: usize,
    cand_len: usize,
) -> (usize, usize) {
    debug_assert!(pos > HASHTAB_LAG);
    debug_assert!(pos + MAX_MATCH_LEN <= src.len());
    let cur = paranoid_unsafe_call!(read_u32_le(src, pos));
    let mut lst = ht6.recover_pos(hash6(paranoid_unsafe_call!(read_u64_le(src, pos))), pos);
    let mut hit = paranoid_unsafe_call!(read_u32_le(src, lst)) == cur;

    if !hit && cand_len == 0 {
        lst = ht4.recover_pos(hash4(cur), pos);
        hit = paranoid_unsafe_call!(read_u32_le(src, lst)) == cur;
    }

    if !hit {
        return (0, lst);
    }

    let reg = paranoid_unsafe_call!(load_u8x32(simd, src, pos));
    let ireg = paranoid_unsafe_call!(load_u8x32(simd, src, lst));
    (lcp_loaded(reg, ireg), lst)
}

#[inline(always)]
fn find_ring_match<S: Simd, const WIDTH: usize>(
    simd: S,
    src: &[u8],
    ht: &RingHashTable<WIDTH>,
    pos: usize,
) -> (usize, usize) {
    debug_assert!(pos > HASHTAB_LAG);
    debug_assert!(pos + MAX_MATCH_LEN <= src.len());

    let hsh = hash4(paranoid_unsafe_call!(read_u32_le(src, pos)));
    let reg = paranoid_unsafe_call!(load_u8x32(simd, src, pos));
    let bucket = ht.bucket(hsh);
    let mut lst = 0;
    let mut match_len = 0;

    for &entry in &bucket.entries {
        let ilst = recover_entry_pos(entry, pos);
        let ireg = paranoid_unsafe_call!(load_u8x32(simd, src, ilst));
        let imatch_len = lcp_loaded(reg, ireg);
        if imatch_len > match_len {
            lst = ilst;
            match_len = imatch_len;
        }
    }

    (match_len, lst)
}

#[inline(always)]
fn batch_insert_latest6(src: &[u8], ht: &mut LatestHashTable, hpos: &mut usize, pos: usize) {
    while pos >= *hpos + HASHTAB_LAG + LATEST_BATCH {
        macro_rules! insert {
            ($i:literal) => {{
                let insert_pos = *hpos + $i;
                let hsh = hash6(paranoid_unsafe_call!(read_u64_le(src, insert_pos)));
                ht.insert(hsh, insert_pos);
            }};
        }

        insert!(0);
        insert!(1);
        insert!(2);
        insert!(3);
        insert!(4);
        insert!(5);
        insert!(6);
        insert!(7);
        *hpos += LATEST_BATCH;
    }
}

#[inline(always)]
fn batch_insert_swift(
    src: &[u8],
    ht6: &mut LatestHashTable,
    ht4: &mut LatestHashTable,
    hpos: &mut usize,
    pos: usize,
) {
    while pos >= *hpos + HASHTAB_LAG + LATEST_BATCH {
        macro_rules! insert {
            ($i:literal) => {{
                let insert_pos = *hpos + $i;
                ht6.insert(
                    hash6(paranoid_unsafe_call!(read_u64_le(src, insert_pos))),
                    insert_pos,
                );
                if insert_pos & 1 == 0 {
                    let val = paranoid_unsafe_call!(read_u32_le(src, insert_pos));
                    ht4.insert(hash4(val), insert_pos);
                }
            }};
        }

        insert!(0);
        insert!(1);
        insert!(2);
        insert!(3);
        insert!(4);
        insert!(5);
        insert!(6);
        insert!(7);
        *hpos += LATEST_BATCH;
    }
}

#[inline(always)]
fn batch_insert_ring<const WIDTH: usize>(
    src: &[u8],
    ht: &mut RingHashTable<WIDTH>,
    hpos: &mut usize,
    pos: usize,
) {
    while pos >= *hpos + HASHTAB_LAG + RING_BATCH {
        macro_rules! insert {
            ($i:literal) => {{
                let insert_pos = *hpos + $i;
                let hsh = hash4(paranoid_unsafe_call!(read_u32_le(src, insert_pos)));
                ht.insert(hsh, insert_pos);
            }};
        }

        insert!(0);
        insert!(1);
        insert!(2);
        insert!(3);
        insert!(4);
        insert!(5);
        insert!(6);
        insert!(7);
        *hpos += RING_BATCH;
    }
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn emit_token(
    dst: &mut [u8],
    dlpos: &mut usize,
    drpos: &mut usize,
    src: &[u8],
    lit: usize,
    lit_len: usize,
    match_len: usize,
    dis: usize,
) {
    let norm_match = match_len - (MIN_MATCH_LEN - 1);
    let lrem = lit_len.min(TOKEN_LIT_MAX);
    dst[*dlpos] = ((lrem as u8) << TOKEN_MATCH_BITS) | (norm_match as u8);
    *dlpos += 1;

    let dbytes = (dis - MIN_DISTANCE) as u16;
    dst[*dlpos..*dlpos + 2].copy_from_slice(&dbytes.to_le_bytes());
    *dlpos += 2;

    if lit_len >= TOKEN_LIT_MAX {
        let mut remaining = lit_len - TOKEN_LIT_MAX;
        while remaining >= 255 {
            dst[*dlpos] = 255;
            *dlpos += 1;
            remaining -= 255;
        }
        dst[*dlpos] = remaining as u8;
        *dlpos += 1;
    }

    *drpos -= lit_len;
    dst[*drpos..*drpos + lit_len].copy_from_slice(&src[lit..lit + lit_len]);
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn emit_heavy_token(
    dst: &mut [u8],
    dlpos: &mut usize,
    drpos: &mut usize,
    src: &[u8],
    lit: usize,
    lit_len: usize,
    match_len_code: usize,
    dis: usize,
) {
    debug_assert!(match_len_code > 0);
    debug_assert!(match_len_code < HEAVY_LEN_OF.len());
    debug_assert!((HEAVY_MIN_DISTANCE..=HEAVY_MAX_DISTANCE).contains(&dis));

    let lrem = lit_len.min(HEAVY_TOKEN_LIT_MAX);
    let token = ((lrem as u32) << 26)
        | ((match_len_code as u32) << 20)
        | (((dis - HEAVY_MIN_DISTANCE) as u32) & HEAVY_DIS_MASK);
    dst[*dlpos..*dlpos + 4].copy_from_slice(&token.to_le_bytes());
    *dlpos += 4;

    if lrem == HEAVY_TOKEN_LIT_MAX {
        let mut remaining = lit_len - HEAVY_TOKEN_LIT_MAX;
        while remaining >= 255 {
            dst[*dlpos] = 255;
            *dlpos += 1;
            remaining -= 255;
        }
        dst[*dlpos] = remaining as u8;
        *dlpos += 1;
    }

    *drpos -= lit_len;
    dst[*drpos..*drpos + lit_len].copy_from_slice(&src[lit..lit + lit_len]);
}

fn heavy_raw(src: &[u8], dst: &mut [u8]) -> usize {
    let src_size = src.len();
    let flags = FLAG_HEAVY as u64;
    dst[0..8].copy_from_slice(&((src_size as u64) | (flags << FLAG_SHIFT)).to_le_bytes());
    let mut dlpos = HEADER_SIZE;

    if src_size <= HEAVY_SMALL_LIM {
        dst[dlpos..dlpos + src_size].copy_from_slice(src);
        return dlpos + src_size;
    }

    dst[dlpos..dlpos + 8].copy_from_slice(&(src_size as u64).to_le_bytes());
    dlpos += 8;
    dst[dlpos..dlpos + src_size].copy_from_slice(src);
    dlpos + src_size
}

#[derive(Clone, Copy)]
struct HeavyTokenRecord {
    match_start: usize,
    len: u8,
    dis: u32,
}

struct HeavyWorkspace {
    sorter: SaisWorkspace,
    sa: Vec<i32>,
    rank: Vec<u32>,
    live: HeavyLiveSet,
    max_len: Vec<u8>,
    match_dis: Vec<u32>,
    dp: Vec<usize>,
    arrival_len: Vec<u8>,
    block_tokens: Vec<HeavyTokenRecord>,
}

impl HeavyWorkspace {
    fn new() -> Self {
        Self {
            sorter: SaisWorkspace::new(),
            sa: Vec::new(),
            rank: Vec::new(),
            live: HeavyLiveSet::default(),
            max_len: Vec::new(),
            match_dis: Vec::new(),
            dp: Vec::new(),
            arrival_len: Vec::new(),
            block_tokens: Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
struct LightArrival {
    dis: u32,
    len: u8,
    lit_run: usize,
}

impl LightArrival {
    const ZERO: Self = Self {
        dis: 0,
        len: 0,
        lit_run: 0,
    };
}

#[derive(Clone, Copy)]
struct LightTokenRecord {
    match_start: usize,
    len: u8,
    dis: u32,
}

struct LightWorkspace {
    sorter: SaisWorkspace,
    sa: Vec<i32>,
    rank: Vec<u32>,
    live: HeavyLiveSet,
    max_len: Vec<u8>,
    match_dis: Vec<u32>,
    dp: Vec<usize>,
    arrivals: Vec<LightArrival>,
    block_tokens: Vec<LightTokenRecord>,
}

impl LightWorkspace {
    fn new() -> Self {
        Self {
            sorter: SaisWorkspace::new(),
            sa: Vec::new(),
            rank: Vec::new(),
            live: HeavyLiveSet::default(),
            max_len: Vec::new(),
            match_dis: Vec::new(),
            dp: Vec::new(),
            arrivals: Vec::new(),
            block_tokens: Vec::new(),
        }
    }
}

#[cfg(feature = "std")]
std::thread_local! {
    static LIGHT_WORKSPACE: core::cell::RefCell<LightWorkspace> =
        core::cell::RefCell::new(LightWorkspace::new());
}

#[cfg(all(not(feature = "paranoid"), feature = "std"))]
std::thread_local! {
    static HEAVY_WORKSPACE: core::cell::RefCell<HeavyWorkspace> =
        core::cell::RefCell::new(HeavyWorkspace::new());
}

#[cfg(all(feature = "paranoid", feature = "std"))]
std::thread_local! {
    static HEAVY_WORKSPACE: core::cell::RefCell<HeavyWorkspace> =
        core::cell::RefCell::new(HeavyWorkspace::new());
}

#[cfg(not(feature = "paranoid"))]
#[allow(clippy::too_many_arguments)]
#[inline(always)]
unsafe fn heavy_find_matches_simd<S: Simd + Copy>(
    simd: S,
    src: &[u8],
    sa: &[i32],
    rank: &[u32],
    live: &mut HeavyLiveSet,
    max_len: &mut [u8],
    match_dis: &mut [u32],
    block_start: usize,
    block_end: usize,
    seg_start: usize,
    hard_end: usize,
) {
    debug_assert_eq!(max_len.len(), block_end - block_start);
    debug_assert_eq!(match_dis.len(), block_end - block_start);
    debug_assert_eq!(rank.len(), sa.len());

    let sa_ptr = sa.as_ptr();
    let rank_ptr = rank.as_ptr();
    let max_len_ptr = max_len.as_mut_ptr();
    let match_dis_ptr = match_dis.as_mut_ptr();
    let mut carry_len = 0usize;
    let mut carry_dis = 0u32;
    let set_start = block_start.max(seg_start + HEAVY_MIN_DISTANCE);
    let clear_start = block_start.max(seg_start + HEAVY_MAX_DISTANCE + 1);

    // SAFETY: caller built `sa`/`rank` for `[seg_start, seg_end)`, sized
    // outputs to block length, and built `live` over those SA ranks.
    unsafe {
        for pos in block_start..block_end {
            let block_pos = pos - block_start;
            if pos >= set_start {
                let live_rank = *rank_ptr.add(pos - HEAVY_MIN_DISTANCE - seg_start);
                live.set_unchecked(live_rank as usize);
            }
            if pos >= clear_start {
                let dead_rank = *rank_ptr.add(pos - HEAVY_MAX_DISTANCE - 1 - seg_start);
                live.clear_unchecked(dead_rank as usize);
            }

            if pos + MIN_MATCH_LEN > hard_end {
                *max_len_ptr.add(block_pos) = 0;
                continue;
            }
            let limit = HEAVY_MAX_MATCH_LEN.min(hard_end - pos);

            if carry_len >= limit {
                *max_len_ptr.add(block_pos) = limit as u8;
                *match_dis_ptr.add(block_pos) = carry_dis;
                carry_len -= 1;
                continue;
            }

            let rank_pos = *rank_ptr.add(pos - seg_start) as usize;
            let mut best_len = 0usize;
            let mut best_dis = 0u32;

            macro_rules! consider_rank {
                ($candidate_rank:expr) => {{
                    let candidate_rank = $candidate_rank;
                    if candidate_rank != HEAVY_NO_RANK {
                        debug_assert!(candidate_rank < sa.len());
                        let sa_pos = *sa_ptr.add(candidate_rank);
                        debug_assert!(sa_pos >= 0);
                        let candidate = seg_start + sa_pos as u32 as usize;
                        debug_assert!(candidate <= pos - HEAVY_MIN_DISTANCE);
                        let len = lcp_heavy(simd, src, pos, candidate, limit);
                        if len > best_len {
                            best_len = len;
                            best_dis = (pos - candidate) as u32;
                        }
                    }
                }};
            }

            consider_rank!(live.prev_unchecked(rank_pos));
            consider_rank!(live.next_unchecked(rank_pos));

            if best_len >= MIN_MATCH_LEN {
                *max_len_ptr.add(block_pos) = best_len as u8;
                *match_dis_ptr.add(block_pos) = best_dis;
                if best_len >= limit {
                    let room = src.len() - (pos + limit);
                    let ext_limit = room.min(HEAVY_NDIS);
                    let ext = lcp_heavy(
                        simd,
                        src,
                        pos + limit,
                        pos - best_dis as usize + limit,
                        ext_limit,
                    );
                    carry_len = limit + ext - 1;
                    carry_dis = best_dis;
                } else {
                    carry_len = best_len - 1;
                    carry_dis = best_dis;
                }
            } else {
                *max_len_ptr.add(block_pos) = 0;
                carry_len = carry_len.saturating_sub(1);
            }
        }
    }
}

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[allow(clippy::too_many_arguments)]
#[target_feature(enable = "avx2")]
unsafe fn heavy_find_matches_avx2(
    src: &[u8],
    sa: &[i32],
    rank: &[u32],
    live: &mut HeavyLiveSet,
    max_len: &mut [u8],
    match_dis: &mut [u32],
    block_start: usize,
    block_end: usize,
    seg_start: usize,
    hard_end: usize,
) {
    debug_assert_eq!(max_len.len(), block_end - block_start);
    debug_assert_eq!(match_dis.len(), block_end - block_start);
    debug_assert_eq!(rank.len(), sa.len());

    let src_ptr = src.as_ptr();
    let sa_ptr = sa.as_ptr();
    let rank_ptr = rank.as_ptr();
    let max_len_ptr = max_len.as_mut_ptr();
    let match_dis_ptr = match_dis.as_mut_ptr();
    let mut carry_len = 0usize;
    let mut carry_dis = 0u32;
    let set_start = block_start.max(seg_start + HEAVY_MIN_DISTANCE);
    let clear_start = block_start.max(seg_start + HEAVY_MAX_DISTANCE + 1);

    // SAFETY: caller built `sa`/`rank` for `[seg_start, seg_end)`, sized
    // outputs to block length, and built `live` over those SA ranks. Runtime
    // dispatch selected AVX2, and every LCP call is clipped to valid input.
    unsafe {
        macro_rules! lcp_at {
            ($a:expr, $b:expr, $limit:expr) => {{
                'lcp: {
                    let a = $a;
                    let b = $b;
                    let limit = $limit;
                    debug_assert!(a + limit <= src.len());
                    debug_assert!(b + limit <= src.len());

                    let mut off = 0usize;
                    while off + VECTOR_WIDTH <= limit {
                        let av = _mm256_lddqu_si256(src_ptr.add(a + off).cast::<__m256i>());
                        let bv = _mm256_lddqu_si256(src_ptr.add(b + off).cast::<__m256i>());
                        let eq = _mm256_cmpeq_epi8(av, bv);
                        let diff = !(_mm256_movemask_epi8(eq) as u32);
                        let len = diff.trailing_zeros() as usize;
                        off += len;
                        if len < VECTOR_WIDTH {
                            break 'lcp off;
                        }
                    }
                    while off + 8 <= limit {
                        let diff = ptr::read_unaligned(src_ptr.add(a + off).cast::<u64>()).to_le()
                            ^ ptr::read_unaligned(src_ptr.add(b + off).cast::<u64>()).to_le();
                        if diff != 0 {
                            break 'lcp off + (diff.trailing_zeros() as usize >> 3);
                        }
                        off += 8;
                    }
                    while off < limit {
                        if *src_ptr.add(a + off) != *src_ptr.add(b + off) {
                            break;
                        }
                        off += 1;
                    }
                    off
                }
            }};
        }

        for pos in block_start..block_end {
            let block_pos = pos - block_start;
            if pos >= set_start {
                let live_rank = *rank_ptr.add(pos - HEAVY_MIN_DISTANCE - seg_start);
                live.set_unchecked(live_rank as usize);
            }
            if pos >= clear_start {
                let dead_rank = *rank_ptr.add(pos - HEAVY_MAX_DISTANCE - 1 - seg_start);
                live.clear_unchecked(dead_rank as usize);
            }

            if pos + MIN_MATCH_LEN > hard_end {
                *max_len_ptr.add(block_pos) = 0;
                continue;
            }
            let limit = HEAVY_MAX_MATCH_LEN.min(hard_end - pos);

            if carry_len >= limit {
                *max_len_ptr.add(block_pos) = limit as u8;
                *match_dis_ptr.add(block_pos) = carry_dis;
                carry_len -= 1;
                continue;
            }

            let rank_pos = *rank_ptr.add(pos - seg_start) as usize;
            let mut best_len = 0usize;
            let mut best_dis = 0u32;

            macro_rules! consider_rank {
                ($candidate_rank:expr) => {{
                    let candidate_rank = $candidate_rank;
                    if candidate_rank != HEAVY_NO_RANK {
                        debug_assert!(candidate_rank < sa.len());
                        let sa_pos = *sa_ptr.add(candidate_rank);
                        debug_assert!(sa_pos >= 0);
                        let candidate = seg_start + sa_pos as u32 as usize;
                        debug_assert!(candidate <= pos - HEAVY_MIN_DISTANCE);
                        let len = lcp_at!(pos, candidate, limit);
                        if len > best_len {
                            best_len = len;
                            best_dis = (pos - candidate) as u32;
                        }
                    }
                }};
            }

            consider_rank!(live.prev_unchecked(rank_pos));
            consider_rank!(live.next_unchecked(rank_pos));

            if best_len >= MIN_MATCH_LEN {
                *max_len_ptr.add(block_pos) = best_len as u8;
                *match_dis_ptr.add(block_pos) = best_dis;
                if best_len >= limit {
                    let room = src.len() - (pos + limit);
                    let ext_limit = room.min(HEAVY_NDIS);
                    let ext = lcp_at!(pos + limit, pos - best_dis as usize + limit, ext_limit);
                    carry_len = limit + ext - 1;
                    carry_dis = best_dis;
                } else {
                    carry_len = best_len - 1;
                    carry_dis = best_dis;
                }
            } else {
                *max_len_ptr.add(block_pos) = 0;
                carry_len = carry_len.saturating_sub(1);
            }
        }
    }
}

#[cfg(feature = "paranoid")]
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn heavy_find_matches<F>(
    src: &[u8],
    sa: &[i32],
    rank: &[u32],
    live: &mut HeavyLiveSet,
    max_len: &mut [u8],
    match_dis: &mut [u32],
    block_start: usize,
    block_end: usize,
    seg_start: usize,
    hard_end: usize,
    lcp_at: &mut F,
) where
    F: FnMut(&[u8], usize, usize, usize) -> usize,
{
    let mut carry_len = 0usize;
    let mut carry_dis = 0u32;
    let set_start = block_start.max(seg_start + HEAVY_MIN_DISTANCE);
    let clear_start = block_start.max(seg_start + HEAVY_MAX_DISTANCE + 1);
    for pos in block_start..block_end {
        let block_pos = pos - block_start;
        if pos >= set_start {
            live.set(rank[pos - HEAVY_MIN_DISTANCE - seg_start] as usize);
        }
        if pos >= clear_start {
            live.clear(rank[pos - HEAVY_MAX_DISTANCE - 1 - seg_start] as usize);
        }

        if pos + MIN_MATCH_LEN > hard_end {
            max_len[block_pos] = 0;
            continue;
        }
        let limit = HEAVY_MAX_MATCH_LEN.min(hard_end - pos);

        if carry_len >= limit {
            max_len[block_pos] = limit as u8;
            match_dis[block_pos] = carry_dis;
            carry_len -= 1;
            continue;
        }

        let rank_pos = rank[pos - seg_start] as usize;
        let mut best_len = 0usize;
        let mut best_dis = 0u32;

        if let Some(candidate_rank) = live.prev(rank_pos) {
            let candidate = seg_start + sa[candidate_rank] as u32 as usize;
            debug_assert!(candidate <= pos - HEAVY_MIN_DISTANCE);
            let len = lcp_at(src, pos, candidate, limit);
            if len > best_len {
                best_len = len;
                best_dis = (pos - candidate) as u32;
            }
        }
        if let Some(candidate_rank) = live.next(rank_pos) {
            let candidate = seg_start + sa[candidate_rank] as u32 as usize;
            debug_assert!(candidate <= pos - HEAVY_MIN_DISTANCE);
            let len = lcp_at(src, pos, candidate, limit);
            if len > best_len {
                best_len = len;
                best_dis = (pos - candidate) as u32;
            }
        }

        if best_len >= MIN_MATCH_LEN {
            max_len[block_pos] = best_len as u8;
            match_dis[block_pos] = best_dis;
            if best_len >= limit {
                let room = src.len() - (pos + limit);
                let ext_limit = room.min(HEAVY_NDIS);
                let ext = lcp_at(src, pos + limit, pos - best_dis as usize + limit, ext_limit);
                carry_len = limit + ext - 1;
                carry_dis = best_dis;
            } else {
                carry_len = best_len - 1;
                carry_dis = best_dis;
            }
        } else {
            max_len[block_pos] = 0;
            carry_len = carry_len.saturating_sub(1);
        }
    }
}

#[inline(always)]
fn loose_compress<S: Simd>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    ring_adaptive_compress::<S, LOOSE_RING_WIDTH>(simd, src, dst, false)
}

#[inline(always)]
fn blitz_compress<S: Simd>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    const ACCEPT_LEN: usize = 9;
    const FIRE_AT: usize = 6;

    let src_size = src.len();

    dst[0..8].copy_from_slice(&(src_size as u64).to_le_bytes());
    let mut dlpos: usize = 8;

    if src_size <= SMALL_LIM {
        dst[8..8 + src_size].copy_from_slice(src);
        return 8 + src_size;
    }

    let literal_suffix_pos = dlpos;
    dlpos += 8;

    let dst_cap = dst.len();
    let mut drpos = dst_cap;
    let match_end_limit = src_size - LITERAL_SUFFIX;

    let mut ht = LatestHashTable::new();
    let mut pos: usize = 0;
    let mut hpos: usize = 0;
    let mut lit: usize = 0;
    let mut miss_run: usize = 0;

    let mut cand_pos: usize = 0;
    let mut cand_len: usize = 0;
    let mut cand_lst: usize = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_latest6(src, &mut ht, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_latest_match6(simd, src, &ht, pos)
        } else {
            (0, 0)
        };

        let pos_safe_bound = pos;
        let pend = pos - lit;
        let mut accept = match_len >= ACCEPT_LEN;
        let fire = pend + BLITZ_PROBE_STEP > FIRE_AT;

        if !accept {
            if fire {
                if cand_len != 0
                    && (match_len < MIN_MATCH_LEN || cand_pos + cand_len >= pos + match_len)
                {
                    pos = cand_pos;
                    lst = cand_lst;
                    match_len = cand_len;
                }
                accept = match_len >= MIN_MATCH_LEN;
            } else if match_len >= MIN_MATCH_LEN
                && (cand_len == 0 || pos + match_len >= cand_pos + cand_len)
            {
                cand_pos = pos;
                cand_len = match_len;
                cand_lst = lst;
            }
        }

        if !accept {
            pos += BLITZ_PROBE_STEP + (miss_run >> SKIP_SHIFT);
            miss_run += 1;
            continue;
        }

        miss_run = 0;

        // Backward match extension
        while pos > lit && lst > 0 && match_len < MAX_MATCH_LEN && src[pos - 1] == src[lst - 1] {
            pos -= 1;
            lst -= 1;
            match_len += 1;
        }

        let lit_len = pos - lit;
        let dis = pos - lst;

        emit_token(
            dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
        );

        pos += match_len;
        lit = pos;
        pos = pos.max(pos_safe_bound);
        cand_len = 0;
    }

    finish_light(src, dst, dst_cap, dlpos, drpos, literal_suffix_pos, lit)
}

#[inline(always)]
fn swift_compress<S: Simd>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    const ACCEPT_LEN: usize = 7;
    const FIRE_AT: usize = 6;

    let src_size = src.len();

    dst[0..8].copy_from_slice(&(src_size as u64).to_le_bytes());
    let mut dlpos: usize = 8;

    if src_size <= SMALL_LIM {
        dst[8..8 + src_size].copy_from_slice(src);
        return 8 + src_size;
    }

    let literal_suffix_pos = dlpos;
    dlpos += 8;

    let dst_cap = dst.len();
    let mut drpos = dst_cap;
    let match_end_limit = src_size - LITERAL_SUFFIX;

    let mut ht6 = LatestHashTable::new();
    let mut ht4 = LatestHashTable::new();
    let mut pos: usize = 0;
    let mut hpos: usize = 0;
    let mut lit: usize = 0;
    let mut miss_run: usize = 0;

    let mut cand_pos: usize = 0;
    let mut cand_len: usize = 0;
    let mut cand_lst: usize = 0;
    let mut regime: i32 = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_swift(src, &mut ht6, &mut ht4, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_swift_match(simd, src, &ht6, &ht4, pos, cand_len)
        } else {
            (0, 0)
        };

        let pos_safe_bound = pos;
        let pend = pos - lit;
        let mut accept = match_len >= ACCEPT_LEN;
        let fire = if regime >= LIGHT_REGIME_THRESHOLD {
            pend >= FIRE_AT
        } else {
            pend == FIRE_AT
        };

        if !accept {
            if fire {
                if cand_len != 0
                    && (match_len < MIN_MATCH_LEN || cand_pos + cand_len >= pos + match_len)
                {
                    pos = cand_pos;
                    lst = cand_lst;
                    match_len = cand_len;
                }
                accept = match_len >= MIN_MATCH_LEN;
            } else if match_len >= MIN_MATCH_LEN
                && (cand_len == 0 || pos + match_len >= cand_pos + cand_len)
            {
                cand_pos = pos;
                cand_len = match_len;
                cand_lst = lst;
            }
        }

        if !accept {
            pos += 1 + (miss_run >> SKIP_SHIFT);
            miss_run += 1;
            continue;
        }

        miss_run = 0;

        // Backward match extension
        while pos > lit && lst > 0 && match_len < MAX_MATCH_LEN && src[pos - 1] == src[lst - 1] {
            pos -= 1;
            lst -= 1;
            match_len += 1;
        }

        let lit_len = pos - lit;
        let dis = pos - lst;
        vote_loose_regime(&mut regime, lit_len);

        emit_token(
            dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
        );

        pos += match_len;
        lit = pos;
        pos = pos.max(pos_safe_bound);
        cand_len = 0;
    }

    finish_light(src, dst, dst_cap, dlpos, drpos, literal_suffix_pos, lit)
}

#[inline(always)]
fn keen_compress<S: Simd>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    ring_adaptive_compress::<S, KEEN_RING_WIDTH>(simd, src, dst, true)
}

#[inline(always)]
fn ring_adaptive_compress<S: Simd, const WIDTH: usize>(
    simd: S,
    src: &[u8],
    dst: &mut [u8],
    keen: bool,
) -> usize {
    const LOOSE_ACCEPT_LEN: usize = 7;
    const KEEN_ACCEPT_LEN: usize = 6;
    const FIRE_AT: usize = 6;
    const LA_GATE: usize = 16;
    const LA_PATE: usize = 8;

    let src_size = src.len();

    dst[0..8].copy_from_slice(&(src_size as u64).to_le_bytes());
    let mut dlpos: usize = 8;

    if src_size <= SMALL_LIM {
        dst[8..8 + src_size].copy_from_slice(src);
        return 8 + src_size;
    }

    let literal_suffix_pos = dlpos;
    dlpos += 8;

    let dst_cap = dst.len();
    let mut drpos = dst_cap;
    let match_end_limit = src_size - LITERAL_SUFFIX;

    let mut ht = RingHashTable::<WIDTH>::new();
    let mut pos: usize = 0;
    let mut hpos: usize = 0;
    let mut lit: usize = 0;
    let mut miss_run: usize = 0;

    let mut cand_pos: usize = 0;
    let mut cand_len: usize = 0;
    let mut cand_lst: usize = 0;
    let mut regime: i32 = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_ring(src, &mut ht, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_ring_match(simd, src, &ht, pos)
        } else {
            (0, 0)
        };

        let pos_safe_bound = pos;
        let accept_len = if keen && regime >= LIGHT_REGIME_THRESHOLD {
            MIN_MATCH_LEN
        } else if keen {
            KEEN_ACCEPT_LEN
        } else {
            LOOSE_ACCEPT_LEN
        };
        let mut accept = match_len >= accept_len;

        if accept && keen {
            let base_pos = pos;
            let mut npos = base_pos + 1;
            while npos <= base_pos + 2
                && npos + MAX_MATCH_LEN <= match_end_limit
                && match_len < LA_GATE
            {
                let (nmatch_len, nlst) = find_ring_match(simd, src, &ht, npos);
                let improved = nmatch_len > match_len;
                if improved {
                    pos = npos;
                    lst = nlst;
                    match_len = nmatch_len;
                }
                if !improved && match_len >= LA_PATE {
                    break;
                }
                npos += 1;
            }
        } else if !accept {
            let pend = pos - lit;
            let fire = if keen || regime >= LIGHT_REGIME_THRESHOLD {
                pend >= FIRE_AT
            } else {
                pend == FIRE_AT
            };

            if fire {
                if cand_len != 0
                    && (match_len < MIN_MATCH_LEN || cand_pos + cand_len >= pos + match_len)
                {
                    pos = cand_pos;
                    lst = cand_lst;
                    match_len = cand_len;
                }
                accept = match_len >= MIN_MATCH_LEN;
            } else if match_len >= MIN_MATCH_LEN
                && (cand_len == 0 || pos + match_len >= cand_pos + cand_len)
            {
                cand_pos = pos;
                cand_len = match_len;
                cand_lst = lst;
            }
        }

        if !accept {
            pos += 1 + (miss_run >> SKIP_SHIFT);
            miss_run += 1;
            continue;
        }

        miss_run = 0;

        while pos > lit && lst > 0 && match_len < MAX_MATCH_LEN && src[pos - 1] == src[lst - 1] {
            pos -= 1;
            lst -= 1;
            match_len += 1;
        }

        let lit_len = pos - lit;
        let dis = pos - lst;
        if keen {
            vote_keen_regime(&mut regime, lit_len);
        } else {
            vote_loose_regime(&mut regime, lit_len);
        }

        emit_token(
            dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
        );

        pos += match_len;
        lit = pos;
        pos = pos.max(pos_safe_bound);
        cand_len = 0;
    }

    finish_light(src, dst, dst_cap, dlpos, drpos, literal_suffix_pos, lit)
}

#[inline(always)]
fn vote_loose_regime(regime: &mut i32, lit_len: usize) {
    let vote = if (7..=32).contains(&lit_len) { 2 } else { -1 };
    *regime = (*regime + vote).clamp(0, LIGHT_REGIME_CAP);
}

#[inline(always)]
fn vote_keen_regime(regime: &mut i32, lit_len: usize) {
    let vote = if lit_len >= TOKEN_LIT_MAX { 3 } else { -1 };
    *regime = (*regime + vote).clamp(0, LIGHT_REGIME_CAP);
}

#[inline(always)]
fn finish_light(
    src: &[u8],
    dst: &mut [u8],
    dst_cap: usize,
    mut dlpos: usize,
    drpos: usize,
    literal_suffix_pos: usize,
    lit: usize,
) -> usize {
    if drpos < dst_cap {
        let lit_data_len = dst_cap - drpos;
        dst.copy_within(drpos..dst_cap, dlpos);
        dlpos += lit_data_len;
    }

    let literal_suffix_cnt = src.len() - lit;
    dst[literal_suffix_pos..literal_suffix_pos + 8]
        .copy_from_slice(&(literal_suffix_cnt as u64).to_le_bytes());
    dst[dlpos..dlpos + literal_suffix_cnt].copy_from_slice(&src[lit..]);
    dlpos += literal_suffix_cnt;

    dlpos
}

#[inline(always)]
fn light_literal_extras(run: usize) -> usize {
    if run < TOKEN_LIT_MAX {
        0
    } else {
        1 + (run - TOKEN_LIT_MAX) / 255
    }
}

#[inline(always)]
fn light_optimal_compress<S: Simd + Copy>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    #[cfg(feature = "std")]
    {
        LIGHT_WORKSPACE.with(|cell| match cell.try_borrow_mut() {
            Ok(mut workspace) => light_optimal_with_workspace(simd, &mut workspace, src, dst),
            Err(_) => {
                let mut workspace = LightWorkspace::new();
                light_optimal_with_workspace(simd, &mut workspace, src, dst)
            }
        })
    }

    #[cfg(not(feature = "std"))]
    {
        let mut workspace = LightWorkspace::new();
        light_optimal_with_workspace(simd, &mut workspace, src, dst)
    }
}

#[inline(always)]
fn light_optimal_with_workspace<S: Simd + Copy>(
    simd: S,
    workspace: &mut LightWorkspace,
    src: &[u8],
    dst: &mut [u8],
) -> usize {
    let mut lcp_at = |src: &[u8], a: usize, b: usize, limit: usize| {
        #[cfg(not(feature = "paranoid"))]
        {
            lcp_heavy(simd, src, a, b, limit)
        }
        #[cfg(feature = "paranoid")]
        {
            let _ = simd;
            lcp_heavy(src, a, b, limit)
        }
    };
    light_optimal_body(workspace, src, dst, &mut lcp_at)
}

#[allow(clippy::too_many_arguments)]
fn light_find_matches<F>(
    src: &[u8],
    sa: &[i32],
    rank: &[u32],
    live: &mut HeavyLiveSet,
    max_len: &mut [u8],
    match_dis: &mut [u32],
    block_start: usize,
    block_end: usize,
    seg_start: usize,
    hard_end: usize,
    lcp_at: &mut F,
) where
    F: FnMut(&[u8], usize, usize, usize) -> usize,
{
    let mut carry_len = 0usize;
    let mut carry_dis = 0u32;
    let set_start = block_start.max(seg_start + MIN_DISTANCE);
    let clear_start = block_start.max(seg_start + MAX_DISTANCE + 1);

    for pos in block_start..block_end {
        let block_pos = pos - block_start;
        if pos >= set_start {
            live.set(rank[pos - MIN_DISTANCE - seg_start] as usize);
        }
        if pos >= clear_start {
            live.clear(rank[pos - MAX_DISTANCE - 1 - seg_start] as usize);
        }

        if pos + MIN_MATCH_LEN > hard_end {
            max_len[block_pos] = 0;
            continue;
        }
        let limit = MAX_MATCH_LEN.min(hard_end - pos);

        if carry_len >= limit {
            max_len[block_pos] = limit as u8;
            match_dis[block_pos] = carry_dis;
            carry_len -= 1;
            continue;
        }

        let rank_pos = rank[pos - seg_start] as usize;
        let mut best_len = 0usize;
        let mut best_dis = 0u32;

        if let Some(candidate_rank) = live.prev(rank_pos) {
            let candidate = seg_start + sa[candidate_rank] as u32 as usize;
            let len = lcp_at(src, pos, candidate, limit);
            if len > best_len {
                best_len = len;
                best_dis = (pos - candidate) as u32;
            }
        }
        if let Some(candidate_rank) = live.next(rank_pos) {
            let candidate = seg_start + sa[candidate_rank] as u32 as usize;
            let len = lcp_at(src, pos, candidate, limit);
            if len > best_len {
                best_len = len;
                best_dis = (pos - candidate) as u32;
            }
        }

        if best_len >= MIN_MATCH_LEN {
            max_len[block_pos] = best_len as u8;
            match_dis[block_pos] = best_dis;
            if best_len >= limit {
                let room = src.len() - (pos + limit);
                let ext_limit = room.min(DIS_LIM);
                let ext = lcp_at(src, pos + limit, pos - best_dis as usize + limit, ext_limit);
                carry_len = limit + ext - 1;
                carry_dis = best_dis;
            } else {
                carry_len = best_len - 1;
                carry_dis = best_dis;
            }
        } else {
            max_len[block_pos] = 0;
            carry_len = carry_len.saturating_sub(1);
        }
    }
}

fn light_optimal_body<F>(
    workspace: &mut LightWorkspace,
    src: &[u8],
    dst: &mut [u8],
    lcp_at: &mut F,
) -> usize
where
    F: FnMut(&[u8], usize, usize, usize) -> usize,
{
    let src_size = src.len();

    dst[0..8].copy_from_slice(&(src_size as u64).to_le_bytes());
    let mut dlpos = HEADER_SIZE;

    if src_size <= SMALL_LIM {
        dst[dlpos..dlpos + src_size].copy_from_slice(src);
        return dlpos + src_size;
    }

    let dst_cap = dst.len();
    let mut drpos = dst_cap;
    let literal_suffix_pos = dlpos;
    dlpos += 8;
    let match_end_limit = src_size - LITERAL_SUFFIX;
    let mut lit = 0usize;

    let LightWorkspace {
        sorter,
        sa,
        rank,
        live,
        max_len,
        match_dis,
        dp,
        arrivals,
        block_tokens,
    } = workspace;

    let mut qstar = 0usize;
    let mut qstar_cost = EXT_HEADER_SIZE;

    for block_start in (0..src_size).step_by(LIGHT_OPTIMAL_BLOCK_SIZE) {
        let block_end = (block_start + LIGHT_OPTIMAL_BLOCK_SIZE).min(src_size);
        let block_len = block_end - block_start;
        let seg_start = block_start.saturating_sub(MAX_DISTANCE);
        let seg_end = src_size.min(block_end + LIGHT_OPTIMAL_PAD_LEN);
        let seg_len = seg_end - seg_start;
        debug_assert!(seg_len <= i32::MAX as usize);

        sa.resize(seg_len, 0);
        rank.resize(seg_len, 0);
        sorter.suffix_array_with_rank(&src[seg_start..seg_end], sa, rank);

        max_len.clear();
        max_len.resize(block_len, 0);
        match_dis.clear();
        match_dis.resize(block_len, 0);

        let hard_end = block_end.min(match_end_limit);
        let init_limit = if block_start >= MIN_DISTANCE && block_start - MIN_DISTANCE >= seg_start {
            u32::try_from(block_start - MIN_DISTANCE - seg_start + 1).unwrap()
        } else {
            0
        };
        live.build(sa.as_slice(), seg_len, init_limit);

        light_find_matches(
            src,
            sa.as_slice(),
            rank.as_slice(),
            live,
            max_len,
            match_dis,
            block_start,
            block_end,
            seg_start,
            hard_end,
            lcp_at,
        );

        dp.clear();
        dp.resize(block_len + 1, LIGHT_OPTIMAL_DP_INF);
        arrivals.clear();
        arrivals.resize(block_len + 1, LightArrival::ZERO);

        let mut literal_run = block_start - qstar;
        let mut literal_cost = qstar_cost + literal_run + light_literal_extras(literal_run);
        let mut next_extra_at = if literal_run < TOKEN_LIT_MAX {
            TOKEN_LIT_MAX
        } else {
            literal_run + 255 - (literal_run - TOKEN_LIT_MAX) % 255
        };
        if block_start == 0 {
            dp[0] = qstar_cost;
        }

        for pos in block_start..=block_end {
            let i = pos - block_start;
            if pos > block_start {
                literal_run += 1;
                if literal_run >= next_extra_at {
                    literal_cost += 1;
                    next_extra_at = literal_run + 255;
                }
                literal_cost += 1;
                if dp[i] <= literal_cost {
                    literal_cost = dp[i];
                    literal_run = 0;
                    next_extra_at = TOKEN_LIT_MAX;
                }
            }

            let longest = if i < block_len {
                max_len[i] as usize
            } else {
                0
            };
            if longest >= MIN_MATCH_LEN {
                let cost = literal_cost + 3;
                for len in MIN_MATCH_LEN..=longest {
                    let target = i + len;
                    if cost < dp[target] {
                        dp[target] = cost;
                        arrivals[target] = LightArrival {
                            dis: match_dis[i],
                            len: len as u8,
                            lit_run: literal_run,
                        };
                    }
                }
            }
        }

        let (commit, commit_cost) = if block_end < src_size {
            let commit = block_end - literal_run;
            (
                commit,
                literal_cost - literal_run - light_literal_extras(literal_run),
            )
        } else {
            let mut best_total = qstar_cost + (src_size - qstar);
            let mut best_commit = qstar;
            let mut best_commit_cost = qstar_cost;
            for pos in block_start..=block_end {
                let cost = dp[pos - block_start];
                if cost == LIGHT_OPTIMAL_DP_INF {
                    continue;
                }
                let total = cost + (src_size - pos);
                if total < best_total {
                    best_total = total;
                    best_commit = pos;
                    best_commit_cost = cost;
                }
            }
            (best_commit, best_commit_cost)
        };

        block_tokens.clear();
        let mut boundary = commit;
        while boundary > qstar {
            let arrival = arrivals[boundary - block_start];
            let matched_len = arrival.len as usize;
            let Some(next) = boundary
                .checked_sub(matched_len)
                .and_then(|pos| pos.checked_sub(arrival.lit_run))
            else {
                return 0;
            };
            if matched_len < MIN_MATCH_LEN || (next < block_start && next != qstar) {
                return 0;
            }
            block_tokens.push(LightTokenRecord {
                match_start: boundary - matched_len,
                len: arrival.len,
                dis: arrival.dis,
            });
            boundary = next;
        }

        for token in block_tokens.iter().rev() {
            let lit_len = token.match_start - lit;
            emit_token(
                dst,
                &mut dlpos,
                &mut drpos,
                src,
                lit,
                lit_len,
                token.len as usize,
                token.dis as usize,
            );
            lit = token.match_start + token.len as usize;
        }

        qstar = commit;
        qstar_cost = commit_cost;
    }

    finish_light(src, dst, dst_cap, dlpos, drpos, literal_suffix_pos, lit)
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn heavy_compress<S: Simd + Copy>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    let mut workspace = HeavyWorkspace::new();
    heavy_compress_body(
        &mut workspace,
        src,
        dst,
        |src, sa, rank, live, max_len, match_dis, block_start, block_end, seg_start, hard_end| {
            paranoid_unsafe_call!(heavy_find_matches_simd(
                simd,
                src,
                sa,
                rank,
                live,
                max_len,
                match_dis,
                block_start,
                block_end,
                seg_start,
                hard_end,
            ));
        },
    )
}

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn heavy_compress_avx2(src: &[u8], dst: &mut [u8]) -> usize {
    #[cfg(feature = "std")]
    {
        HEAVY_WORKSPACE.with(|cell| match cell.try_borrow_mut() {
            Ok(mut workspace) => {
                // SAFETY: caller reached this function through AVX2 runtime dispatch.
                unsafe { heavy_compress_avx2_with_workspace(&mut workspace, src, dst) }
            }
            Err(_) => {
                let mut workspace = HeavyWorkspace::new();
                // SAFETY: caller reached this function through AVX2 runtime dispatch.
                unsafe { heavy_compress_avx2_with_workspace(&mut workspace, src, dst) }
            }
        })
    }

    #[cfg(not(feature = "std"))]
    {
        let mut workspace = HeavyWorkspace::new();
        // SAFETY: caller reached this function through AVX2 runtime dispatch.
        unsafe { heavy_compress_avx2_with_workspace(&mut workspace, src, dst) }
    }
}

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn heavy_compress_avx2_with_workspace(
    workspace: &mut HeavyWorkspace,
    src: &[u8],
    dst: &mut [u8],
) -> usize {
    heavy_compress_body(
        workspace,
        src,
        dst,
        |src, sa, rank, live, max_len, match_dis, block_start, block_end, seg_start, hard_end| {
            // SAFETY: caller reached this function through AVX2 runtime dispatch.
            unsafe {
                heavy_find_matches_avx2(
                    src,
                    sa,
                    rank,
                    live,
                    max_len,
                    match_dis,
                    block_start,
                    block_end,
                    seg_start,
                    hard_end,
                )
            };
        },
    )
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn heavy_compress(src: &[u8], dst: &mut [u8]) -> usize {
    #[cfg(feature = "std")]
    {
        HEAVY_WORKSPACE.with(|cell| match cell.try_borrow_mut() {
            Ok(mut workspace) => heavy_compress_paranoid_with_workspace(&mut workspace, src, dst),
            Err(_) => {
                let mut workspace = HeavyWorkspace::new();
                heavy_compress_paranoid_with_workspace(&mut workspace, src, dst)
            }
        })
    }

    #[cfg(not(feature = "std"))]
    {
        let mut workspace = HeavyWorkspace::new();
        heavy_compress_paranoid_with_workspace(&mut workspace, src, dst)
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn heavy_compress_paranoid_with_workspace(
    workspace: &mut HeavyWorkspace,
    src: &[u8],
    dst: &mut [u8],
) -> usize {
    heavy_compress_body(
        workspace,
        src,
        dst,
        |src, sa, rank, live, max_len, match_dis, block_start, block_end, seg_start, hard_end| {
            let mut lcp_at = lcp_heavy;
            heavy_find_matches(
                src,
                sa,
                rank,
                live,
                max_len,
                match_dis,
                block_start,
                block_end,
                seg_start,
                hard_end,
                &mut lcp_at,
            );
        },
    )
}

#[inline(always)]
fn heavy_compress_body<F>(
    workspace: &mut HeavyWorkspace,
    src: &[u8],
    dst: &mut [u8],
    mut find_matches: F,
) -> usize
where
    F: FnMut(
        &[u8],
        &[i32],
        &[u32],
        &mut HeavyLiveSet,
        &mut [u8],
        &mut [u32],
        usize,
        usize,
        usize,
        usize,
    ),
{
    let src_size = src.len();
    if src_size <= HEAVY_SMALL_LIM {
        return heavy_raw(src, dst);
    }

    let mut dlpos = 0usize;
    let dst_cap = dst.len();
    let mut drpos = dst_cap;
    let header = (src_size as u64) | ((FLAG_HEAVY as u64) << FLAG_SHIFT);
    dst[dlpos..dlpos + 8].copy_from_slice(&header.to_le_bytes());
    dlpos += 8;

    let literal_suffix_pos = dlpos;
    dlpos += 8;
    let match_end_limit = src_size - HEAVY_LITERAL_SUFFIX;
    let mut lit = 0usize;
    let mut tokens = 0usize;
    let mut long_matches = 0usize;

    type TokenRecord = HeavyTokenRecord;

    let HeavyWorkspace {
        sorter,
        sa,
        rank,
        live,
        max_len,
        match_dis,
        dp,
        arrival_len,
        block_tokens,
    } = workspace;

    let mut qstar = 0usize;
    let mut qstar_cost = EXT_HEADER_SIZE;

    for block_start in (0..src_size).step_by(HEAVY_BLOCK_SIZE) {
        let block_end = (block_start + HEAVY_BLOCK_SIZE).min(src_size);
        let block_len = block_end - block_start;

        let seg_start = block_start.saturating_sub(HEAVY_MAX_DISTANCE);
        let seg_end = src_size.min(block_end + HEAVY_PAD_LEN);
        let seg_len = seg_end - seg_start;
        debug_assert!(seg_len <= i32::MAX as usize);

        sa.resize(seg_len, 0);
        rank.resize(seg_len, 0);
        sorter.suffix_array_with_rank(&src[seg_start..seg_end], sa, rank);

        if max_len.len() != block_len {
            max_len.resize(block_len, 0u8);
        }
        if match_dis.len() != block_len {
            match_dis.resize(block_len, 0u32);
        }

        let hard_end = block_end.min(match_end_limit);
        let init_limit =
            if block_start >= HEAVY_MIN_DISTANCE && block_start - HEAVY_MIN_DISTANCE >= seg_start {
                u32::try_from(block_start - HEAVY_MIN_DISTANCE - seg_start + 1).unwrap()
            } else {
                0
            };
        live.build(sa.as_slice(), seg_len, init_limit);

        find_matches(
            src,
            sa.as_slice(),
            rank.as_slice(),
            live,
            max_len,
            match_dis,
            block_start,
            block_end,
            seg_start,
            hard_end,
        );

        dp.clear();
        dp.resize(block_len + 1, HEAVY_DP_INF);
        if arrival_len.len() != block_len + 1 {
            arrival_len.resize(block_len + 1, 0);
        }

        let mut literal_run = block_start - qstar;
        let mut literal_cost = qstar_cost + literal_run + heavy_literal_extras(literal_run);
        let mut next_extra_at = if literal_run < HEAVY_TOKEN_LIT_MAX {
            HEAVY_TOKEN_LIT_MAX
        } else {
            literal_run + 255 - (literal_run - HEAVY_TOKEN_LIT_MAX) % 255
        };
        if block_start == 0 {
            dp[0] = qstar_cost;
        }

        #[cfg(not(feature = "paranoid"))]
        {
            let dp_ptr = dp.as_mut_ptr();
            let arrival_len_ptr = arrival_len.as_mut_ptr();
            let max_len_ptr = max_len.as_ptr();
            // SAFETY: DP arrays have `block_len + 1` entries, match arrays have
            // `block_len` entries, and matcher clips all lengths to the block.
            unsafe {
                for pos in block_start..=block_end {
                    let i = pos - block_start;
                    if pos > block_start {
                        literal_run += 1;
                        if literal_run >= next_extra_at {
                            literal_cost += 1;
                            next_extra_at = literal_run + 255;
                        }
                        literal_cost += 1;
                        let current = *dp_ptr.add(i);
                        if current <= literal_cost {
                            literal_cost = current;
                            literal_run = 0;
                            next_extra_at = HEAVY_TOKEN_LIT_MAX;
                        }
                    }

                    let longest = if i < block_len {
                        *max_len_ptr.add(i) as usize
                    } else {
                        0
                    };
                    if longest >= MIN_MATCH_LEN {
                        let cost = literal_cost + 4;
                        let top_code = *HEAVY_CODE_FLOOR.get_unchecked(longest) as usize;
                        macro_rules! relax_len {
                            ($len:expr) => {{
                                let len = $len;
                                let target = i + len as usize;
                                let target_cost = dp_ptr.add(target);
                                if cost < *target_cost {
                                    *target_cost = cost;
                                    *arrival_len_ptr.add(target) = len;
                                }
                            }};
                        }

                        if top_code >= 1 {
                            relax_len!(4);
                        }
                        if top_code >= 2 {
                            relax_len!(5);
                        }
                        if top_code >= 3 {
                            relax_len!(6);
                        }
                        if top_code >= 4 {
                            relax_len!(7);
                        }
                        if top_code >= 5 {
                            relax_len!(8);
                        }
                        if top_code >= 6 {
                            relax_len!(9);
                        }
                        if top_code >= 7 {
                            relax_len!(10);
                        }
                        if top_code >= 8 {
                            relax_len!(11);
                        }
                        let mut code = 9usize;
                        if top_code >= 16 {
                            relax_len!(12);
                            relax_len!(13);
                            relax_len!(14);
                            relax_len!(15);
                            relax_len!(16);
                            relax_len!(17);
                            relax_len!(18);
                            relax_len!(19);
                            code = 17;
                        }
                        while code <= top_code {
                            let len = *HEAVY_LEN_OF.get_unchecked(code);
                            let target = i + len as usize;
                            let target_cost = dp_ptr.add(target);
                            if cost < *target_cost {
                                *target_cost = cost;
                                *arrival_len_ptr.add(target) = len;
                            }
                            code += 1;
                        }
                    }
                }
            }
        }

        #[cfg(feature = "paranoid")]
        for pos in block_start..=block_end {
            let i = pos - block_start;
            if pos > block_start {
                literal_run += 1;
                if literal_run >= next_extra_at {
                    literal_cost += 1;
                    next_extra_at = literal_run + 255;
                }
                literal_cost += 1;
                if dp[i] <= literal_cost {
                    literal_cost = dp[i];
                    literal_run = 0;
                    next_extra_at = HEAVY_TOKEN_LIT_MAX;
                }
            }

            let longest = if i < block_len {
                max_len[i] as usize
            } else {
                0
            };
            if longest >= MIN_MATCH_LEN {
                let cost = literal_cost + 4;
                let top_code = HEAVY_CODE_FLOOR[longest] as usize;
                for &len in HEAVY_LEN_OF.iter().take(top_code + 1).skip(1) {
                    let target = i + len as usize;
                    if cost < dp[target] {
                        dp[target] = cost;
                        arrival_len[target] = len;
                    }
                }
            }
        }

        let (commit, commit_cost) = if block_end < src_size {
            let commit = block_end - literal_run;
            (
                commit,
                literal_cost - literal_run - heavy_literal_extras(literal_run),
            )
        } else {
            let mut best_total = qstar_cost + (src_size - qstar);
            let mut best_commit = qstar;
            let mut best_commit_cost = qstar_cost;
            #[cfg(not(feature = "paranoid"))]
            {
                let dp_ptr = dp.as_ptr();
                // SAFETY: `pos - block_start` ranges over `0..=block_len`.
                unsafe {
                    for pos in block_start..=block_end {
                        let cost = *dp_ptr.add(pos - block_start);
                        if cost == HEAVY_DP_INF {
                            continue;
                        }
                        let total = cost + (src_size - pos);
                        if total < best_total {
                            best_total = total;
                            best_commit = pos;
                            best_commit_cost = cost;
                        }
                    }
                }
            }
            #[cfg(feature = "paranoid")]
            for pos in block_start..=block_end {
                let cost = dp[pos - block_start];
                if cost == HEAVY_DP_INF {
                    continue;
                }
                let total = cost + (src_size - pos);
                if total < best_total {
                    best_total = total;
                    best_commit = pos;
                    best_commit_cost = cost;
                }
            }
            (best_commit, best_commit_cost)
        };

        block_tokens.clear();
        let mut boundary = commit;
        #[cfg(not(feature = "paranoid"))]
        {
            let arrival_len_ptr = arrival_len.as_ptr();
            let dp_ptr = dp.as_ptr();
            let match_dis_ptr = match_dis.as_ptr();
            // SAFETY: backtracking only indexes committed block boundaries and
            // match starts previously written by the hot DP.
            unsafe {
                while boundary > qstar {
                    if boundary < block_start {
                        return 0;
                    }
                    let arrival_index = boundary - block_start;
                    let arrival_len = *arrival_len_ptr.add(arrival_index) as usize;
                    if arrival_len < MIN_MATCH_LEN {
                        return 0;
                    }
                    let Some(match_start) = boundary.checked_sub(arrival_len) else {
                        return 0;
                    };
                    if match_start < block_start {
                        return 0;
                    }
                    let Some(literal_cost) = (*dp_ptr.add(arrival_index)).checked_sub(4) else {
                        return 0;
                    };
                    let mut prev_boundary = match_start;
                    let mut found = false;
                    loop {
                        let run = match_start - prev_boundary;
                        let prev_cost = *dp_ptr.add(prev_boundary - block_start);
                        if prev_cost != HEAVY_DP_INF
                            && prev_cost + run + heavy_literal_extras(run) == literal_cost
                        {
                            found = true;
                            break;
                        }
                        if prev_boundary == block_start {
                            break;
                        }
                        prev_boundary -= 1;
                    }
                    if !found {
                        let run = match_start - qstar;
                        if qstar <= block_start
                            && qstar_cost + run + heavy_literal_extras(run) == literal_cost
                        {
                            prev_boundary = qstar;
                        } else {
                            return 0;
                        }
                    }
                    let origin_index = match_start - block_start;
                    block_tokens.push(TokenRecord {
                        match_start,
                        len: *arrival_len_ptr.add(arrival_index),
                        dis: *match_dis_ptr.add(origin_index),
                    });
                    boundary = prev_boundary;
                }
            }
        }
        #[cfg(feature = "paranoid")]
        while boundary > qstar {
            if boundary < block_start {
                return 0;
            }
            let arrival_index = boundary - block_start;
            let matched_len = arrival_len[arrival_index] as usize;
            if matched_len < MIN_MATCH_LEN {
                return 0;
            };
            let Some(match_start) = boundary.checked_sub(matched_len) else {
                return 0;
            };
            if match_start < block_start {
                return 0;
            }
            let Some(literal_cost) = dp[arrival_index].checked_sub(4) else {
                return 0;
            };
            let mut prev_boundary = match_start;
            let mut found = false;
            loop {
                let run = match_start - prev_boundary;
                let prev_cost = dp[prev_boundary - block_start];
                if prev_cost != HEAVY_DP_INF
                    && prev_cost + run + heavy_literal_extras(run) == literal_cost
                {
                    found = true;
                    break;
                }
                if prev_boundary == block_start {
                    break;
                }
                prev_boundary -= 1;
            }
            if !found {
                let run = match_start - qstar;
                if qstar <= block_start
                    && qstar_cost + run + heavy_literal_extras(run) == literal_cost
                {
                    prev_boundary = qstar;
                } else {
                    return 0;
                }
            }
            let origin_index = match_start - block_start;
            block_tokens.push(TokenRecord {
                match_start,
                len: matched_len as u8,
                dis: match_dis[origin_index],
            });
            boundary = prev_boundary;
        }

        for token in block_tokens.iter().rev() {
            let lit_len = token.match_start - lit;
            let token_len = token.len as usize;
            let code = HEAVY_CODE_FLOOR[token_len] as usize;
            debug_assert_eq!(HEAVY_LEN_OF[code] as usize, token_len);
            emit_heavy_token(
                dst,
                &mut dlpos,
                &mut drpos,
                src,
                lit,
                lit_len,
                code,
                token.dis as usize,
            );
            tokens += 1;
            long_matches += usize::from(token_len > VECTOR_WIDTH);
            lit = token.match_start + token_len;
        }

        qstar = commit;
        qstar_cost = commit_cost;
    }

    if drpos < dst_cap {
        let lit_data_len = dst_cap - drpos;
        dst.copy_within(drpos..dst_cap, dlpos);
        dlpos += lit_data_len;
    }

    let literal_suffix_cnt = src_size - lit;
    dst[literal_suffix_pos..literal_suffix_pos + 8]
        .copy_from_slice(&(literal_suffix_cnt as u64).to_le_bytes());
    dst[dlpos..dlpos + literal_suffix_cnt].copy_from_slice(&src[lit..]);
    dlpos += literal_suffix_cnt;

    let cond = tokens != 0
        && long_matches * HEAVY_COND_FLAG_THRESH_DEN < HEAVY_COND_FLAG_THRESH_NUM * tokens;
    let flags = (FLAG_HEAVY | if cond { FLAG_HEAVY_COND } else { 0 }) as u64;
    let header = (src_size as u64) | (flags << FLAG_SHIFT);
    dst[0..8].copy_from_slice(&header.to_le_bytes());

    dlpos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_empty() {
        let compressed = compress(b"");
        assert_eq!(&compressed[..8], &0u64.to_le_bytes());
        assert_eq!(compressed.len(), 8);
    }

    #[test]
    fn compress_small() {
        let data = b"hello";
        let compressed = compress(data);
        assert_eq!(&compressed[..8], &(data.len() as u64).to_le_bytes());
        assert_eq!(&compressed[8..], data);
    }

    #[test]
    fn compress_small_max() {
        let data = vec![0xAB; SMALL_LIM];
        let compressed = compress(&data);
        assert_eq!(&compressed[..8], &(data.len() as u64).to_le_bytes());
        assert_eq!(&compressed[8..], &data[..]);
    }

    #[test]
    fn compress_header_valid() {
        let data = vec![0x42; 200];
        let compressed = compress(&data);
        let size = u64::from_le_bytes(compressed[..8].try_into().unwrap()) as usize;
        assert_eq!(size, 200);
    }

    #[test]
    fn compress_bound_covers() {
        let data = vec![0x42; 200];
        let compressed = compress(&data);
        assert!(compressed.len() <= compress_bound(data.len()));
    }

    #[test]
    fn compress_bound_covers_level0() {
        let data = vec![0x42; 200];
        let compressed = compress_level(&data, 0).unwrap();
        assert!(compressed.len() <= compress_bound(data.len()));
    }

    #[test]
    fn compress_bound_covers_level4() {
        let data = vec![0x42; HEAVY_SMALL_LIM];
        let compressed = compress_level(&data, 4).unwrap();
        assert!(compressed.len() <= compress_bound(data.len()));
        assert!(compressed.len() <= compress_bound_level(data.len(), 4).unwrap());
    }

    #[test]
    fn invalid_level_is_rejected() {
        assert_eq!(
            compress_level(b"input", -2),
            Err(Error::InvalidLevel { level: -2 })
        );
        assert_eq!(
            compress_into_level(b"input", &mut [0; 64], 5),
            Err(Error::InvalidLevel { level: 5 })
        );
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn hash4_stays_inside_hash_table() {
        let val: u32 = kani::any();
        let hsh = hash4(val);
        assert!(hsh < HASH_SIZE);
    }

    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn recovered_match_position_keeps_vector_loads_in_bounds() {
        let src_len: usize = kani::any();
        let pos: usize = kani::any();
        let match_pos: usize = kani::any();

        kani::assume(src_len >= LITERAL_SUFFIX);
        let pos_after_match = pos.checked_add(MAX_MATCH_LEN);
        kani::assume(pos_after_match.is_some());
        kani::assume(pos_after_match.unwrap() <= src_len - LITERAL_SUFFIX);
        let match_after_distance = match_pos.checked_add(MIN_DISTANCE);
        kani::assume(match_after_distance.is_some());
        kani::assume(match_after_distance.unwrap() <= pos);
        kani::assume(pos - match_pos <= MAX_DISTANCE);

        let entry = match_pos as u16;
        let d = (pos as u16)
            .wrapping_sub(entry)
            .wrapping_sub((HASHTAB_LAG + 1) as u16);
        let recovered = pos.wrapping_sub(MAX_MATCH_LEN + 1).wrapping_sub(d as usize);

        assert!(recovered == match_pos);
        assert!(pos_after_match.unwrap() <= src_len);
        assert!(recovered.checked_add(MAX_MATCH_LEN).unwrap() <= src_len);
    }

    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn recovered_table_entry_is_always_safe_to_probe() {
        let src_len: usize = kani::any();
        let pos: usize = kani::any();
        let entry: u16 = kani::any();

        kani::assume(src_len >= LITERAL_SUFFIX);
        let pos_after_match = pos.checked_add(MAX_MATCH_LEN);
        kani::assume(pos_after_match.is_some());
        kani::assume(pos_after_match.unwrap() <= src_len - LITERAL_SUFFIX);
        kani::assume(pos >= MIN_DISTANCE);

        if pos < MAX_DISTANCE {
            kani::assume((entry as usize) <= pos - MIN_DISTANCE);
        }

        let d = (pos as u16)
            .wrapping_sub(entry)
            .wrapping_sub((HASHTAB_LAG + 1) as u16);
        let recovered = pos.wrapping_sub(MAX_MATCH_LEN + 1).wrapping_sub(d as usize);

        assert!(recovered + MIN_DISTANCE <= pos);
        assert!(pos - recovered <= MAX_DISTANCE);
        assert!(recovered.checked_add(MAX_MATCH_LEN).unwrap() <= src_len);
    }

    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn latest_batch_insert_reads_are_in_bounds() {
        let src_len: usize = kani::any();
        let pos: usize = kani::any();
        let hpos: usize = kani::any();
        let i: usize = kani::any();

        kani::assume(src_len >= LITERAL_SUFFIX);
        let pos_after_match = pos.checked_add(MAX_MATCH_LEN);
        kani::assume(pos_after_match.is_some());
        kani::assume(pos_after_match.unwrap() <= src_len - LITERAL_SUFFIX);
        let batch_ready = hpos.checked_add(HASHTAB_LAG + LATEST_BATCH);
        kani::assume(batch_ready.is_some());
        kani::assume(pos >= batch_ready.unwrap());
        kani::assume(i < LATEST_BATCH);

        let insert_pos = hpos + i;
        assert!(insert_pos.checked_add(8).unwrap() <= src_len);
    }

    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn ring_batch_insert_reads_are_in_bounds() {
        let src_len: usize = kani::any();
        let pos: usize = kani::any();
        let hpos: usize = kani::any();
        let i: usize = kani::any();

        kani::assume(src_len >= LITERAL_SUFFIX);
        let pos_after_match = pos.checked_add(MAX_MATCH_LEN);
        kani::assume(pos_after_match.is_some());
        kani::assume(pos_after_match.unwrap() <= src_len - LITERAL_SUFFIX);
        let batch_ready = hpos.checked_add(HASHTAB_LAG + RING_BATCH);
        kani::assume(batch_ready.is_some());
        kani::assume(pos >= batch_ready.unwrap());
        kani::assume(i < RING_BATCH);

        let insert_pos = hpos + i;
        assert!(insert_pos.checked_add(4).unwrap() <= src_len);
    }

    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn ring_cursor_stays_in_bounds() {
        let next: u8 = kani::any();
        let loose: bool = kani::any();
        let width = if loose {
            LOOSE_RING_WIDTH
        } else {
            KEEN_RING_WIDTH
        };
        kani::assume((next as usize) < width);

        let updated = if next as usize == width - 1 {
            0
        } else {
            next + 1
        };

        assert!((updated as usize) < width);
    }

    #[kani::proof]
    fn heavy_len_floor_tables_are_valid() {
        let len: u8 = kani::any();
        let len = len as usize;
        let floor = HEAVY_LEN_FLOOR[len] as usize;
        let code = HEAVY_CODE_FLOOR[len] as usize;

        assert!(floor <= len);
        assert!(code < HEAVY_LEN_OF.len());
        assert_eq!(HEAVY_LEN_OF[code] as usize, floor);
        if len >= MIN_MATCH_LEN {
            assert!(floor >= MIN_MATCH_LEN);
        }
    }
}
