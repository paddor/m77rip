//! Heavy-format decoder: 4-byte tokens, 20-bit distances, matches of up to
//! 192 bytes copied in 32-byte chunks.

use fearless_simd::{Level, Simd, dispatch};
use m77rip_core::Error;
use m77rip_core::format::{
    HEAVY_DIS_MASK, HEAVY_LEN_OF, HEAVY_LITERAL_SUFFIX, HEAVY_MAX_DISTANCE, HEAVY_MIN_DISTANCE,
    HEAVY_TOKEN_LIT_MAX,
};

use crate::decode::{Cursor, Layout};
use crate::primitives::{
    CHUNK, array, array_mut, copy_literals, copy_match, load, read_extension, store,
};

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

/// Tokens decoded per group: four quads of four.
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
/// Output of one group token at most, for any `u8` match length: the
/// compiler bounds lengths by their type, not by `HEAVY_MAX_MATCH_LEN`.
const TOKEN_OUT_MAX: usize = INLINE_LIT_MAX + u8::MAX as usize;
/// Output a group may touch past its start, and the room it needs before
/// `token_output_end`.
const GROUP_OUT: usize = (GROUP - 1) * TOKEN_OUT_MAX + TOKEN_ROOM;

/// Output of the window over the output start.
const START_WIN: usize = 1 << 20;
/// Output of the sliding window: the whole match history before a group,
/// then the group's output.
const MAIN_WIN: usize = HEAVY_MAX_DISTANCE + GROUP_OUT;

// Match chunks never overlap their own source bytes.
const _: () = assert!(HEAVY_MIN_DISTANCE > CHUNK);
// The suffix is slack for the checked path's wild chunks.
const _: () = assert!(HEAVY_LITERAL_SUFFIX >= 2 * CHUNK);
const _: () = assert!(INLINE_LIT_MAX <= 2 * CHUNK);
const _: () = assert!(MATCH_ROOM >= u8::MAX as usize);

/// Fields of one token.
#[derive(Clone, Copy)]
struct Fields {
    /// Literal run length; `HEAVY_TOKEN_LIT_MAX` means an extension follows.
    lit_len: usize,
    /// Match length; 0 marks an invalid code.
    match_len: usize,
    dis: usize,
}

impl Fields {
    #[inline(always)]
    fn parse(token: u32) -> Self {
        Self {
            lit_len: (token >> 26) as usize,
            match_len: usize::from(HEAVY_LEN_OF[((token >> 20) & 0x3F) as usize]),
            dis: (token & HEAVY_DIS_MASK) as usize + HEAVY_MIN_DISTANCE,
        }
    }
}

/// Decode all tokens. Returns the cursor for [`Layout::finish`].
///
/// `cond` streams rarely hold matches over one chunk, so their match copy
/// tests the length after the first chunk. Output before and after
/// `HEAVY_MAX_DISTANCE` decodes in separately dispatched phases, so each
/// phase's group loops get their own code generation.
pub(crate) fn decode(
    level: Level,
    src: &[u8],
    dst: &mut [u8],
    layout: Layout,
    cond: bool,
) -> Result<Cursor, Error> {
    let end = layout.token_output_end;
    let mut cursor = layout.start();
    cursor = if cond {
        dispatch!(level, simd => phase::<_, true, false>(simd, src, dst, end, cursor))
    } else {
        dispatch!(level, simd => phase::<_, false, false>(simd, src, dst, end, cursor))
    }?;
    if cursor.control < cursor.literals {
        cursor = if cond {
            dispatch!(level, simd => phase::<_, true, true>(simd, src, dst, end, cursor))
        } else {
            dispatch!(level, simd => phase::<_, false, true>(simd, src, dst, end, cursor))
        }?;
    }
    Ok(cursor)
}

/// Decode tokens until the stream ends or, before `MAIN`, the output
/// reaches `HEAVY_MAX_DISTANCE`.
#[inline(always)]
fn phase<S: Simd, const COND: bool, const MAIN: bool>(
    _simd: S,
    src: &[u8],
    dst: &mut [u8],
    end: usize,
    mut cursor: Cursor,
) -> Result<Cursor, Error> {
    while cursor.control < cursor.literals && (MAIN || cursor.out < HEAVY_MAX_DISTANCE) {
        if MAIN {
            groups::<COND, MainWindow>(src, dst, &mut cursor, end);
        } else {
            groups::<COND, StartWindow>(src, dst, &mut cursor, end);
            groups::<COND, TokenWindows>(src, dst, &mut cursor, end);
        }
        if cursor.control < cursor.literals {
            checked_step(src, dst, &mut cursor, end)?;
        }
    }
    Ok(cursor)
}

