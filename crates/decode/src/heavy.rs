//! Heavy-format decoder: 4-byte tokens, 20-bit distances, matches of up to
//! 192 bytes copied in 32-byte chunks.

use fearless_simd::Simd;
use m77rip_core::Error;
use m77rip_core::format::{
    HEAVY_DIS_MASK, HEAVY_LEN_OF, HEAVY_LITERAL_SUFFIX, HEAVY_MAX_MATCH_LEN, HEAVY_MIN_DISTANCE,
    HEAVY_TOKEN_LIT_MAX,
};

use crate::decode::{Cursor, Layout};
use crate::primitives::{CHUNK, array, copy_literals, copy_match, load, read_extension, store};

/// Token bytes before any literal length extension.
const TOKEN_LEN: usize = 4;
/// Longest literal run a token holds without extension bytes.
const INLINE_LIT_MAX: usize = HEAVY_TOKEN_LIT_MAX - 1;
/// Output a group token's match may touch past its start: any `u8` length
/// rounded up to whole chunks.
const MATCH_ROOM: usize = 256;
/// Output a group token may touch past its start: inline literals, then its
/// match chunks.
const TOKEN_ROOM: usize = INLINE_LIT_MAX + MATCH_ROOM;

/// Tokens decoded per group: four calls of [`Group::quad`].
const GROUP: usize = 16;
/// Literal bytes a group may consume.
const GROUP_LITS: usize = GROUP * INLINE_LIT_MAX;
/// Control bytes a group reads.
const CTL_WIN: usize = GROUP * TOKEN_LEN;
/// Literal bytes a group reads: its runs plus two wild chunks.
const LIT_WIN: usize = GROUP_LITS + 2 * CHUNK;
/// Unread stream bytes a group needs, so its control and literal reads
/// never cross.
const GROUP_SRC: usize = CTL_WIN + GROUP_LITS;
/// Output room a group needs before `token_output_end`.
const GROUP_OUT: usize = (GROUP - 1) * (INLINE_LIT_MAX + HEAVY_MAX_MATCH_LEN) + TOKEN_ROOM;

// Match chunks never overlap their own source bytes.
const _: () = assert!(HEAVY_MIN_DISTANCE > CHUNK);
// The suffix is slack for the checked path's wild chunks.
const _: () = assert!(HEAVY_LITERAL_SUFFIX >= 2 * CHUNK);
const _: () = assert!(INLINE_LIT_MAX <= 2 * CHUNK);
const _: () = assert!(MATCH_ROOM >= u8::MAX as usize);

/// Token fields: literal length (63 means an extension follows), match
/// length (0 marks an invalid code), and distance.
#[inline(always)]
fn fields(token: u32) -> (usize, usize, usize) {
    let lit_len = (token >> 26) as usize;
    let match_len = usize::from(HEAVY_LEN_OF[((token >> 20) & 0x3F) as usize]);
    let dis = (token & HEAVY_DIS_MASK) as usize + HEAVY_MIN_DISTANCE;
    (lit_len, match_len, dis)
}

/// Decode all tokens. Returns the cursor for [`Layout::finish`].
///
/// `COND` streams rarely hold matches over one chunk, so their match copy
/// tests the length after the first chunk.
#[inline(always)]
pub(crate) fn decode<S: Simd, const COND: bool>(
    _simd: S,
    src: &[u8],
    dst: &mut [u8],
    layout: Layout,
) -> Result<Cursor, Error> {
    let end = layout.token_output_end;
    let mut cursor = layout.start();
    while cursor.control < cursor.literals {
        groups::<COND>(src, dst, &mut cursor, end);
        if cursor.control < cursor.literals {
            checked_step(src, dst, &mut cursor, end)?;
        }
    }
    Ok(cursor)
}

