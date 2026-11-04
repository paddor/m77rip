//! Light-format stream writer.

use crate::stream::{join, write_extension};
use m77rip_core::format::{
    EXT_HEADER_SIZE, HEADER_SIZE, MIN_DISTANCE, MIN_MATCH_LEN, SMALL_LIM, TOKEN_LIT_MAX,
    TOKEN_MATCH_BITS,
};

/// Writes tokens forward from the extended header and literal runs backward
/// from the end of `dst`, then joins them in [`LightWriter::finish`].
pub(crate) struct LightWriter<'a> {
    dst: &'a mut [u8],
    /// End of the token stream.
    tokens: usize,
    /// Start of the literal runs written so far.
    literals: usize,
}

impl<'a> LightWriter<'a> {
    /// Write the stream header. Inputs up to `SMALL_LIM` bytes are stored
    /// raw; then returns `Err` with the stream length.
    pub(crate) fn start(src: &[u8], dst: &'a mut [u8]) -> Result<Self, usize> {
        dst[..HEADER_SIZE].copy_from_slice(&(src.len() as u64).to_le_bytes());
        if src.len() <= SMALL_LIM {
            dst[HEADER_SIZE..HEADER_SIZE + src.len()].copy_from_slice(src);
            return Err(HEADER_SIZE + src.len());
        }
        let literals = dst.len();
        Ok(Self {
            dst,
            tokens: EXT_HEADER_SIZE,
            literals,
        })
    }

    /// Append a token: literal run `lits`, then a match of `match_len` bytes
    /// at distance `dis`.
    #[inline(always)]
    pub(crate) fn token(&mut self, lits: &[u8], match_len: usize, dis: usize) {
        let lit_len = lits.len();
        let field = lit_len.min(TOKEN_LIT_MAX) as u8;
        self.dst[self.tokens] =
            (field << TOKEN_MATCH_BITS) | (match_len - (MIN_MATCH_LEN - 1)) as u8;
        let dis = (dis - MIN_DISTANCE) as u16;
        self.dst[self.tokens + 1..self.tokens + 3].copy_from_slice(&dis.to_le_bytes());
        self.tokens += 3;
        if lit_len >= TOKEN_LIT_MAX {
            write_extension(self.dst, &mut self.tokens, lit_len - TOKEN_LIT_MAX);
        }
        self.literals -= lit_len;
        self.dst[self.literals..self.literals + lit_len].copy_from_slice(lits);
    }

    /// Move the literal runs behind the tokens and append the raw `suffix`.
    /// Returns the stream length.
    pub(crate) fn finish(self, suffix: &[u8]) -> usize {
        join(self.dst, self.tokens, self.literals, suffix)
    }
}
