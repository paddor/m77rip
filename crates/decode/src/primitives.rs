//! Low-level memory primitives for decompression.
//!
//! Each operation has two implementations selected at compile time. The default
//! build uses unchecked pointer operations for speed; these functions are
//! `unsafe fn` because callers must uphold bounds preconditions documented in
//! each function's `debug_assert!` guards. The `paranoid` feature provides safe
//! `fn` twins with no preconditions (violations panic via bounds checks).

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
use core::arch::x86_64::{__m256i, _mm256_loadu_si256, _mm256_storeu_si256};

/// Read 1 byte without bounds checking.
///
/// # Safety
///
/// `pos` must be within `src`.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn read_byte(src: &[u8], pos: usize) -> u8 {
    debug_assert!(pos < src.len());
    // SAFETY: Caller guarantees `pos < src.len()`.
    unsafe { *src.as_ptr().add(pos) }
}

/// Read 1 byte (paranoid: bounds-checked).
#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn read_byte(src: &[u8], pos: usize) -> u8 {
    src[pos]
}

/// Read 2 bytes as little-endian u16 without bounds checking.
///
/// # Safety
///
/// `pos + 2` must be within `src`.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn read_u16_le(src: &[u8], pos: usize) -> u16 {
    debug_assert!(pos + 2 <= src.len());
    // SAFETY: Caller guarantees two readable bytes starting at `pos`.
    u16::from_le(unsafe { (src.as_ptr().add(pos) as *const u16).read_unaligned() })
}

/// Read 2 bytes as little-endian u16 (paranoid: bounds-checked).
#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn read_u16_le(src: &[u8], pos: usize) -> u16 {
    u16::from_le_bytes(src[pos..pos + 2].try_into().unwrap())
}

/// Read 4 bytes as little-endian u32 without bounds checking.
///
/// # Safety
///
/// `pos + 4` must be within `src`.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn read_u32_le(src: &[u8], pos: usize) -> u32 {
    debug_assert!(pos + 4 <= src.len());
    // SAFETY: Caller guarantees four readable bytes starting at `pos`.
    u32::from_le(unsafe { (src.as_ptr().add(pos) as *const u32).read_unaligned() })
}

/// Read 4 bytes as little-endian u32 (paranoid: bounds-checked).
#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn read_u32_le(src: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(src[pos..pos + 4].try_into().unwrap())
}

/// Copy exactly `len` bytes from `src[src_pos..]` to `dst[dst_pos..]`.
///
/// # Safety
///
/// Both ranges must be within their slices and must not overlap.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn copy_from_src(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: usize,
    len: usize,
) {
    debug_assert!(src_pos + len <= src.len());
    debug_assert!(dst_pos + len <= dst.len());
    // SAFETY: Caller guarantees both ranges are valid and non-overlapping.
    unsafe {
        core::ptr::copy_nonoverlapping(
            src.as_ptr().add(src_pos),
            dst.as_mut_ptr().add(dst_pos),
            len,
        );
    }
}

/// Copy exactly `len` bytes (paranoid: bounds-checked).
#[cfg(feature = "paranoid")]
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
///
/// Default build copies exactly 16 bytes regardless of `actual_len` (caller
/// guarantees overwrite room). Paranoid build copies exactly 16 bytes with
/// bounds checks.
///
/// # Safety
///
/// Both 16-byte ranges must be within their slices and must not overlap.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn wild_copy_literals_16(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: usize,
    _actual_len: usize,
) {
    debug_assert!(src_pos + 16 <= src.len());
    debug_assert!(dst_pos + 16 <= dst.len());
    // SAFETY: Caller guarantees both fixed-size ranges are valid and
    // non-overlapping.
    unsafe {
        core::ptr::copy_nonoverlapping(
            src.as_ptr().add(src_pos),
            dst.as_mut_ptr().add(dst_pos),
            16,
        );
    }
}

