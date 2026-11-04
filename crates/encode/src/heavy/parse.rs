//! Exact block parse of level `4`: a shortest-path pass over token costs in
//! bytes, then a backtrack to the tokens.

use m77rip_core::format::{HEAVY_CODE_FLOOR, HEAVY_LEN_OF, HEAVY_TOKEN_LIT_MAX, MIN_MATCH_LEN};

use crate::read::array_mut;
use crate::sa::{Block, SaWorkspace};

/// Cost of an unreached position.
const INF: usize = usize::MAX;
/// Bytes of a token header.
const TOKEN_COST: usize = 4;
/// Relaxation window past a position: any `u8` match length. The DP
/// arrays carry this many padding entries, so a window never runs out.
const RELAX_WINDOW: usize = 256;

/// The committed parse so far: it ends at `pos` and costs `cost` bytes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frontier {
    pub(crate) pos: usize,
    pub(crate) cost: usize,
}

/// A parsed match: start, length, and distance.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TokenRecord {
    pub(crate) match_start: usize,
    pub(crate) len: u8,
    pub(crate) dis: u32,
}

/// Parse buffers, reused across blocks and calls.
#[derive(Default)]
pub(crate) struct ParseBuffers {
    /// Cheapest cost to reach each block position with a match.
    dp: Vec<usize>,
    /// Length of the match that reaches each position at that cost.
    arrival_len: Vec<u8>,
    /// Tokens of the last parsed block, last token first.
    pub(crate) tokens: Vec<TokenRecord>,
}

/// Extension bytes of a literal run of `run` bytes.
fn literal_extras(run: usize) -> usize {
    if run < HEAVY_TOKEN_LIT_MAX {
        0
    } else {
        1 + (run - HEAVY_TOKEN_LIT_MAX) / 255
    }
}

/// Cost of the cheapest parse that ends in a literal run at the current
/// position.
struct LiteralRun {
    run: usize,
    cost: usize,
    /// Run length at which the next extension byte starts.
    next_extra_at: usize,
}

impl LiteralRun {
    fn new(run: usize, base: usize) -> Self {
        let next_extra_at = if run < HEAVY_TOKEN_LIT_MAX {
            HEAVY_TOKEN_LIT_MAX
        } else {
            run + 255 - (run - HEAVY_TOKEN_LIT_MAX) % 255
        };
        Self {
            run,
            cost: base + run + literal_extras(run),
            next_extra_at,
        }
    }

    /// Extend the run by one byte, or restart it at a cheaper match end.
    #[inline(always)]
    fn advance(&mut self, reached: usize) {
        self.run += 1;
        if self.run >= self.next_extra_at {
            self.cost += 1;
            self.next_extra_at = self.run + 255;
        }
        self.cost += 1;
        if reached <= self.cost {
            self.cost = reached;
            self.run = 0;
            self.next_extra_at = HEAVY_TOKEN_LIT_MAX;
        }
    }
}

impl ParseBuffers {
    /// Parse `block` on from `frontier`, filling `tokens`. Returns the new
    /// frontier, or `None` if the backtrack finds no consistent parse.
    pub(crate) fn parse_block(
        &mut self,
        matches: &SaWorkspace,
        src_len: usize,
        block: Block,
        frontier: Frontier,
    ) -> Option<Frontier> {
        let lits = self.relax(matches, block, frontier);
        let commit = if block.end < src_len {
            Frontier {
                pos: block.end - lits.run,
                cost: lits.cost - lits.run - literal_extras(lits.run),
            }
        } else {
            self.best_end(src_len, block, frontier)
        };
        self.backtrack(matches, block, frontier, commit)?;
        Some(commit)
    }

    /// Fill `dp` and `arrival_len` for the block. Returns the literal run
    /// that ends at the block end.
    fn relax(&mut self, matches: &SaWorkspace, block: Block, frontier: Frontier) -> LiteralRun {
        let len = block.len();
        self.dp.clear();
        self.dp.resize(len + RELAX_WINDOW, INF);
        self.arrival_len.resize(len + RELAX_WINDOW, 0);
        let mut lits = LiteralRun::new(block.start - frontier.pos, frontier.cost);
        if block.start == 0 {
            self.dp[0] = frontier.cost;
        }
        for i in 0..=len {
            if i > 0 {
                lits.advance(self.dp[i]);
            }
            let longest = if i < len { matches.max_len[i] } else { 0 };
            if usize::from(longest) >= MIN_MATCH_LEN {
                self.relax_from(i, lits.cost + TOKEN_COST, longest);
            }
        }
        lits
    }

    /// Offer a token at `i` costing `cost` for each encodable length up to
    /// `longest`.
    #[inline(always)]
    fn relax_from(&mut self, i: usize, cost: usize, longest: u8) {
        let top_code = usize::from(HEAVY_CODE_FLOOR[usize::from(longest)]);
        let dp = array_mut::<_, RELAX_WINDOW>(&mut self.dp, i);
        let arrival_len = array_mut::<_, RELAX_WINDOW>(&mut self.arrival_len, i);
        for &len in &HEAVY_LEN_OF[1..=top_code] {
            let target = usize::from(len);
            if cost < dp[target] {
                dp[target] = cost;
                arrival_len[target] = len;
            }
        }
    }

    /// Cheapest end of the last block: a reached position plus raw suffix.
    fn best_end(&self, src_len: usize, block: Block, frontier: Frontier) -> Frontier {
        let mut best = frontier;
        let mut best_total = frontier.cost + (src_len - frontier.pos);
        for (i, &cost) in self.dp[..=block.len()].iter().enumerate() {
            let pos = block.start + i;
            if cost != INF && cost + (src_len - pos) < best_total {
                best_total = cost + (src_len - pos);
                best = Frontier { pos, cost };
            }
        }
        best
    }

    /// Walk back from `commit` to `frontier`, collecting the tokens.
    fn backtrack(
        &mut self,
        matches: &SaWorkspace,
        block: Block,
        frontier: Frontier,
        commit: Frontier,
    ) -> Option<()> {
        self.tokens.clear();
        let mut boundary = commit.pos;
        while boundary > frontier.pos {
            let arrival = boundary.checked_sub(block.start)?;
            let len = self.arrival_len[arrival];
            let match_start = boundary.checked_sub(usize::from(len))?;
            if usize::from(len) < MIN_MATCH_LEN || match_start < block.start {
                return None;
            }
            let literal_cost = self.dp[arrival].checked_sub(TOKEN_COST)?;
            let prev = self.literal_origin(block, frontier, match_start, literal_cost)?;
            self.tokens.push(TokenRecord {
                match_start,
                len,
                dis: matches.match_dis[match_start - block.start],
            });
            boundary = prev;
        }
        Some(())
    }

    /// Start of the literal run before `match_start` whose parse costs
    /// `literal_cost`: the nearest reached position, else the frontier.
    fn literal_origin(
        &self,
        block: Block,
        frontier: Frontier,
        match_start: usize,
        literal_cost: usize,
    ) -> Option<usize> {
        for prev in (block.start..=match_start).rev() {
            let run = match_start - prev;
            let cost = self.dp[prev - block.start];
            if cost != INF && cost + run + literal_extras(run) == literal_cost {
                return Some(prev);
            }
        }
        let run = match_start - frontier.pos;
        (frontier.pos <= block.start && frontier.cost + run + literal_extras(run) == literal_cost)
            .then_some(frontier.pos)
    }
}
