//! Heavy-format stream writer.

use m77rip_core::format::{
    EXT_HEADER_SIZE, FLAG_HEAVY, FLAG_HEAVY_COND, FLAG_SHIFT, HEADER_SIZE, HEAVY_CODE_FLOOR,
    HEAVY_DIS_MASK, HEAVY_LEN_OF, HEAVY_MIN_DISTANCE, HEAVY_SMALL_LIM, HEAVY_TOKEN_LIT_MAX,
    VECTOR_WIDTH,
};

use super::parse::TokenRecord;
use crate::stream::{join, write_extension};

/// Streams get `FLAG_HEAVY_COND` when fewer than 14 % of their matches
/// exceed one chunk.
const COND_NUM: usize = 14;
const COND_DEN: usize = 100;

/// Writes heavy tokens and literal runs; see [`crate::stream`].
pub(crate) struct HeavyWriter<'a> {
    dst: &'a mut [u8],
    tokens: usize,
    literals: usize,
    /// Start of the pending literals in the input.
    lit: usize,
    count: usize,
    long_matches: usize,
}

fn header(len: usize, flags: u8) -> [u8; HEADER_SIZE] {
    (len as u64 | (u64::from(flags) << FLAG_SHIFT)).to_le_bytes()
}

impl<'a> HeavyWriter<'a> {
    /// Write the stream header. Inputs up to `HEAVY_SMALL_LIM` bytes are
    /// stored raw; then returns `Err` with the stream length.
    pub(crate) fn start(src: &[u8], dst: &'a mut [u8]) -> Result<Self, usize> {
        dst[..HEADER_SIZE].copy_from_slice(&header(src.len(), FLAG_HEAVY));
        if src.len() <= HEAVY_SMALL_LIM {
            dst[HEADER_SIZE..HEADER_SIZE + src.len()].copy_from_slice(src);
            return Err(HEADER_SIZE + src.len());
        }
        let literals = dst.len();
        Ok(Self {
            dst,
            tokens: EXT_HEADER_SIZE,
            literals,
            lit: 0,
            count: 0,
            long_matches: 0,
        })
    }

    /// Append the token for `record` with the literals before it.
    pub(crate) fn token(&mut self, src: &[u8], record: TokenRecord) {
        let lit_len = record.match_start - self.lit;
        let len = usize::from(record.len);
        let code = usize::from(HEAVY_CODE_FLOOR[len]);
        debug_assert_eq!(usize::from(HEAVY_LEN_OF[code]), len);
        let dis = record.dis as usize;
        debug_assert!(dis >= HEAVY_MIN_DISTANCE);
        let field = lit_len.min(HEAVY_TOKEN_LIT_MAX);
        let token = ((field as u32) << 26)
            | ((code as u32) << 20)
            | (((dis - HEAVY_MIN_DISTANCE) as u32) & HEAVY_DIS_MASK);
        self.dst[self.tokens..self.tokens + 4].copy_from_slice(&token.to_le_bytes());
        self.tokens += 4;
        if field == HEAVY_TOKEN_LIT_MAX {
            write_extension(self.dst, &mut self.tokens, lit_len - HEAVY_TOKEN_LIT_MAX);
        }
        self.literals -= lit_len;
        self.dst[self.literals..self.literals + lit_len]
            .copy_from_slice(&src[self.lit..record.match_start]);
        self.count += 1;
        self.long_matches += usize::from(len > VECTOR_WIDTH);
        self.lit = record.match_start + len;
    }

    /// Join the stream and set its final flags. Returns the stream length.
    pub(crate) fn finish(self, src: &[u8]) -> usize {
        let len = join(self.dst, self.tokens, self.literals, &src[self.lit..]);
        let cond = self.count != 0 && self.long_matches * COND_DEN < COND_NUM * self.count;
        let flags = FLAG_HEAVY | if cond { FLAG_HEAVY_COND } else { 0 };
        self.dst[..HEADER_SIZE].copy_from_slice(&header(src.len(), flags));
        len
    }
}
