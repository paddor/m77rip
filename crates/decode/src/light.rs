//! Light-format decoder: 3-byte tokens, 16-bit distances, matches of up to
//! 34 bytes copied as one 32-byte chunk.

use fearless_simd::Simd;
use m77rip_core::Error;
use m77rip_core::format::{
    LITERAL_SUFFIX, MAX_DISTANCE, MIN_DISTANCE, MIN_MATCH_LEN, TOKEN_LIT_MAX, TOKEN_MATCH_BITS,
    TOKEN_MATCH_MASK,
};

use crate::decode::{Cursor, Layout};
use crate::primitives::{CHUNK, array, array_mut, copy_literals, load, read_extension, store};

/// Token header bytes: control byte and 16-bit distance.
const TOKEN_LEN: usize = 3;
/// Longest match a token can encode.
const TOKEN_MATCH_MAX: usize = TOKEN_MATCH_MASK as usize + MIN_MATCH_LEN - 1;
/// Longest literal run a group token holds: a run with one extension byte,
/// copied as one chunk.
const GROUP_LIT_MAX: usize = CHUNK;
/// Output of one group token at most.
const TOKEN_OUT_MAX: usize = GROUP_LIT_MAX + TOKEN_MATCH_MAX;

/// Tokens decoded per group: four calls of [`Group::quad`].
const GROUP: usize = 16;
/// Literal bytes a group may consume.
const GROUP_LITS: usize = GROUP * GROUP_LIT_MAX;
/// Control bytes a group reads: headers with one extension byte each.
const CTL_WIN: usize = GROUP * (TOKEN_LEN + 1);
/// Literal bytes a group reads: its runs plus one wild copy.
const LIT_WIN: usize = GROUP_LITS + CHUNK;
/// Unread stream bytes a group needs, so its control and literal reads
/// never cross.
const GROUP_SRC: usize = CTL_WIN + GROUP_LITS;
/// Output room a group needs before `token_output_end`.
const GROUP_OUT: usize = GROUP * TOKEN_OUT_MAX;
/// Output a group touches: `MAX_DISTANCE` bytes of history, then its tokens
/// and the wild copies of the last one.
const OUT_WIN: usize = MAX_DISTANCE + (GROUP - 1) * TOKEN_OUT_MAX + GROUP_LIT_MAX + CHUNK;

// The largest distance reaches exactly the window start.
const _: () = assert!(u16::MAX as usize + MIN_DISTANCE == MAX_DISTANCE);
// Match chunks never overlap their source.
const _: () = assert!(MIN_DISTANCE > CHUNK);
// Wild copies of a group stay inside its output room plus the suffix.
const _: () = assert!(OUT_WIN - MAX_DISTANCE <= GROUP_OUT + LITERAL_SUFFIX);
// The suffix is slack for the checked path's chunk copies.
const _: () = assert!(LITERAL_SUFFIX >= CHUNK);

/// Decode all tokens. Returns the cursor for [`Layout::finish`].
#[inline(always)]
pub(crate) fn decode<S: Simd>(
    _simd: S,
    src: &[u8],
    dst: &mut [u8],
    layout: Layout,
) -> Result<Cursor, Error> {
    let end = layout.token_output_end;
    let mut cursor = layout.start();
    while cursor.control < cursor.literals {
        if dst.len() >= OUT_WIN {
            if cursor.out <= MAX_DISTANCE {
                groups::<false>(src, dst, &mut cursor, end)?;
            }
            if cursor.out > MAX_DISTANCE {
                groups::<true>(src, dst, &mut cursor, end)?;
            }
        }
        if cursor.control < cursor.literals {
            checked_step(src, dst, &mut cursor, end)?;
        }
    }
    Ok(cursor)
}

/// Decode groups of tokens while the stream and output have room for
/// a whole group. A token the group cannot take (a literal run over
/// `GROUP_LIT_MAX` bytes, or a match reaching before the output start)
/// ends its group and is decoded with every check.
///
/// Each group borrows fixed-size windows: its control bytes, its literal
/// bytes, and its output with the `MAX_DISTANCE` bytes before it. Every
/// offset into them is bounded by the token field widths, so the token
/// copies need no bounds checks. `MAIN` groups start past `MAX_DISTANCE`,
/// where no match can reach before the output start. Earlier groups use a
/// window at the output start and check each distance.
#[inline(always)]
fn groups<const MAIN: bool>(
    src: &[u8],
    dst: &mut [u8],
    cursor: &mut Cursor,
    end: usize,
) -> Result<(), Error> {
    let Cursor {
        mut control,
        mut literals,
        mut out,
    } = *cursor;
    while literals - control >= GROUP_SRC && end - out >= GROUP_OUT && (MAIN || out <= MAX_DISTANCE)
    {
        let (base, start) = if MAIN {
            (out - MAX_DISTANCE, MAX_DISTANCE)
        } else {
            (0, out)
        };
        let mut group = Group {
            ctl: array(src, control),
            lits: array(src, literals - GROUP_LITS),
            win: array_mut(dst, base),
            lit: GROUP_LITS,
            at: start,
            ctl_at: 0,
        };
        let done = group.quad::<MAIN>()
            && group.quad::<MAIN>()
            && group.quad::<MAIN>()
            && group.quad::<MAIN>();
        control += group.ctl_at;
        literals -= GROUP_LITS - group.lit;
        out += group.at - start;
        if !done {
            let mut ext = Cursor {
                control,
                literals,
                out,
            };
            checked_step(src, dst, &mut ext, end)?;
            Cursor {
                control,
                literals,
                out,
            } = ext;
        }
    }
    *cursor = Cursor {
        control,
        literals,
        out,
    };
    Ok(())
}