/// How groups borrow their output. Every group borrows fixed-size windows
/// of its control and literal bytes; its output either lies in one
/// fixed-size window, so offsets bounded by the token fields need no checks,
/// or each token borrows its own.
trait Placement {
    /// Whether a group starting at output `out` can use this placement.
    fn fits(out: usize, dst_len: usize) -> bool;

    /// Decode the tokens of `group`. Returns `false` at the first token the
    /// group cannot take.
    fn run<const COND: bool>(group: &mut Group<'_>, dst: &mut [u8]) -> bool;
}

/// Each token borrows its output from its match source. Covers outputs
/// under `START_WIN` and the gap before `MainWindow` applies.
struct TokenWindows;

impl Placement for TokenWindows {
    #[inline(always)]
    fn fits(out: usize, _dst_len: usize) -> bool {
        out < HEAVY_MAX_DISTANCE
    }

    #[inline(always)]
    fn run<const COND: bool>(group: &mut Group<'_>, dst: &mut [u8]) -> bool {
        group.quad_windows::<COND>(dst, 0)
            && group.quad_windows::<COND>(dst, 4)
            && group.quad_windows::<COND>(dst, 8)
            && group.quad_windows::<COND>(dst, 12)
    }
}

/// One window over the first `START_WIN` output bytes. Each distance is
/// checked against the output position.
struct StartWindow;

impl Placement for StartWindow {
    #[inline(always)]
    fn fits(out: usize, dst_len: usize) -> bool {
        dst_len >= START_WIN && out <= START_WIN - GROUP_OUT
    }

    #[inline(always)]
    fn run<const COND: bool>(group: &mut Group<'_>, dst: &mut [u8]) -> bool {
        group.in_window::<COND, START_WIN>(array_mut(dst, 0), 0)
    }
}

/// A window from `HEAVY_MAX_DISTANCE` bytes before the group, so no match
/// reaches before it.
struct MainWindow;

impl Placement for MainWindow {
    #[inline(always)]
    fn fits(out: usize, _dst_len: usize) -> bool {
        out >= HEAVY_MAX_DISTANCE
    }

