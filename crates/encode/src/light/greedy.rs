//! Greedy light levels `-1` to `2`: probe a hash table per position, defer
//! short matches as candidates, and emit when a candidate fires.

use fearless_simd::Simd;
use m77rip_core::format::{
    HASHTAB_LAG, LITERAL_SUFFIX, MAX_MATCH_LEN, MIN_MATCH_LEN, TOKEN_LIT_MAX,
};

use super::matchers::Matcher;
use super::writer::LightWriter;

/// Misses before the probe step grows by one.
const SKIP_SHIFT: usize = 6;
/// Pending literals at which a deferred candidate fires.
const FIRE_AT: usize = 6;
/// Upper bound of the literal-run regime vote.
const REGIME_CAP: i32 = 64;
/// Regime vote above which a level treats the input as literal-heavy.
const REGIME_THRESHOLD: i32 = 32;
/// Keen lookahead: stop probing ahead once the match is this long.
const LOOKAHEAD_GATE: usize = 16;
/// Keen lookahead: stop after a probe that does not improve a match this long.
const LOOKAHEAD_PATIENCE: usize = 8;

/// Match acceptance rules of a greedy level.
pub(crate) trait Policy {
    /// Positions to advance after a miss, before the skip acceleration.
    const STEP: usize;
    /// Probe the next two positions for a longer match before accepting.
    const LOOKAHEAD: bool;
    /// Shortest match to accept at once.
    fn accept_len(regime: i32) -> usize;
    /// Whether a deferred candidate fires with `pending` literals.
    fn fire(pending: usize, regime: i32) -> bool;
    /// Update the regime after a token with `lit_len` literals.
    fn vote(regime: &mut i32, lit_len: usize);
}

/// Level `-1`.
pub(crate) struct Blitz;
/// Levels `0` and `1`.
pub(crate) struct Loose;
/// Level `2`.
pub(crate) struct Keen;

impl Policy for Blitz {
    const STEP: usize = 2;
    const LOOKAHEAD: bool = false;

    fn accept_len(_regime: i32) -> usize {
        9
    }

    fn fire(pending: usize, _regime: i32) -> bool {
        pending + Self::STEP > FIRE_AT
    }

    fn vote(_regime: &mut i32, _lit_len: usize) {}
}

impl Policy for Loose {
    const STEP: usize = 1;
    const LOOKAHEAD: bool = false;

    fn accept_len(_regime: i32) -> usize {
        7
    }

    fn fire(pending: usize, regime: i32) -> bool {
        if regime >= REGIME_THRESHOLD {
            pending >= FIRE_AT
        } else {
            pending == FIRE_AT
        }
    }

    fn vote(regime: &mut i32, lit_len: usize) {
        let vote = if (7..=32).contains(&lit_len) { 2 } else { -1 };
        *regime = (*regime + vote).clamp(0, REGIME_CAP);
    }
}

impl Policy for Keen {
    const STEP: usize = 1;
    const LOOKAHEAD: bool = true;

    fn accept_len(regime: i32) -> usize {
        if regime >= REGIME_THRESHOLD {
            MIN_MATCH_LEN
        } else {
            6
        }
    }

    fn fire(pending: usize, _regime: i32) -> bool {
        pending >= FIRE_AT
    }

    fn vote(regime: &mut i32, lit_len: usize) {
        let vote = if lit_len >= TOKEN_LIT_MAX { 3 } else { -1 };
        *regime = (*regime + vote).clamp(0, REGIME_CAP);
    }
}

/// A match: start, length, and source position.
#[derive(Clone, Copy)]
struct Match {
    pos: usize,
    len: usize,
    src: usize,
}

/// Input of the greedy loop: matches end by `end`.
struct Input<'a, S> {
    simd: S,
    src: &'a [u8],
    end: usize,
}

/// Parse state of the greedy loop.
struct Greedy {
    pos: usize,
    /// Start of the pending literals.
    lit: usize,
    misses: usize,
    /// Deferred candidate; `len == 0` when none.
    cand: Match,
    regime: i32,
}

