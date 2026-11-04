//! Fixed-size chunk copies and shared stream helpers.
//!
//! Copies move `[u8; N]` chunks, so each chunk costs at most one bounds check
//! and the compiler emits plain vector loads and stores. A chunk is loaded
//! completely before it is stored. Wild copies write up to one chunk past
//! the requested length; callers reserve that slack.

use m77rip_core::Error;
use m77rip_core::format::VECTOR_WIDTH;

/// Width of literal and match chunk copies.
pub(crate) const CHUNK: usize = VECTOR_WIDTH;

/// Literal runs longer than this use one exact copy instead of chunks.
const LONG_LITERALS: usize = 2 * CHUNK;

/// Load `N` bytes of `buf` at `at`.
#[inline(always)]
pub(crate) fn load<const N: usize>(buf: &[u8], at: usize) -> [u8; N] {
    buf[at..at + N].try_into().unwrap()
}

/// Store `N` bytes into `buf` at `at`.
#[inline(always)]
pub(crate) fn store<const N: usize>(buf: &mut [u8], at: usize, chunk: [u8; N]) {
    buf[at..at + N].copy_from_slice(&chunk);
}

/// Copy `N` bytes within `buf` from `from` to `to`, loading before storing.
#[inline(always)]
pub(crate) fn copy_chunk<const N: usize>(buf: &mut [u8], from: usize, to: usize) {
    let chunk = load::<N>(buf, from);
    store(buf, to, chunk);
}

/// Borrow `N` bytes of `buf` at `at` as an array, so offsets into it that
/// the compiler can bound need no checks.
#[inline(always)]
pub(crate) fn array<const N: usize>(buf: &[u8], at: usize) -> &[u8; N] {
    buf[at..at + N].try_into().unwrap()
}

/// Mutable [`array`].
#[inline(always)]
pub(crate) fn array_mut<const N: usize>(buf: &mut [u8], at: usize) -> &mut [u8; N] {
    (&mut buf[at..at + N]).try_into().unwrap()
}

/// Read a literal length extension: bytes of 255 continue, the first smaller
/// byte ends it. Extension bytes must lie before `limit`.
#[inline(always)]
pub(crate) fn read_extension(
    src: &[u8],
    control: &mut usize,
    limit: usize,
) -> Result<usize, Error> {
    let mut total = 0usize;
    loop {
        if *control >= limit {
            return Err(Error::CorruptInput);
        }
        let byte = src[*control];
        *control += 1;
        total = total
            .checked_add(usize::from(byte))
            .ok_or(Error::CorruptInput)?;
        if byte != u8::MAX {
            return Ok(total);
        }
    }
}

/// Copy `len` literal bytes from `src[from..]` to `dst[to..]`.
///
/// Short runs copy whole chunks, so both sides need `CHUNK` bytes of slack
/// past `len`.
#[inline(always)]
pub(crate) fn copy_literals(src: &[u8], from: usize, dst: &mut [u8], to: usize, len: usize) {
    if len > LONG_LITERALS {
        dst[to..to + len].copy_from_slice(&src[from..from + len]);
        return;
    }
    store(dst, to, load::<CHUNK>(src, from));
    if len > CHUNK {
        store(dst, to + CHUNK, load::<CHUNK>(src, from + CHUNK));
    }
}

/// Copy a match of `len` bytes from `dis` bytes back to `to`, in whole
/// chunks. Needs `dis > CHUNK` and `CHUNK` bytes of slack past `to + len`.
/// Chunks run forward, so a match that overlaps its source repeats it.
#[inline(always)]
pub(crate) fn copy_match(dst: &mut [u8], to: usize, dis: usize, len: usize) {
    debug_assert!(dis > CHUNK);
    let mut copied = 0;
    while copied < len {
        copy_chunk::<CHUNK>(dst, to - dis + copied, to + copied);
        copied += CHUNK;
    }
}
