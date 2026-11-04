//! Stream assembly shared by the light and heavy writers.
//!
//! Writers put tokens forward from the extended header and literal runs
//! backward from the end of the output, then join the two.

use m77rip_core::format::{EXT_HEADER_SIZE, HEADER_SIZE};

/// Join a stream: the literal runs from `literals` to the end of `dst` move
/// behind the tokens ending at `tokens`, then the raw `suffix` follows.
/// Returns the stream length.
pub(crate) fn join(dst: &mut [u8], tokens: usize, literals: usize, suffix: &[u8]) -> usize {
    let cap = dst.len();
    dst.copy_within(literals..cap, tokens);
    let len = tokens + cap - literals;
    dst[HEADER_SIZE..EXT_HEADER_SIZE].copy_from_slice(&(suffix.len() as u64).to_le_bytes());
    dst[len..len + suffix.len()].copy_from_slice(suffix);
    len + suffix.len()
}

/// Write a literal length extension: bytes of 255, then the remainder.
#[inline(always)]
pub(crate) fn write_extension(dst: &mut [u8], pos: &mut usize, mut remaining: usize) {
    while remaining >= 255 {
        dst[*pos] = 255;
        *pos += 1;
        remaining -= 255;
    }
    dst[*pos] = remaining as u8;
    *pos += 1;
}
