use m77rip_core::Error;
use m77rip_core::format::*;

#[cfg(not(feature = "paranoid"))]
use core::ptr;
#[cfg(not(feature = "paranoid"))]
use fearless_simd::{Simd, prelude::*, u8x32};

const HASH_SIZE: usize = 1 << 16;
const HASH_MUL: u32 = 2654435761;
const HASHTAB_WID: usize = 16;
const BATCH: usize = 8;
const LOOKAHEAD: usize = 2;
const LA_GATE: usize = 16;
const LA_PATE: usize = 8;
const SKIP_SHIFT: usize = 6;

#[inline(always)]
fn hash4(val: u32) -> usize {
    (val.wrapping_mul(HASH_MUL) >> 16) as usize
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

/// Compresses `input` at the given level (0 = fast, 1 = default).
///
/// Returns [`Error::InvalidLevel`](m77rip_core::Error::InvalidLevel) for any
/// other level.
pub fn compress_level(input: &[u8], level: u8) -> Result<Vec<u8>, Error> {
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
pub fn compress_into_level(input: &[u8], dst: &mut [u8], level: u8) -> Result<usize, Error> {
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
fn validate_level(level: u8) -> Result<(), Error> {
    if level <= 1 {
        Ok(())
    } else {
        Err(Error::InvalidLevel { level })
    }
}

#[cfg(not(feature = "paranoid"))]
fn compress_dispatch(src: &[u8], dst: &mut [u8], level: u8) -> usize {
    let level_obj = fearless_simd::Level::new();
    match level {
        0 => fearless_simd::dispatch!(level_obj, simd => loose_compress(simd, src, dst)),
        _ => fearless_simd::dispatch!(level_obj, simd => default_compress(simd, src, dst)),
    }
}

#[cfg(feature = "paranoid")]
fn compress_dispatch(src: &[u8], dst: &mut [u8], level: u8) -> usize {
    match level {
        0 => loose_compress(src, dst),
        _ => default_compress(src, dst),
    }
}

struct HashTable {
    entries: Vec<[u16; HASHTAB_WID]>,
    indices: Vec<u8>,
}

impl HashTable {
    fn new() -> Self {
        Self {
            entries: vec![[0u16; HASHTAB_WID]; HASH_SIZE],
            indices: vec![0u8; HASH_SIZE],
        }
    }

    #[cfg(feature = "paranoid")]
    #[inline(always)]
    fn insert(&mut self, hsh: usize, pos: usize) {
        let idx = self.indices[hsh] as usize;
        self.entries[hsh][idx] = pos as u16;
        self.indices[hsh] = if idx == HASHTAB_WID - 1 {
            0
        } else {
            (idx + 1) as u8
        };
    }

    #[cfg(feature = "paranoid")]
    #[inline(always)]
    fn recover_pos(&self, hsh: usize, entry_idx: usize, pos: usize) -> usize {
        let d = (pos as u16)
            .wrapping_sub(self.entries[hsh][entry_idx])
            .wrapping_sub((HASHTAB_LAG + 1) as u16);
        pos.wrapping_sub(MAX_MATCH_LEN + 1).wrapping_sub(d as usize)
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    unsafe fn insert_unchecked(&mut self, hsh: usize, pos: usize) {
        debug_assert!(hsh < HASH_SIZE);
        let idx = unsafe { *self.indices.get_unchecked(hsh) } as usize;
        debug_assert!(idx < HASHTAB_WID);
        unsafe {
            *self.entries.get_unchecked_mut(hsh).get_unchecked_mut(idx) = pos as u16;
            *self.indices.get_unchecked_mut(hsh) = if idx == HASHTAB_WID - 1 {
                0
            } else {
                (idx + 1) as u8
            };
        }
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    unsafe fn recover_pos_unchecked(&self, hsh: usize, entry_idx: usize, pos: usize) -> usize {
        debug_assert!(hsh < HASH_SIZE);
        debug_assert!(entry_idx < HASHTAB_WID);
        let entry = unsafe { *self.entries.get_unchecked(hsh).get_unchecked(entry_idx) };
        let d = (pos as u16)
            .wrapping_sub(entry)
            .wrapping_sub((HASHTAB_LAG + 1) as u16);
        pos.wrapping_sub(MAX_MATCH_LEN + 1).wrapping_sub(d as usize)
    }
}

struct LatestHashTable {
    entries: Vec<u16>,
}

impl LatestHashTable {
    fn new() -> Self {
        Self {
            entries: vec![0u16; HASH_SIZE],
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
        let d = (pos as u16)
            .wrapping_sub(self.entries[hsh])
            .wrapping_sub((HASHTAB_LAG + 1) as u16);
        pos.wrapping_sub(MAX_MATCH_LEN + 1).wrapping_sub(d as usize)
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
        let d = (pos as u16)
            .wrapping_sub(entry)
            .wrapping_sub((HASHTAB_LAG + 1) as u16);
        pos.wrapping_sub(MAX_MATCH_LEN + 1).wrapping_sub(d as usize)
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn find_best_match16<S: Simd>(simd: S, src: &[u8], ht: &HashTable, pos: usize) -> (usize, usize) {
    debug_assert!(pos > HASHTAB_LAG);
    debug_assert!(pos + MAX_MATCH_LEN <= src.len());
    let hsh = hash4(paranoid_unsafe_call!(read_u32_le_unchecked(src, pos)));
    let reg = paranoid_unsafe_call!(load_u8x32_unchecked(simd, src, pos));
    let mut best_len: usize = 0;
    let mut best_src: usize = 0;

    macro_rules! probe {
        ($i:literal) => {{
            let ilst = paranoid_unsafe_call!(ht.recover_pos_unchecked(hsh, $i, pos));
            debug_assert!(ilst + MAX_MATCH_LEN <= src.len());
            let ireg = paranoid_unsafe_call!(load_u8x32_unchecked(simd, src, ilst));
            let m = lcp_loaded(reg, ireg);
            if m > best_len {
                best_len = m;
                best_src = ilst;
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

    (best_len, best_src)
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
    let hsh = hash4(paranoid_unsafe_call!(read_u32_le_unchecked(src, pos)));
    let reg = paranoid_unsafe_call!(load_u8x32_unchecked(simd, src, pos));
    let lst = paranoid_unsafe_call!(ht.recover_pos_unchecked(hsh, pos));
    debug_assert!(lst + MAX_MATCH_LEN <= src.len());
    let ireg = paranoid_unsafe_call!(load_u8x32_unchecked(simd, src, lst));
    (lcp_loaded(reg, ireg), lst)
}

#[cfg(feature = "paranoid")]
fn find_best_match(src: &[u8], ht: &HashTable, pos: usize) -> (usize, usize) {
    let hsh = hash4(read_u32_le(src, pos));
    let mut best_len: usize = 0;
    let mut best_src: usize = 0;

    for i in 0..HASHTAB_WID {
        let ilst = ht.recover_pos(hsh, i, pos);
        let m = lcp(src, pos, ilst);
        if m > best_len {
            best_len = m;
            best_src = ilst;
        }
    }
    (best_len, best_src)
}

#[cfg(feature = "paranoid")]
fn find_latest_match(src: &[u8], ht: &LatestHashTable, pos: usize) -> (usize, usize) {
    let hsh = hash4(read_u32_le(src, pos));
    let lst = ht.recover_pos(hsh, pos);
    (lcp(src, pos, lst), lst)
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn batch_insert(src: &[u8], ht: &mut HashTable, hpos: &mut usize, pos: usize) {
    while pos >= *hpos + HASHTAB_LAG + BATCH {
        for i in 0..BATCH {
            let insert_pos = *hpos + i;
            let hsh = hash4(paranoid_unsafe_call!(read_u32_le_unchecked(
                src, insert_pos
            )));
            paranoid_unsafe_call!(ht.insert_unchecked(hsh, insert_pos));
        }
        *hpos += BATCH;
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn batch_insert_latest(src: &[u8], ht: &mut LatestHashTable, hpos: &mut usize, pos: usize) {
    while pos >= *hpos + HASHTAB_LAG + BATCH {
        for i in 0..BATCH {
            let insert_pos = *hpos + i;
            let hsh = hash4(paranoid_unsafe_call!(read_u32_le_unchecked(
                src, insert_pos
            )));
            paranoid_unsafe_call!(ht.insert_unchecked(hsh, insert_pos));
        }
        *hpos += BATCH;
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn batch_insert(src: &[u8], ht: &mut HashTable, hpos: &mut usize, pos: usize) {
    while pos >= *hpos + HASHTAB_LAG + BATCH {
        for i in 0..BATCH {
            let hsh = hash4(read_u32_le(src, *hpos + i));
            ht.insert(hsh, *hpos + i);
        }
        *hpos += BATCH;
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn batch_insert_latest(src: &[u8], ht: &mut LatestHashTable, hpos: &mut usize, pos: usize) {
    while pos >= *hpos + HASHTAB_LAG + BATCH {
        for i in 0..BATCH {
            let hsh = hash4(read_u32_le(src, *hpos + i));
            ht.insert(hsh, *hpos + i);
        }
        *hpos += BATCH;
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

    let mut ht = HashTable::new();
    let mut pos: usize = 0;
    let mut hpos: usize = 0;
    let mut lit: usize = 0;
    let mut miss_run: usize = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert(src, &mut ht, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_best_match16(simd, src, &ht, pos)
        } else {
            (0, 0)
        };

        if (MIN_MATCH_LEN..LA_GATE).contains(&match_len) {
            for npos in (pos + 1)..=(pos + LOOKAHEAD) {
                if npos + MAX_MATCH_LEN > match_end_limit || match_len >= LA_GATE {
                    break;
                }

                let (nmatch_len, nlst) = find_best_match16(simd, src, &ht, npos);

                if nmatch_len > match_len {
                    pos = npos;
                    match_len = nmatch_len;
                    lst = nlst;
                } else if match_len >= LA_PATE {
                    break;
                }
            }
        }

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

    let mut ht = HashTable::new();
    let mut pos: usize = 0;
    let mut hpos: usize = 0;
    let mut lit: usize = 0;
    let mut miss_run: usize = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert(src, &mut ht, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_best_match(src, &ht, pos)
        } else {
            (0, 0)
        };

        if (MIN_MATCH_LEN..LA_GATE).contains(&match_len) {
            for npos in (pos + 1)..=(pos + LOOKAHEAD) {
                if npos + MAX_MATCH_LEN > match_end_limit || match_len >= LA_GATE {
                    break;
                }

                let (nmatch_len, nlst) = find_best_match(src, &ht, npos);

                if nmatch_len > match_len {
                    pos = npos;
                    match_len = nmatch_len;
                    lst = nlst;
                } else if match_len >= LA_PATE {
                    break;
                }
            }
        }

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
    const FIRE_AT: usize = 6;
    const REGIME_CAP: i64 = 64;
    const REGIME_THRESHOLD: i64 = 32;

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
    let mut regime: i64 = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_latest(src, &mut ht, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_latest_match(simd, src, &ht, pos)
        } else {
            (0, 0)
        };

        let pos_safe_bound = pos;
        let mut accept = match_len >= ACCEPT_LEN;

        let pend = pos - lit;
        let fire = if regime >= REGIME_THRESHOLD {
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

        emit_token(
            dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
        );

        regime += if (7..=32).contains(&lit_len) { 2 } else { -1 };
        regime = regime.clamp(0, REGIME_CAP);

        pos += match_len;
        lit = pos;
        pos = pos.max(pos_safe_bound);
        cand_len = 0;
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
    const FIRE_AT: usize = 6;
    const REGIME_CAP: i64 = 64;
    const REGIME_THRESHOLD: i64 = 32;

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
    let mut regime: i64 = 0;

    while pos + MAX_MATCH_LEN <= match_end_limit {
        batch_insert_latest(src, &mut ht, &mut hpos, pos);

        let (mut match_len, mut lst) = if pos > HASHTAB_LAG {
            find_latest_match(src, &ht, pos)
        } else {
            (0, 0)
        };

        let pos_safe_bound = pos;
        let mut accept = match_len >= ACCEPT_LEN;

        let pend = pos - lit;
        let fire = if regime >= REGIME_THRESHOLD {
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

        emit_token(
            dst, &mut dlpos, &mut drpos, src, lit, lit_len, match_len, dis,
        );

        regime += if (7..=32).contains(&lit_len) { 2 } else { -1 };
        regime = regime.clamp(0, REGIME_CAP);

        pos += match_len;
        lit = pos;
        pos = pos.max(pos_safe_bound);
        cand_len = 0;
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
    fn batch_insert_reads_are_in_bounds() {
        let src_len: usize = kani::any();
        let pos: usize = kani::any();
        let hpos: usize = kani::any();
        let i: usize = kani::any();

        kani::assume(src_len >= LITERAL_SUFFIX);
        let pos_after_match = pos.checked_add(MAX_MATCH_LEN);
        kani::assume(pos_after_match.is_some());
        kani::assume(pos_after_match.unwrap() <= src_len - LITERAL_SUFFIX);
        let batch_ready = hpos.checked_add(HASHTAB_LAG + BATCH);
        kani::assume(batch_ready.is_some());
        kani::assume(pos >= batch_ready.unwrap());
        kani::assume(i < BATCH);

        let insert_pos = hpos + i;
        assert!(insert_pos.checked_add(4).unwrap() <= src_len);
    }
}
