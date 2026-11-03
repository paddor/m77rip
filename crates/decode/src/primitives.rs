//! Bounds-checked memory primitives for decompression.

/// Read 1 byte.
#[inline(always)]
pub(crate) fn read_byte(src: &[u8], pos: usize) -> u8 {
    src[pos]
}

/// Read 2 bytes as little-endian u16.
#[inline(always)]
pub(crate) fn read_u16_le(src: &[u8], pos: usize) -> u16 {
    u16::from_le_bytes(src[pos..pos + 2].try_into().unwrap())
}

/// Read 4 bytes as little-endian u32.
#[inline(always)]
pub(crate) fn read_u32_le(src: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(src[pos..pos + 4].try_into().unwrap())
}

/// Copy exactly `len` bytes from `src[src_pos..]` to `dst[dst_pos..]`.
#[inline(always)]
pub(crate) fn copy_from_src(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: usize,
    len: usize,
) {
    dst[dst_pos..dst_pos + len].copy_from_slice(&src[src_pos..src_pos + len]);
}

/// Unconditional 16-byte literal copy from `src` to `dst`.
#[inline(always)]
pub(crate) fn wild_copy_literals_16(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: usize,
    _actual_len: usize,
) {
    dst[dst_pos..dst_pos + 16].copy_from_slice(&src[src_pos..src_pos + 16]);
}

#[inline(always)]
pub(crate) fn wild_copy_literals_16_slices(src: &[u8], dst: &mut [u8]) {
    dst[..16].copy_from_slice(&src[..16]);
}

/// Unconditional 32-byte match copy within `dst`. Uses `split_at_mut` to
/// prove non-overlap, so the copy compiles to memcpy instead of memmove.
#[inline(always)]
pub(crate) fn wild_copy_match_32(
    dst: &mut [u8],
    src_pos: usize,
    dst_pos: usize,
    _actual_len: usize,
) {
    let (left, right) = dst.split_at_mut(dst_pos);
    right[..32].copy_from_slice(&left[src_pos..src_pos + 32]);
}

#[inline(always)]
pub(crate) fn wild_copy_match_32_slices(src: &[u8], dst: &mut [u8]) {
    dst[..32].copy_from_slice(&src[..32]);
}