/// Compress `src` into `dst` with matcher `M` and policy `P`. Returns the
/// stream length.
#[inline(always)]
pub(crate) fn compress<S: Simd, M: Matcher, P: Policy>(
    simd: S,
    src: &[u8],
    dst: &mut [u8],
) -> usize {
    let mut out = match LightWriter::start(src, dst) {
        Ok(out) => out,
        Err(len) => return len,
    };
    let input = Input {
        simd,
        src,
        end: src.len() - LITERAL_SUFFIX,
    };
    let mut matcher = M::new();
    let none = Match {
        pos: 0,
        len: 0,
        src: 0,
    };
    let mut state = Greedy {
        pos: 0,
        lit: 0,
        misses: 0,
        cand: none,
        regime: 0,
    };
    while state.pos + MAX_MATCH_LEN <= input.end {
        matcher.insert_until(src, state.pos);
        let found = if state.pos > HASHTAB_LAG {
            matcher.find(simd, src, state.pos, state.cand.len != 0)
        } else {
            (0, 0)
        };
        let probed = state.pos;
        let Some(found) = state.select::<S, M, P>(&input, &matcher, found) else {
            state.pos += P::STEP + (state.misses >> SKIP_SHIFT);
            state.misses += 1;
            continue;
        };
        state.misses = 0;
        let found = extend_backward(src, state.lit, found);
        P::vote(&mut state.regime, found.pos - state.lit);
        out.token(&src[state.lit..found.pos], found.len, found.pos - found.src);
        state.lit = found.pos + found.len;
        state.pos = state.lit.max(probed);
        state.cand = none;
    }
    out.finish(&src[state.lit..])
}

impl Greedy {
    /// Decide on the match `found` at `self.pos`: accept it, accept the
    /// deferred candidate, or defer. Returns the match to emit.
    #[inline(always)]
    fn select<S: Simd, M: Matcher, P: Policy>(
        &mut self,
        input: &Input<'_, S>,
        matcher: &M,
        (len, src_pos): (usize, usize),
    ) -> Option<Match> {
        let mut found = Match {
            pos: self.pos,
            len,
            src: src_pos,
        };
        if found.len >= P::accept_len(self.regime) {
            if P::LOOKAHEAD {
                found = lookahead(input, matcher, found);
            }
            return Some(found);
        }
        if P::fire(found.pos - self.lit, self.regime) {
            let cand = self.cand;
            if cand.len != 0
                && (found.len < MIN_MATCH_LEN || cand.pos + cand.len >= found.pos + found.len)
            {
                found = cand;
            }
            return (found.len >= MIN_MATCH_LEN).then_some(found);
        }
        if found.len >= MIN_MATCH_LEN
            && (self.cand.len == 0 || found.pos + found.len >= self.cand.pos + self.cand.len)
        {
            self.cand = found;
        }
        None
    }
}

/// Probe the next two positions for a longer match.
#[inline(always)]
fn lookahead<S: Simd, M: Matcher>(input: &Input<'_, S>, matcher: &M, mut found: Match) -> Match {
    let base = found.pos;
    for pos in base + 1..=base + 2 {
        if pos + MAX_MATCH_LEN > input.end || found.len >= LOOKAHEAD_GATE {
            break;
        }
        let (len, src_pos) = matcher.find(input.simd, input.src, pos, false);
        if len > found.len {
            found = Match {
                pos,
                len,
                src: src_pos,
            };
        } else if found.len >= LOOKAHEAD_PATIENCE {
            break;
        }
    }
    found
}

/// Extend a match backward over equal bytes, into the pending literals and
/// up to `MAX_MATCH_LEN`.
#[inline(always)]
fn extend_backward(src: &[u8], lit: usize, found: Match) -> Match {
    let limit = (found.pos - lit)
        .min(found.src)
        .min(MAX_MATCH_LEN - found.len);
    let back = src[..found.pos]
        .iter()
        .rev()
        .zip(src[..found.src].iter().rev())
        .take(limit)
        .take_while(|(a, b)| a == b)
        .count();
    Match {
        pos: found.pos - back,
        len: found.len + back,
        src: found.src - back,
    }
}