/// Windows and positions of one group of tokens.
struct Group<'a> {
    ctl: &'a [u8; CTL_WIN],
    lits: &'a [u8; LIT_WIN],
    win: &'a mut [u8; OUT_WIN],
    /// End of the unread literals in `lits`.
    lit: usize,
    /// Output position in `win`.
    at: usize,
    /// Next header in `ctl`.
    ctl_at: usize,
}

impl Group<'_> {
    /// Decode the next four tokens. Returns `false` at the first token the
    /// group cannot take.
    #[inline(always)]
    fn quad<const MAIN: bool>(&mut self) -> bool {
        self.token::<MAIN>() && self.token::<MAIN>() && self.token::<MAIN>() && self.token::<MAIN>()
    }

    /// Decode the next token of the group. Returns `false`, consuming
    /// nothing, if its literal run is longer than `GROUP_LIT_MAX` or, before
    /// `MAIN`, its match starts before the output.
    ///
    /// The 4-byte header load holds the first literal extension byte, so a
    /// run of up to `GROUP_LIT_MAX` bytes needs no extra read.
    #[inline(always)]
    fn token<const MAIN: bool>(&mut self) -> bool {
        let header = u32::from_le_bytes(load(self.ctl, self.ctl_at)) as usize;
        let mut lit_len = (header >> TOKEN_MATCH_BITS) & TOKEN_LIT_MAX;
        if lit_len == TOKEN_LIT_MAX {
            // A predicted branch, not a select: the next header address
            // must not wait for this load.
            lit_len += header >> 24;
            if lit_len > GROUP_LIT_MAX {
                return false;
            }
            self.ctl_at += 1;
        }
        let dis = ((header >> 8) & 0xFFFF) + MIN_DISTANCE;
        if !MAIN && dis > self.at + lit_len {
            // Undo the extension byte.
            self.ctl_at -= usize::from(lit_len >= TOKEN_LIT_MAX);
            return false;
        }
        self.ctl_at += TOKEN_LEN;
        self.lit -= lit_len;
        store(self.win, self.at, load::<CHUNK>(self.lits, self.lit));
        let at = self.at + lit_len;
        let chunk = load::<CHUNK>(self.win, at - dis);
        store(self.win, at, chunk);
        self.at = at + (header & TOKEN_MATCH_MASK as usize) + MIN_MATCH_LEN - 1;
        true
    }
}

/// Decode one token with every check: long literal runs and tokens near
/// the end of either stream.
#[inline(never)]
fn checked_step(src: &[u8], dst: &mut [u8], cursor: &mut Cursor, end: usize) -> Result<(), Error> {
    if cursor.literals - cursor.control < TOKEN_LEN {
        return Err(Error::CorruptInput);
    }
    let [token, d0, d1] = load(src, cursor.control);
    cursor.control += TOKEN_LEN;
    let mut lit_len = usize::from(token >> TOKEN_MATCH_BITS);
    if lit_len == TOKEN_LIT_MAX {
        lit_len += read_extension(src, &mut cursor.control, cursor.literals)?;
    }
    let match_len = usize::from(token & TOKEN_MATCH_MASK) + MIN_MATCH_LEN - 1;
    let dis = usize::from(u16::from_le_bytes([d0, d1])) + MIN_DISTANCE;

    if lit_len > cursor.literals - cursor.control {
        return Err(Error::CorruptInput);
    }
    let after = cursor.out + lit_len;
    if after + match_len > end || dis > after {
        return Err(Error::CorruptInput);
    }
    cursor.literals -= lit_len;
    copy_literals(src, cursor.literals, dst, cursor.out, lit_len);
    let (history, tail) = dst.split_at_mut(after);
    store(tail, 0, load::<CHUNK>(history, after - dis));
    cursor.out = after + match_len;
    Ok(())
}
