use crate::sais::SaisWorkspace;
use m77rip_core::Error;
use m77rip_core::format::*;

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
const HEAVY_BLOCK_SIZE: usize = 1 << 21;
const HEAVY_PAD_LEN: usize = HEAVY_MAX_MATCH_LEN + 1;
const HEAVY_DP_INF: usize = usize::MAX;
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

#[inline(always)]
fn read_u32_le(src: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(src[pos..pos + 4].try_into().unwrap())
}

#[inline(always)]
fn read_u64_le(src: &[u8], pos: usize) -> u64 {
    u64::from_le_bytes(src[pos..pos + 8].try_into().unwrap())
}

#[inline(always)]
fn load_u8x32_checked<S: Simd>(simd: S, src: &[u8], pos: usize) -> u8x32<S> {
    let lanes: &[u8; VECTOR_WIDTH] = src[pos..pos + VECTOR_WIDTH].try_into().unwrap();
    simd.load_array_ref_u8x32(lanes)
}

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

#[inline(never)]
fn compress_dispatch_level_blitz(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => blitz_compress(simd, src, dst))
}

#[inline(never)]
fn compress_dispatch_level_swift(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => swift_compress(simd, src, dst))
}

#[inline(never)]
fn compress_dispatch_level_loose(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => loose_compress(simd, src, dst))
}

#[inline(never)]
fn compress_dispatch_level_keen(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => keen_compress(simd, src, dst))
}

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
}

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
    let cur = read_u32_le(src, pos);
    let hsh = hash6(read_u64_le(src, pos));
    let lst = ht.recover_pos(hsh, pos);
    debug_assert!(lst + MAX_MATCH_LEN <= src.len());

    if read_u32_le(src, lst) != cur {
        return (0, lst);
    }

    let reg = load_u8x32(simd, src, pos);
    let ireg = load_u8x32(simd, src, lst);
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
    let cur = read_u32_le(src, pos);
    let mut lst = ht6.recover_pos(hash6(read_u64_le(src, pos)), pos);
    let mut hit = read_u32_le(src, lst) == cur;

    if !hit && cand_len == 0 {
        lst = ht4.recover_pos(hash4(cur), pos);
        hit = read_u32_le(src, lst) == cur;
    }

    if !hit {
        return (0, lst);
    }

    let reg = load_u8x32(simd, src, pos);
    let ireg = load_u8x32(simd, src, lst);
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

    let hsh = hash4(read_u32_le(src, pos));
    let reg = load_u8x32(simd, src, pos);
    let bucket = ht.bucket(hsh);
    let mut lst = 0;
    let mut match_len = 0;

    for &entry in &bucket.entries {
        let ilst = recover_entry_pos(entry, pos);
        let ireg = load_u8x32(simd, src, ilst);
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
                let hsh = hash6(read_u64_le(src, insert_pos));
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
                ht6.insert(hash6(read_u64_le(src, insert_pos)), insert_pos);
                if insert_pos & 1 == 0 {
                    let val = read_u32_le(src, insert_pos);
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
                let hsh = hash4(read_u32_le(src, insert_pos));
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

struct LightWorkspace {
    sorter: SaisWorkspace,
    sa: Vec<i32>,
    rank: Vec<u32>,
    live: HeavyLiveSet,
    max_len: Vec<u8>,
    match_dis: Vec<u32>,
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
        }
    }
}

#[cfg(feature = "std")]
std::thread_local! {
    static LIGHT_WORKSPACE: core::cell::RefCell<LightWorkspace> =
        core::cell::RefCell::new(LightWorkspace::new());
}

#[cfg(feature = "std")]
std::thread_local! {
    static HEAVY_WORKSPACE: core::cell::RefCell<HeavyWorkspace> =
        core::cell::RefCell::new(HeavyWorkspace::new());
}

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
    {
        let mut lcp_at = |src: &[u8], a: usize, b: usize, limit: usize| {
            let _ = simd;
            lcp_heavy(src, a, b, limit)
        };
        light_optimal_body(
            workspace,
            src,
            dst,
            |src,
             sa,
             rank,
             live,
             max_len,
             match_dis,
             block_start,
             block_end,
             seg_start,
             hard_end| {
                light_find_matches(
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
    } = workspace;

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

        if max_len.len() != block_len {
            max_len.resize(block_len, 0);
        }
        if match_dis.len() != block_len {
            match_dis.resize(block_len, 0);
        }

        let hard_end = block_end.min(match_end_limit);
        let init_limit = if block_start >= MIN_DISTANCE && block_start - MIN_DISTANCE >= seg_start {
            u32::try_from(block_start - MIN_DISTANCE - seg_start + 1).unwrap()
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

        let mut pos = block_start;
        while pos < hard_end {
            let i = pos - block_start;
            let len = max_len[i] as usize;
            if len >= MIN_MATCH_LEN {
                if pos + 1 < hard_end {
                    let next_len = max_len[i + 1] as usize;
                    if next_len > len {
                        pos += 1;
                        continue;
                    }
                }
                emit_token(
                    dst,
                    &mut dlpos,
                    &mut drpos,
                    src,
                    lit,
                    pos - lit,
                    len,
                    match_dis[i] as usize,
                );
                pos += len;
                lit = pos;
            } else {
                pos += 1;
            }
        }
    }

    finish_light(src, dst, dst_cap, dlpos, drpos, literal_suffix_pos, lit)
}

#[inline(always)]
fn heavy_compress(src: &[u8], dst: &mut [u8]) -> usize {
    #[cfg(feature = "std")]
    {
        HEAVY_WORKSPACE.with(|cell| match cell.try_borrow_mut() {
            Ok(mut workspace) => heavy_compress_with_workspace(&mut workspace, src, dst),
            Err(_) => {
                let mut workspace = HeavyWorkspace::new();
                heavy_compress_with_workspace(&mut workspace, src, dst)
            }
        })
    }

    #[cfg(not(feature = "std"))]
    {
        let mut workspace = HeavyWorkspace::new();
        heavy_compress_with_workspace(&mut workspace, src, dst)
    }
}

#[inline(always)]
fn heavy_compress_with_workspace(
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