    #[inline(always)]
    fn run<const COND: bool>(group: &mut Group<'_>, dst: &mut [u8]) -> bool {
        let base = group.at - HEAVY_MAX_DISTANCE;
        group.in_window::<COND, MAIN_WIN>(array_mut(dst, base), base)
    }
}

/// Decode groups of tokens while the stream and output have room for a
/// whole group and `P` fits. A token the group cannot take (a literal
/// extension, an invalid code, or a match reaching before the output start)
/// ends its group and is decoded with every check.
#[inline(always)]
fn groups<const COND: bool, P: Placement>(
    src: &[u8],
    dst: &mut [u8],
    cursor: &mut Cursor,
    end: usize,
) {
    let Cursor {
        mut control,
        mut literals,
        mut out,
    } = *cursor;
    while literals - control >= GROUP_SRC && end - out >= GROUP_OUT && P::fits(out, dst.len()) {
        let mut group = Group {
            ctl: array(src, control),
            lits: array(src, literals - GROUP_LITS),
            lit: GROUP_LITS,
            at: out,
            tokens: 0,
        };
        let done = P::run::<COND>(&mut group, dst);
        control += group.tokens * TOKEN_LEN;
        literals -= GROUP_LITS - group.lit;
        out = group.at;
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
    /// Output position: in `dst`, or in the group's window while it runs.
    at: usize,
    tokens: usize,
}

impl Group<'_> {
    /// Decode tokens `n..n + 4`, each in a window of its own. Returns
    /// `false` at the first token the group cannot take.
    #[inline(always)]
    fn quad_windows<const COND: bool>(&mut self, dst: &mut [u8], n: usize) -> bool {
        self.token_window::<COND>(dst, n)
            && self.token_window::<COND>(dst, n + 1)
            && self.token_window::<COND>(dst, n + 2)
            && self.token_window::<COND>(dst, n + 3)
    }

    /// Decode the group in `win`, which starts at output `base`.
    #[inline(always)]
    fn in_window<const COND: bool, const WIN: usize>(
        &mut self,
        win: &mut [u8; WIN],
        base: usize,
    ) -> bool {
        self.at -= base;
        let done = self.quad_in::<COND, WIN>(win, 0)
            && self.quad_in::<COND, WIN>(win, 4)
            && self.quad_in::<COND, WIN>(win, 8)
            && self.quad_in::<COND, WIN>(win, 12);
        self.at += base;
        done
    }

    /// Decode tokens `n..n + 4` in the group window `win`. Returns `false`
    /// at the first token the group cannot take.
    #[inline(always)]
    fn quad_in<const COND: bool, const WIN: usize>(
        &mut self,
        win: &mut [u8; WIN],
        n: usize,
    ) -> bool {
        self.token_in::<COND, WIN>(win, n)
            && self.token_in::<COND, WIN>(win, n + 1)
            && self.token_in::<COND, WIN>(win, n + 2)
            && self.token_in::<COND, WIN>(win, n + 3)
    }

    /// Fields of token `n`.
    #[inline(always)]
    fn fields(&self, n: usize) -> Fields {
        Fields::parse(u32::from_le_bytes(load(self.ctl, n * TOKEN_LEN)))
    }

    /// Decode token `n` in the group window `win`. Returns `false`,
    /// consuming nothing, if the group cannot take it.
    #[inline(always)]
    fn token_in<const COND: bool, const WIN: usize>(
        &mut self,
        win: &mut [u8; WIN],
        n: usize,
    ) -> bool {
        let f = self.fields(n);
        if f.lit_len > INLINE_LIT_MAX || f.match_len == 0 {
            return false;
        }
        let Some(from) = (self.at + f.lit_len).checked_sub(f.dis) else {
            return false;
        };
        self.copy::<COND, false>(win, self.at, from, f);
        self.at += f.lit_len + f.match_len;
        self.tokens = n + 1;
        true
    }

    /// Decode token `n` in a window of its own: `dis` bytes of history,
    /// then `TOKEN_ROOM` bytes of output. Returns `false`, consuming
    /// nothing, if the group cannot take it.
    #[inline(always)]
    fn token_window<const COND: bool>(&mut self, dst: &mut [u8], n: usize) -> bool {
        let f = self.fields(n);
        if f.lit_len > INLINE_LIT_MAX || f.match_len == 0 {
            return false;
        }
        let Some(start) = self.at.checked_sub(f.dis) else {
            return false;
        };
        let win = &mut dst[start..][..f.dis + TOKEN_ROOM];
        self.copy::<COND, true>(win, f.dis, f.lit_len, f);
        self.at += f.lit_len + f.match_len;
        self.tokens = n + 1;
        true
    }

    /// Copy the token's literals to `win[to..]`, then its match from
    /// `win[from..]`, in whole chunks. The caller sizes `win` to hold every
    /// chunk.
    ///
    /// `SPLIT` stores through `win[to..]`, for token windows: its length
    /// is a constant. Group windows index `win` directly, as the compiler
    /// bounds `to` itself.
    #[inline(always)]
    fn copy<const COND: bool, const SPLIT: bool>(
        &mut self,
        win: &mut [u8],
        to: usize,
        from: usize,
        f: Fields,
    ) {
        let put = |win: &mut [u8], off: usize, chunk: [u8; CHUNK]| {
            if SPLIT {
                store(&mut win[to..], off, chunk);
            } else {
                store(win, to + off, chunk);
            }
        };
        self.lit -= f.lit_len;
        put(win, 0, load(self.lits, self.lit));
        if f.lit_len > CHUNK {
            put(win, CHUNK, load(self.lits, self.lit + CHUNK));
        }
        put(win, f.lit_len, load(win, from));
        if !COND || f.match_len > CHUNK {
            put(win, f.lit_len + CHUNK, load(win, from + CHUNK));
            let mut copied = 2 * CHUNK;
            while copied < f.match_len {
                put(win, f.lit_len + copied, load(win, from + copied));
                copied += CHUNK;
            }
        }
    }
}

/// Decode one token with every check: tokens a group cannot take and tokens
/// near the end of either stream.
#[inline(never)]
fn checked_step(src: &[u8], dst: &mut [u8], cursor: &mut Cursor, end: usize) -> Result<(), Error> {
    if cursor.literals - cursor.control < TOKEN_LEN {
        return Err(Error::CorruptInput);
    }
    let Fields {
        mut lit_len,
        match_len,
        dis,
    } = Fields::parse(u32::from_le_bytes(load(src, cursor.control)));
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