/// Decode groups of tokens while the stream and output have room for a
/// whole group. A token the group cannot take (a literal extension, an
/// invalid code, or a match reaching into its own literals) ends the group
/// and is decoded with every check.
///
/// Each group borrows fixed-size windows of its control and literal bytes.
/// Each token borrows its output from the match source to the end of its
/// wild copies, so its copies need no further checks.
#[inline(always)]
fn groups<const COND: bool>(src: &[u8], dst: &mut [u8], cursor: &mut Cursor, end: usize) {
    let Cursor {
        mut control,
        mut literals,
        mut out,
    } = *cursor;
    while literals - control >= GROUP_SRC && end - out >= GROUP_OUT {
        let mut group = Group {
            ctl: array(src, control),
            lits: array(src, literals - GROUP_LITS),
            lit: GROUP_LITS,
            out,
            tokens: 0,
        };
        let done = group.quad::<COND>(dst, 0)
            && group.quad::<COND>(dst, 4)
            && group.quad::<COND>(dst, 8)
            && group.quad::<COND>(dst, 12);
        control += group.tokens * TOKEN_LEN;
        literals -= GROUP_LITS - group.lit;
        out = group.out;
        if !done {
            let mut step = Cursor {
                control,
                literals,
                out,
            };
            if checked_step(src, dst, &mut step, end).is_err() {
                // Leave the cursor before the bad token; the caller's
                // checked step reports it.
                break;
            }
            Cursor {
                control,
                literals,
                out,
            } = step;
        }
    }
    *cursor = Cursor {
        control,
        literals,
        out,
    };
}

/// Windows and positions of one group of tokens.
struct Group<'a> {
    ctl: &'a [u8; CTL_WIN],
    lits: &'a [u8; LIT_WIN],
    /// End of the unread literals in `lits`.
    lit: usize,
    out: usize,
    tokens: usize,
}

impl Group<'_> {
    /// Decode tokens `n..n + 4` of the group. Returns `false` at the first
    /// token that needs the checked path.
    #[inline(always)]
    fn quad<const COND: bool>(&mut self, dst: &mut [u8], n: usize) -> bool {
        self.token::<COND>(dst, n)
            && self.token::<COND>(dst, n + 1)
            && self.token::<COND>(dst, n + 2)
            && self.token::<COND>(dst, n + 3)
    }

    /// Decode token `n` of the group. Returns `false`, consuming nothing, if
    /// the token needs the checked path.
    #[inline(always)]
    fn token<const COND: bool>(&mut self, dst: &mut [u8], n: usize) -> bool {
        let (lit_len, match_len, dis) = fields(u32::from_le_bytes(load(self.ctl, n * TOKEN_LEN)));
        if lit_len > INLINE_LIT_MAX || match_len == 0 {
            return false;
        }
        let Some(start) = self.out.checked_sub(dis) else {
            return false;
        };
        self.lit -= lit_len;
        // Output from the match source to the end of the wild copies: `dis`
        // bytes of history, then `TOKEN_ROOM` bytes of output. Loads index
        // the whole window; stores index the output part, whose length is a
        // constant.
        let win = &mut dst[start..][..dis + TOKEN_ROOM];
        store(&mut win[dis..], 0, load::<CHUNK>(self.lits, self.lit));
        if lit_len > CHUNK {
            store(
                &mut win[dis..],
                CHUNK,
                load::<CHUNK>(self.lits, self.lit + CHUNK),
            );
        }
        let chunk = load::<CHUNK>(win, lit_len);
        store(&mut win[dis..], lit_len, chunk);
        if !COND || match_len > CHUNK {
            let chunk = load::<CHUNK>(win, lit_len + CHUNK);
            store(&mut win[dis..], lit_len + CHUNK, chunk);
            let mut copied = 2 * CHUNK;
            while copied < match_len {
                let chunk = load::<CHUNK>(win, lit_len + copied);
                store(&mut win[dis..], lit_len + copied, chunk);
                copied += CHUNK;
            }
        }
        self.out += lit_len + match_len;
        self.tokens = n + 1;
        true
    }
}

/// Decode one token with every check: tokens a group cannot take and tokens
/// near the end of either stream.
#[inline(never)]
fn checked_step(src: &[u8], dst: &mut [u8], cursor: &mut Cursor, end: usize) -> Result<(), Error> {
    if cursor.literals - cursor.control < TOKEN_LEN {
        return Err(Error::CorruptInput);
    }
    let (mut lit_len, match_len, dis) = fields(u32::from_le_bytes(load(src, cursor.control)));
    let mut control = cursor.control + TOKEN_LEN;
    if lit_len == HEAVY_TOKEN_LIT_MAX {
        lit_len += read_extension(src, &mut control, cursor.literals)?;
    }
    if match_len == 0 || lit_len > cursor.literals - control {
        return Err(Error::CorruptInput);
    }
    let after = cursor.out + lit_len;
    if after + match_len > end || dis > after {
        return Err(Error::CorruptInput);
    }
    cursor.control = control;
    cursor.literals -= lit_len;
    copy_literals(src, cursor.literals, dst, cursor.out, lit_len);
    copy_match(dst, after, dis, match_len);
    cursor.out = after + match_len;
    Ok(())
}
