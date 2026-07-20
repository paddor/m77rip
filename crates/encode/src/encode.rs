use m77rip_core::Error;
use m77rip_core::format::*;

#[cfg(not(feature = "paranoid"))]
use core::ptr;
#[cfg(not(feature = "paranoid"))]
use fearless_simd::{Simd, prelude::*, u8x32};

const HASH_SIZE: usize = 1 << 16;
const HASH_MUL: u32 = 2654435761;
const LATEST_BATCH: usize = 176;
const LATEST_INSERTS_PER_BATCH: usize = 8;
const RING_BATCH: usize = 8;
const RING_WIDTH: usize = 16;
const CHAIN_AFTER: usize = 8;
const SKIP_SHIFT: usize = 6;

const _: () = assert!(LATEST_INSERTS_PER_BATCH == 8);
const _: () = assert!(LATEST_INSERTS_PER_BATCH <= LATEST_BATCH);
const _: () = assert!(RING_WIDTH <= u8::MAX as usize + 1);

#[inline(always)]
fn hash4(val: u32) -> usize {
    (val.wrapping_mul(HASH_MUL) >> 16) as usize
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

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn read_u32_le_unchecked(src: &[u8], pos: usize) -> u32 {
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
fn lcp(src: &[u8], a: usize, b: usize) -> usize {
    let limit = MAX_MATCH_LEN.min(src.len().saturating_sub(a.max(b)));
    lcp_portable(src, a, b, limit)
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

/// Returns the maximum compressed size for a given input size.
pub fn compress_bound(src_size: usize) -> usize {
    if src_size <= SMALL_LIM {
        return HEADER_SIZE.saturating_add(src_size);
    }
    EXT_HEADER_SIZE
        .saturating_add(src_size)
        .saturating_add(src_size / 255)
        .saturating_add(16)
}

/// Compresses `input` into the misa77 stream format (level 1, default).
pub fn compress(input: &[u8]) -> Vec<u8> {
    let mut dst = vec![0u8; compress_bound(input.len())];
    let written = compress_dispatch(input, &mut dst, 1);
    dst.truncate(written);
    dst
}

/// Compresses `input` at the given level (-1 = fastest, 0 = fast,
/// 1 = default).
///
/// Returns [`Error::InvalidLevel`](m77rip_core::Error::InvalidLevel) for any
/// other level.
pub fn compress_level(input: &[u8], level: i8) -> Result<Vec<u8>, Error> {
    validate_level(level)?;
    let mut dst = vec![0u8; compress_bound(input.len())];
    let written = compress_dispatch(input, &mut dst, level);
    dst.truncate(written);
    Ok(dst)
}

/// Compresses `input` into `dst` (level 1, default).
///
/// Returns the number of bytes written to `dst`.
pub fn compress_into(input: &[u8], dst: &mut [u8]) -> Result<usize, Error> {
    let bound = compress_bound(input.len());
    if dst.len() < bound {
        return Err(Error::OutputTooSmall {
            need: bound,
            have: dst.len(),
        });
    }
    Ok(compress_dispatch(input, dst, 1))
}

/// Compresses `input` into `dst` at the given level.
///
/// Returns the number of bytes written to `dst`.
pub fn compress_into_level(input: &[u8], dst: &mut [u8], level: i8) -> Result<usize, Error> {
    validate_level(level)?;
    let bound = compress_bound(input.len());
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
    if (-1..=1).contains(&level) {
        Ok(())
    } else {
        Err(Error::InvalidLevel { level })
    }
}

#[cfg(not(feature = "paranoid"))]
fn compress_dispatch(src: &[u8], dst: &mut [u8], level: i8) -> usize {
    match level {
        -1 => compress_dispatch_level_speed(src, dst),
        0 => compress_dispatch_level0(src, dst),
        1 => compress_dispatch_level1(src, dst),
        _ => unreachable!(),
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(never)]
fn compress_dispatch_level_speed(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => speed_compress(simd, src, dst))
}

#[cfg(not(feature = "paranoid"))]
#[inline(never)]
fn compress_dispatch_level0(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => loose_compress(simd, src, dst))
}

#[cfg(not(feature = "paranoid"))]
#[inline(never)]
fn compress_dispatch_level1(src: &[u8], dst: &mut [u8]) -> usize {
    let level_obj = fearless_simd::Level::new();
    fearless_simd::dispatch!(level_obj, simd => default_compress(simd, src, dst))
}

#[cfg(feature = "paranoid")]
fn compress_dispatch(src: &[u8], dst: &mut [u8], level: i8) -> usize {
    match level {
        -1 => speed_compress(src, dst),
        0 => loose_compress(src, dst),
        1 => default_compress(src, dst),
        _ => unreachable!(),
    }
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

    #[cfg(feature = "paranoid")]
    #[inline(always)]
    fn insert(&mut self, hsh: usize, pos: usize) {
        self.entries[hsh] = pos as u16;
    }

    #[cfg(feature = "paranoid")]
    #[inline(always)]
    fn recover_pos(&self, hsh: usize, pos: usize) -> usize {
        recover_entry_pos(self.entries[hsh], pos)
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    unsafe fn insert_unchecked(&mut self, hsh: usize, pos: usize) {
        debug_assert!(hsh < HASH_SIZE);
        unsafe {
            *self.entries.get_unchecked_mut(hsh) = pos as u16;
        }
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    unsafe fn recover_pos_unchecked(&self, hsh: usize, pos: usize) -> usize {
        debug_assert!(hsh < HASH_SIZE);
        let entry = unsafe { *self.entries.get_unchecked(hsh) };
        recover_entry_pos(entry, pos)
    }
}

#[derive(Clone, Copy)]
struct RingBucket {
    entries: [u16; RING_WIDTH],
    next: u8,
}

impl RingBucket {
    fn new() -> Self {
        Self {
            entries: [0u16; RING_WIDTH],
            next: 0,
        }
    }
}

struct RingHashTable {
    buckets: Box<[RingBucket]>,
}

impl RingHashTable {
    fn new() -> Self {
        Self {
            buckets: vec![RingBucket::new(); HASH_SIZE].into_boxed_slice(),
        }
    }

    #[cfg(feature = "paranoid")]
    #[inline(always)]
    fn insert(&mut self, hsh: usize, pos: usize) {
        let bucket = &mut self.buckets[hsh];
        let next = bucket.next as usize;
        bucket.entries[next] = pos as u16;
        bucket.next = if next == RING_WIDTH - 1 {
            0
        } else {
            (next + 1) as u8
        };
    }

    #[cfg(feature = "paranoid")]
    #[inline(always)]
    fn bucket(&self, hsh: usize) -> &RingBucket {
        &self.buckets[hsh]
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    unsafe fn insert_unchecked(&mut self, hsh: usize, pos: usize) {
        debug_assert!(hsh < HASH_SIZE);
        let bucket = unsafe { self.buckets.get_unchecked_mut(hsh) };
        let next = bucket.next as usize;
        debug_assert!(next < RING_WIDTH);
        unsafe {
            *bucket.entries.get_unchecked_mut(next) = pos as u16;
        }
        bucket.next = if next == RING_WIDTH - 1 {
            0
        } else {
            (next + 1) as u8
        };
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    unsafe fn bucket_unchecked(&self, hsh: usize) -> &RingBucket {
        debug_assert!(hsh < HASH_SIZE);
        unsafe { self.buckets.get_unchecked(hsh) }
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn find_latest_match<S: Simd>(
    simd: S,
    src: &[u8],
    ht: &LatestHashTable,
    pos: usize,
) -> (usize, usize) {
    debug_assert!(pos > HASHTAB_LAG);
    debug_assert!(pos + MAX_MATCH_LEN <= src.len());
    let cur = paranoid_unsafe_call!(read_u32_le_unchecked(src, pos));
    let hsh = hash4(cur);
    let lst = paranoid_unsafe_call!(ht.recover_pos_unchecked(hsh, pos));
    debug_assert!(lst + MAX_MATCH_LEN <= src.len());

    if paranoid_unsafe_call!(read_u32_le_unchecked(src, lst)) != cur {
        return (0, lst);
    }

    let cur_next = paranoid_unsafe_call!(read_u32_le_unchecked(src, pos + 4));
    let lst_next = paranoid_unsafe_call!(read_u32_le_unchecked(src, lst + 4));
    let diff = cur_next ^ lst_next;
    if diff != 0 {
        return (MIN_MATCH_LEN + (diff.trailing_zeros() as usize >> 3), lst);
    }

    let reg = paranoid_unsafe_call!(load_u8x32_unchecked(simd, src, pos));
    let ireg = paranoid_unsafe_call!(load_u8x32_unchecked(simd, src, lst));
    (lcp_loaded(reg, ireg), lst)
}

#[cfg(feature = "paranoid")]
fn find_latest_match(src: &[u8], ht: &LatestHashTable, pos: usize) -> (usize, usize) {
    let cur = read_u32_le(src, pos);
    let hsh = hash4(cur);
    let lst = ht.recover_pos(hsh, pos);
    if read_u32_le(src, lst) != cur {
        return (0, lst);
    }
    let diff = read_u32_le(src, pos + 4) ^ read_u32_le(src, lst + 4);
    if diff != 0 {
        return (MIN_MATCH_LEN + (diff.trailing_zeros() as usize >> 3), lst);
    }
    (lcp(src, pos, lst), lst)
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn find_ring_match<S: Simd>(simd: S, src: &[u8], ht: &RingHashTable, pos: usize) -> (usize, usize) {
    debug_assert!(pos > HASHTAB_LAG);
    debug_assert!(pos + MAX_MATCH_LEN <= src.len());

    let hsh = hash4(paranoid_unsafe_call!(read_u32_le_unchecked(src, pos)));
    let reg = paranoid_unsafe_call!(load_u8x32_unchecked(simd, src, pos));
    let bucket = paranoid_unsafe_call!(ht.bucket_unchecked(hsh));

    let mut lst = 0;
    let mut match_len = 0;

    macro_rules! probe {
        ($i:literal) => {{
            let ilst = recover_entry_pos(bucket.entries[$i], pos);
            let ireg = paranoid_unsafe_call!(load_u8x32_unchecked(simd, src, ilst));
            let imatch_len = lcp_loaded(reg, ireg);
            if imatch_len > match_len {
                lst = ilst;
                match_len = imatch_len;
            }
        }};
    }

    probe!(0);
    probe!(1);
    probe!(2);
    probe!(3);
    probe!(4);
    probe!(5);
    probe!(6);
    probe!(7);
    probe!(8);
    probe!(9);
    probe!(10);
    probe!(11);
    probe!(12);
    probe!(13);
    probe!(14);
    probe!(15);

    (match_len, lst)
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn find_ring_match(src: &[u8], ht: &RingHashTable, pos: usize) -> (usize, usize) {
    let hsh = hash4(read_u32_le(src, pos));
    let bucket = ht.bucket(hsh);
    let mut lst = 0;
    let mut match_len = 0;

    for &entry in &bucket.entries {
        let ilst = recover_entry_pos(entry, pos);
        let imatch_len = lcp(src, pos, ilst);
        if imatch_len > match_len {
            lst = ilst;
            match_len = imatch_len;
        }
    }

    (match_len, lst)
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn batch_insert_latest(
    src: &[u8],
    ht: &mut LatestHashTable,
    hpos: &mut usize,
    pos: usize,
    sparse: bool,
) {
    while pos >= *hpos + HASHTAB_LAG + LATEST_BATCH {
        macro_rules! insert {
            ($i:literal) => {{
                let insert_pos = *hpos + $i;
                let hsh = hash4(paranoid_unsafe_call!(read_u32_le_unchecked(
                    src, insert_pos
                )));
                paranoid_unsafe_call!(ht.insert_unchecked(hsh, insert_pos));
            }};
        }

        insert!(0);
        if !sparse {
            insert!(1);
            insert!(2);
            insert!(3);
            insert!(4);
            insert!(5);
            insert!(6);
            insert!(7);
        }
        *hpos += LATEST_BATCH;
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn batch_insert_latest(
    src: &[u8],
    ht: &mut LatestHashTable,
    hpos: &mut usize,
    pos: usize,
    sparse: bool,
) {
    while pos >= *hpos + HASHTAB_LAG + LATEST_BATCH {
        let insert_pos = *hpos;
        let hsh = hash4(read_u32_le(src, insert_pos));
        ht.insert(hsh, insert_pos);

        if !sparse {
            for i in 1..LATEST_INSERTS_PER_BATCH {
                let insert_pos = *hpos + i;
                let hsh = hash4(read_u32_le(src, insert_pos));
                ht.insert(hsh, insert_pos);
            }
        }
        *hpos += LATEST_BATCH;
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn batch_insert_ring(src: &[u8], ht: &mut RingHashTable, hpos: &mut usize, pos: usize) {
    while pos >= *hpos + HASHTAB_LAG + RING_BATCH {
        macro_rules! insert {
            ($i:literal) => {{
                let insert_pos = *hpos + $i;
                let hsh = hash4(paranoid_unsafe_call!(read_u32_le_unchecked(
                    src, insert_pos
                )));
                paranoid_unsafe_call!(ht.insert_unchecked(hsh, insert_pos));
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

#[cfg(feature = "paranoid")]
#[inline(always)]
fn batch_insert_ring(src: &[u8], ht: &mut RingHashTable, hpos: &mut usize, pos: usize) {
    while pos >= *hpos + HASHTAB_LAG + RING_BATCH {
        for i in 0..RING_BATCH {
            let insert_pos = *hpos + i;
            let hsh = hash4(read_u32_le(src, insert_pos));
            ht.insert(hsh, insert_pos);
        }
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

#[inline(always)]
fn emit_match_token(dst: &mut [u8], dlpos: &mut usize, match_len: usize, dis: usize) {
    let norm_match = match_len - (MIN_MATCH_LEN - 1);
    dst[*dlpos] = norm_match as u8;
    *dlpos += 1;

    let dbytes = (dis - MIN_DISTANCE) as u16;
    dst[*dlpos..*dlpos + 2].copy_from_slice(&dbytes.to_le_bytes());
    *dlpos += 2;
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn speed_compress<S: Simd>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    speed_compress_impl(simd, src, dst)
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn speed_compress(src: &[u8], dst: &mut [u8]) -> usize {
    speed_compress_impl(src, dst)
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn speed_compress_impl<S: Simd>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
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

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_latest(src, &mut ht, &mut hpos, pos, false);

        let (match_len, lst) = if pos > HASHTAB_LAG {
            find_latest_match(simd, src, &ht, pos)
        } else {
            (0, 0)
        };

        if match_len < MIN_MATCH_LEN {
            pos += 1 + (miss_run >> SKIP_SHIFT);
            miss_run += 1;
            continue;
        }

        miss_run = 0;
        let lit_len = pos - lit;
        let dis = pos - lst;

        emit_token(
            dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
        );

        pos += match_len;
        lit = pos;

        if match_len >= CHAIN_AFTER {
            loop {
                if pos + MAX_MATCH_LEN > match_end_limit {
                    break;
                }

                batch_insert_latest(src, &mut ht, &mut hpos, pos, false);
                let (chain_match_len, chain_lst) = if pos > HASHTAB_LAG {
                    find_latest_match(simd, src, &ht, pos)
                } else {
                    (0, 0)
                };

                if chain_match_len < MIN_MATCH_LEN {
                    break;
                }

                let dis = pos - chain_lst;
                emit_match_token(dst, &mut dlpos, chain_match_len, dis);

                pos += chain_match_len;
                lit = pos;
            }
        }
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

    dlpos
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn speed_compress_impl(src: &[u8], dst: &mut [u8]) -> usize {
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

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_latest(src, &mut ht, &mut hpos, pos, false);

        let (match_len, lst) = if pos > HASHTAB_LAG {
            find_latest_match(src, &ht, pos)
        } else {
            (0, 0)
        };

        if match_len < MIN_MATCH_LEN {
            pos += 1 + (miss_run >> SKIP_SHIFT);
            miss_run += 1;
            continue;
        }

        miss_run = 0;
        let lit_len = pos - lit;
        let dis = pos - lst;

        emit_token(
            dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
        );

        pos += match_len;
        lit = pos;

        if match_len >= CHAIN_AFTER {
            loop {
                if pos + MAX_MATCH_LEN > match_end_limit {
                    break;
                }

                batch_insert_latest(src, &mut ht, &mut hpos, pos, false);
                let (chain_match_len, chain_lst) = if pos > HASHTAB_LAG {
                    find_latest_match(src, &ht, pos)
                } else {
                    (0, 0)
                };

                if chain_match_len < MIN_MATCH_LEN {
                    break;
                }

                let dis = pos - chain_lst;
                emit_match_token(dst, &mut dlpos, chain_match_len, dis);

                pos += chain_match_len;
                lit = pos;
            }
        }
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

    dlpos
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn default_compress<S: Simd>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    default_compress_impl(simd, src, dst)
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn default_compress(src: &[u8], dst: &mut [u8]) -> usize {
    default_compress_impl(src, dst)
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn default_compress_impl<S: Simd>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    const LOOKAHEAD: usize = 2;
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

    let mut ht = RingHashTable::new();
    let mut pos: usize = 0;
    let mut hpos: usize = 0;
    let mut lit: usize = 0;
    let mut miss_run: usize = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_ring(src, &mut ht, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_ring_match(simd, src, &ht, pos)
        } else {
            (0, 0)
        };

        if match_len >= MIN_MATCH_LEN {
            let base_pos = pos;
            let mut npos = base_pos + 1;
            while npos <= base_pos + LOOKAHEAD
                && npos + MAX_MATCH_LEN <= match_end_limit
                && match_len < LA_GATE
            {
                let (nmatch_len, nlst) = find_ring_match(simd, src, &ht, npos);
                if nmatch_len > match_len {
                    pos = npos;
                    lst = nlst;
                    match_len = nmatch_len;
                } else if match_len >= LA_PATE {
                    break;
                }
                npos += 1;
            }

            let lit_len = pos - lit;
            let dis = pos - lst;

            emit_token(
                dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
            );

            pos += match_len;
            lit = pos;
            miss_run = 0;
        } else {
            pos += 1 + (miss_run >> SKIP_SHIFT);
            miss_run += 1;
        }
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

    dlpos
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn default_compress_impl(src: &[u8], dst: &mut [u8]) -> usize {
    const LOOKAHEAD: usize = 2;
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

    let mut ht = RingHashTable::new();
    let mut pos: usize = 0;
    let mut hpos: usize = 0;
    let mut lit: usize = 0;
    let mut miss_run: usize = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_ring(src, &mut ht, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_ring_match(src, &ht, pos)
        } else {
            (0, 0)
        };

        if match_len >= MIN_MATCH_LEN {
            let base_pos = pos;
            let mut npos = base_pos + 1;
            while npos <= base_pos + LOOKAHEAD
                && npos + MAX_MATCH_LEN <= match_end_limit
                && match_len < LA_GATE
            {
                let (nmatch_len, nlst) = find_ring_match(src, &ht, npos);
                if nmatch_len > match_len {
                    pos = npos;
                    lst = nlst;
                    match_len = nmatch_len;
                } else if match_len >= LA_PATE {
                    break;
                }
                npos += 1;
            }

            let lit_len = pos - lit;
            let dis = pos - lst;

            emit_token(
                dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
            );

            pos += match_len;
            lit = pos;
            miss_run = 0;
        } else {
            pos += 1 + (miss_run >> SKIP_SHIFT);
            miss_run += 1;
        }
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

    dlpos
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn loose_compress<S: Simd>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    loose_compress_impl(simd, src, dst)
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn loose_compress(src: &[u8], dst: &mut [u8]) -> usize {
    loose_compress_impl(src, dst)
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn loose_compress_impl<S: Simd>(simd: S, src: &[u8], dst: &mut [u8]) -> usize {
    const ACCEPT_LEN: usize = 7;
    const FIRE_AT: usize = 4;

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

    let mut ht = RingHashTable::new();
    let mut pos: usize = 0;
    let mut hpos: usize = 0;
    let mut lit: usize = 0;
    let mut miss_run: usize = 0;

    let mut cand_pos: usize = 0;
    let mut cand_len: usize = 0;
    let mut cand_lst: usize = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_ring(src, &mut ht, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_ring_match(simd, src, &ht, pos)
        } else {
            (0, 0)
        };

        let pos_safe_bound = pos;
        let pend = pos - lit;
        let mut accept = match_len >= ACCEPT_LEN;

        if !accept {
            if pend == FIRE_AT {
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

        emit_token(
            dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
        );

        pos += match_len;
        lit = pos;
        pos = pos.max(pos_safe_bound);
        cand_len = 0;

        loop {
            if pos + MAX_MATCH_LEN > match_end_limit {
                break;
            }

            batch_insert_ring(src, &mut ht, &mut hpos, pos);
            let (chain_match_len, chain_lst) = if pos > HASHTAB_LAG {
                find_ring_match(simd, src, &ht, pos)
            } else {
                (0, 0)
            };

            if chain_match_len < MIN_MATCH_LEN {
                break;
            }

            let dis = pos - chain_lst;
            emit_match_token(dst, &mut dlpos, chain_match_len, dis);

            pos += chain_match_len;
            lit = pos;
        }
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

    dlpos
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn loose_compress_impl(src: &[u8], dst: &mut [u8]) -> usize {
    const ACCEPT_LEN: usize = 7;
    const FIRE_AT: usize = 4;

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

    let mut ht = RingHashTable::new();
    let mut pos: usize = 0;
    let mut hpos: usize = 0;
    let mut lit: usize = 0;
    let mut miss_run: usize = 0;

    let mut cand_pos: usize = 0;
    let mut cand_len: usize = 0;
    let mut cand_lst: usize = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_ring(src, &mut ht, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_ring_match(src, &ht, pos)
        } else {
            (0, 0)
        };

        let pos_safe_bound = pos;
        let pend = pos - lit;
        let mut accept = match_len >= ACCEPT_LEN;

        if !accept {
            if pend == FIRE_AT {
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

        emit_token(
            dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
        );

        pos += match_len;
        lit = pos;
        pos = pos.max(pos_safe_bound);
        cand_len = 0;

        loop {
            if pos + MAX_MATCH_LEN > match_end_limit {
                break;
            }

            batch_insert_ring(src, &mut ht, &mut hpos, pos);
            let (chain_match_len, chain_lst) = if pos > HASHTAB_LAG {
                find_ring_match(src, &ht, pos)
            } else {
                (0, 0)
            };

            if chain_match_len < MIN_MATCH_LEN {
                break;
            }

            let dis = pos - chain_lst;
            emit_match_token(dst, &mut dlpos, chain_match_len, dis);

            pos += chain_match_len;
            lit = pos;
        }
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
    fn invalid_level_is_rejected() {
        assert_eq!(
            compress_level(b"input", 2),
            Err(Error::InvalidLevel { level: 2 })
        );
        assert_eq!(
            compress_into_level(b"input", &mut [0; 64], 2),
            Err(Error::InvalidLevel { level: 2 })
        );
        assert_eq!(
            compress_level(b"input", -2),
            Err(Error::InvalidLevel { level: -2 })
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
        kani::assume(i < LATEST_INSERTS_PER_BATCH);

        let insert_pos = hpos + i;
        assert!(insert_pos.checked_add(4).unwrap() <= src_len);
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
        kani::assume((next as usize) < RING_WIDTH);

        let updated = if next as usize == RING_WIDTH - 1 {
            0
        } else {
            next + 1
        };

        assert!((updated as usize) < RING_WIDTH);
    }
}