/// Unconditional 16-byte literal copy (paranoid: bounds-checked, fixed-size).
#[cfg(feature = "paranoid")]
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

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn wild_copy_literals_16_slices(src: &[u8], dst: &mut [u8]) {
    dst[..16].copy_from_slice(&src[..16]);
}

/// Unconditional 32-byte literal copy from `src` to `dst`.
///
/// # Safety
///
/// Both 32-byte ranges must be within their slices and must not overlap.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn wild_copy_literals_32(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: usize,
) {
    debug_assert!(src_pos + 32 <= src.len());
    debug_assert!(dst_pos + 32 <= dst.len());
    // SAFETY: Caller guarantees both fixed-size ranges are valid and
    // non-overlapping.
    unsafe {
        core::ptr::copy_nonoverlapping(
            src.as_ptr().add(src_pos),
            dst.as_mut_ptr().add(dst_pos),
            32,
        );
    }
}

/// AVX2 32-byte literal copy from `src` to `dst`.
///
/// # Safety
///
/// CPU must support AVX2. Both 32-byte ranges must be within their slices and
/// must not overlap.
#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[inline(always)]
pub(crate) unsafe fn avx2_copy_literals_32(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: usize,
) {
    debug_assert!(src_pos + 32 <= src.len());
    debug_assert!(dst_pos + 32 <= dst.len());
    // SAFETY: Caller guarantees AVX2 support plus valid non-overlapping ranges.
    let reg = unsafe { _mm256_loadu_si256(src.as_ptr().add(src_pos).cast::<__m256i>()) };
    // SAFETY: Caller guarantees 32 writable bytes at `dst_pos`.
    unsafe { _mm256_storeu_si256(dst.as_mut_ptr().add(dst_pos).cast::<__m256i>(), reg) };
}

/// Unconditional 32-byte non-overlapping match copy within `dst`.
///
/// Default build copies exactly 32 bytes (caller guarantees `dis >= 33` and
/// overwrite room from suffix padding). Paranoid build copies exactly 32 bytes
/// with bounds checks via `copy_within`.
///
/// # Safety
///
/// Both 32-byte ranges must be within `dst` and must not overlap.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn wild_copy_match_32(
    dst: &mut [u8],
    src_pos: usize,
    dst_pos: usize,
    _actual_len: usize,
) {
    debug_assert!(dst_pos >= src_pos + 33);
    debug_assert!(dst_pos + 32 <= dst.len());
    // SAFETY: Caller guarantees both fixed-size ranges are valid and
    // non-overlapping. The distance invariant prevents overlap.
    unsafe {
        let ptr = dst.as_mut_ptr();
        core::ptr::copy_nonoverlapping(ptr.add(src_pos), ptr.add(dst_pos), 32);
    }
}

/// AVX2 32-byte non-overlapping match copy within `dst`.
///
/// # Safety
///
/// CPU must support AVX2. Both 32-byte ranges must be within `dst` and must not
/// overlap.
#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[inline(always)]
pub(crate) unsafe fn avx2_copy_match_32(dst: &mut [u8], src_pos: usize, dst_pos: usize) {
    debug_assert!(dst_pos >= src_pos + 33);
    debug_assert!(dst_pos + 32 <= dst.len());
    // SAFETY: Caller guarantees valid non-overlapping ranges and AVX2 support.
    unsafe {
        let ptr = dst.as_mut_ptr();
        let reg = _mm256_loadu_si256(ptr.add(src_pos).cast::<__m256i>());
        _mm256_storeu_si256(ptr.add(dst_pos).cast::<__m256i>(), reg);
    }
}

/// Unconditional 32-byte non-overlapping match copy (paranoid: bounds-checked,
/// fixed-size). Uses `split_at_mut` to prove non-overlap to the compiler,
/// enabling memcpy codegen instead of memmove.
#[cfg(feature = "paranoid")]
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

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn wild_copy_match_32_slices(src: &[u8], dst: &mut [u8]) {
    dst[..32].copy_from_slice(&src[..32]);
}
