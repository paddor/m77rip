//! Light level `3`: suffix-array matches with a one-step lazy parse.

use fearless_simd::Simd;
use m77rip_core::format::{
    DIS_LIM, LITERAL_SUFFIX, MAX_DISTANCE, MAX_MATCH_LEN, MIN_DISTANCE, MIN_MATCH_LEN,
};

use super::writer::LightWriter;
use crate::sa::{Block, Limits, SaWorkspace};

/// Input bytes per suffix-array block.
const BLOCK_SIZE: usize = 3 << 17;

const _: () = assert!(BLOCK_SIZE <= u32::MAX as usize);

/// Light-format match limits.
struct Light;

impl Limits for Light {
    const MIN_DISTANCE: usize = MIN_DISTANCE;
    const MAX_DISTANCE: usize = MAX_DISTANCE;
    const MAX_MATCH_LEN: usize = MAX_MATCH_LEN;
    const EXTEND_LIMIT: usize = DIS_LIM;
    const PAD_LEN: usize = MAX_MATCH_LEN + 1;
}

#[cfg(feature = "std")]
std::thread_local! {
    static WORKSPACE: core::cell::RefCell<SaWorkspace> = core::cell::RefCell::default();
}

/// Run `f` with this thread's workspace.
#[inline(always)]
pub(crate) fn with_workspace<R>(f: impl FnOnce(&mut SaWorkspace) -> R) -> R {
    #[cfg(feature = "std")]
    {
        crate::sa::with_workspace(&WORKSPACE, f)
    }
    #[cfg(not(feature = "std"))]
    {
        f(&mut SaWorkspace::default())
    }
}

/// Compress `src` into `dst`. Returns the stream length.
#[inline(always)]
pub(crate) fn compress<S: Simd>(
    simd: S,
    workspace: &mut SaWorkspace,
    src: &[u8],
    dst: &mut [u8],
) -> usize {
    let mut out = match LightWriter::start(src, dst) {
        Ok(out) => out,
        Err(len) => return len,
    };
    let match_end = src.len() - LITERAL_SUFFIX;
    let mut lit = 0;
    for start in (0..src.len()).step_by(BLOCK_SIZE) {
        let end = (start + BLOCK_SIZE).min(src.len());
        let block = Block {
            start,
            end,
            hard_end: end.min(match_end),
        };
        workspace.find_block::<S, Light>(simd, src, block);
        lit = parse_block(&mut out, src, workspace, block, lit);
    }
    out.finish(&src[lit..])
}

/// Emit the matches of `block`, skipping a match when the next position
/// has a longer one. Returns the start of the pending literals.
fn parse_block(
    out: &mut LightWriter<'_>,
    src: &[u8],
    workspace: &SaWorkspace,
    block: Block,
    mut lit: usize,
) -> usize {
    let mut pos = block.start;
    while pos < block.hard_end {
        let i = pos - block.start;
        let len = usize::from(workspace.max_len[i]);
        if len < MIN_MATCH_LEN
            || (pos + 1 < block.hard_end && usize::from(workspace.max_len[i + 1]) > len)
        {
            pos += 1;
            continue;
        }
        out.token(&src[lit..pos], len, workspace.match_dis[i] as usize);
        pos += len;
        lit = pos;
    }
    lit
}
