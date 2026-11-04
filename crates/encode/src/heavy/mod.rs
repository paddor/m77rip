//! Heavy level `4`: suffix-array matches with an exact block parse.

mod parse;
mod writer;

use fearless_simd::Simd;
use m77rip_core::format::{
    EXT_HEADER_SIZE, HEAVY_LITERAL_SUFFIX, HEAVY_MAX_DISTANCE, HEAVY_MAX_MATCH_LEN,
    HEAVY_MIN_DISTANCE, HEAVY_NDIS,
};

use crate::sa::{Block, Limits, SaWorkspace};
use parse::{Frontier, ParseBuffers};
use writer::HeavyWriter;

/// Input bytes per suffix-array block.
const BLOCK_SIZE: usize = 1 << 21;

const _: () = assert!(BLOCK_SIZE <= u32::MAX as usize);
const _: () = assert!(HEAVY_MAX_DISTANCE <= u32::MAX as usize);
const _: () = assert!(HEAVY_MAX_MATCH_LEN <= u8::MAX as usize);

/// Heavy-format match limits.
struct Heavy;

impl Limits for Heavy {
    const MIN_DISTANCE: usize = HEAVY_MIN_DISTANCE;
    const MAX_DISTANCE: usize = HEAVY_MAX_DISTANCE;
    const MAX_MATCH_LEN: usize = HEAVY_MAX_MATCH_LEN;
    const EXTEND_LIMIT: usize = HEAVY_NDIS;
    const PAD_LEN: usize = HEAVY_MAX_MATCH_LEN + 1;
}

/// Match finder and parse buffers, reused across calls.
#[derive(Default)]
pub(crate) struct Workspace {
    matches: SaWorkspace,
    parse: ParseBuffers,
}

#[cfg(feature = "std")]
std::thread_local! {
    static WORKSPACE: core::cell::RefCell<Workspace> = core::cell::RefCell::default();
}

/// Run `f` with this thread's workspace.
#[inline(always)]
pub(crate) fn with_workspace<R>(f: impl FnOnce(&mut Workspace) -> R) -> R {
    #[cfg(feature = "std")]
    {
        crate::sa::with_workspace(&WORKSPACE, f)
    }
    #[cfg(not(feature = "std"))]
    {
        f(&mut Workspace::default())
    }
}

/// Compress `src` into `dst`. Returns the stream length.
#[inline(always)]
pub(crate) fn compress<S: Simd>(
    simd: S,
    workspace: &mut Workspace,
    src: &[u8],
    dst: &mut [u8],
) -> usize {
    let mut out = match HeavyWriter::start(src, dst) {
        Ok(out) => out,
        Err(len) => return len,
    };
    let match_end = src.len() - HEAVY_LITERAL_SUFFIX;
    let mut frontier = Frontier {
        pos: 0,
        cost: EXT_HEADER_SIZE,
    };
    for start in (0..src.len()).step_by(BLOCK_SIZE) {
        let end = (start + BLOCK_SIZE).min(src.len());
        let block = Block {
            start,
            end,
            hard_end: end.min(match_end),
        };
        workspace.matches.find_block::<S, Heavy>(simd, src, block);
        let Some(next) =
            workspace
                .parse
                .parse_block(&workspace.matches, src.len(), block, frontier)
        else {
            return 0;
        };
        for &token in workspace.parse.tokens.iter().rev() {
            out.token(src, token);
        }
        frontier = next;
    }
    out.finish(src)
}
