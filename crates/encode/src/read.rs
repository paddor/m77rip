//! Input reads, hashes, and match length measurement.
//!
//! Reads borrow fixed-size arrays, so one bounds check covers every byte of
//! a word or a 32-byte vector compare.

use fearless_simd::{Simd, prelude::*, u8x32};
use m77rip_core::format::{MAX_MATCH_LEN, VECTOR_WIDTH};

/// Slots of a greedy hash table.
pub(crate) const HASH_SIZE: usize = 1 << 16;
const HASH_MUL: u32 = 2_654_435_761;
const HASH6_MUL: u64 = 0x9E37_79B1_85EB_CA87;
const HASH6_MASK: u64 = (1 << 48) - 1;

const _: () = assert!(VECTOR_WIDTH == MAX_MATCH_LEN);

/// Hash of 4 bytes.
#[inline(always)]
pub(crate) fn hash4(val: u32) -> usize {
    (val.wrapping_mul(HASH_MUL) >> 16) as usize
}

/// Hash of the low 6 of 8 bytes.
#[inline(always)]
pub(crate) fn hash6(val: u64) -> usize {
    ((val & HASH6_MASK).wrapping_mul(HASH6_MUL) >> 48) as usize
}

/// Borrow `N` bytes of `src` at `pos` as an array.
#[inline(always)]
pub(crate) fn array<const N: usize>(src: &[u8], pos: usize) -> &[u8; N] {
    src[pos..pos + N].try_into().unwrap()
}

/// Borrow `N` elements of `buf` at `at` as a mutable array.
#[inline(always)]
pub(crate) fn array_mut<T, const N: usize>(buf: &mut [T], at: usize) -> &mut [T; N] {
    (&mut buf[at..at + N]).try_into().unwrap()
}

/// Read 4 bytes as little-endian u32.
#[inline(always)]
pub(crate) fn read_u32(src: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(*array(src, pos))
}

/// Read 8 bytes as little-endian u64.
#[inline(always)]
pub(crate) fn read_u64(src: &[u8], pos: usize) -> u64 {
    u64::from_le_bytes(*array(src, pos))
}

/// Load 32 bytes as a vector.
#[inline(always)]
pub(crate) fn load<S: Simd>(simd: S, bytes: &[u8; VECTOR_WIDTH]) -> u8x32<S> {
    simd.load_array_ref_u8x32(bytes)
}

/// Common prefix length of two loaded vectors.
#[inline(always)]
pub(crate) fn lcp_loaded<S: Simd>(a: u8x32<S>, b: u8x32<S>) -> usize {
    let eq = a.simd_eq(b).to_bitmask() as u32;
    (!eq).trailing_zeros() as usize
}

/// Common prefix length of `src[a..]` and `src[b..]`, up to `limit`.
#[inline(always)]
pub(crate) fn lcp<S: Simd>(simd: S, src: &[u8], a: usize, b: usize, limit: usize) -> usize {
    let mut off = 0;
    while off + VECTOR_WIDTH <= limit {
        let len = lcp_loaded(
            load(simd, array(src, a + off)),
            load(simd, array(src, b + off)),
        );
        off += len;
        if len < VECTOR_WIDTH {
            return off;
        }
    }
    while off + 8 <= limit {
        let diff = read_u64(src, a + off) ^ read_u64(src, b + off);
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
