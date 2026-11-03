//! Public compression API and level dispatch.

use fearless_simd::Level;
use m77rip_core::Error;
use m77rip_core::format::{
    DEFAULT_LEVEL, EXT_HEADER_SIZE, HEADER_SIZE, HEAVY_LEVEL_MIN, MAX_LEVEL, MIN_LEVEL, SMALL_LIM,
};

use crate::light::greedy::{self, Blitz, Keen, Loose, Policy};
use crate::light::matchers::{KEEN_WIDTH, LOOSE_WIDTH, Latest6, Matcher, Ring, Swift};
use crate::{heavy, light};

/// Returns the maximum compressed size for a given input size.
#[must_use]
pub fn compress_bound(src_size: usize) -> usize {
    bound_light(src_size).max(bound_heavy(src_size))
}

/// Returns the maximum compressed size for a given input size and level.
///
/// # Errors
///
/// Returns [`Error::InvalidLevel`] for levels outside `-1..=4`.
pub fn compress_bound_level(src_size: usize, level: i8) -> Result<usize, Error> {
    validate_level(level)?;
    Ok(bound_for_level(src_size, level))
}

fn bound_for_level(src_size: usize, level: i8) -> usize {
    if level >= HEAVY_LEVEL_MIN {
        bound_heavy(src_size)
    } else {
        bound_light(src_size)
    }
}

fn bound_light(src_size: usize) -> usize {
    if src_size <= SMALL_LIM {
        return HEADER_SIZE.saturating_add(src_size);
    }
    EXT_HEADER_SIZE
        .saturating_add(src_size)
        .saturating_add(src_size / 255)
        .saturating_add(16)
}

fn bound_heavy(src_size: usize) -> usize {
    EXT_HEADER_SIZE
        .saturating_add(src_size)
        .saturating_add(src_size / 32)
        .saturating_add(64)
}

/// Compresses `input` into the misa77 stream format (level 1, default).
#[must_use]
pub fn compress(input: &[u8]) -> Vec<u8> {
    let mut dst = vec![0u8; bound_for_level(input.len(), DEFAULT_LEVEL)];
    let written = dispatch(input, &mut dst, DEFAULT_LEVEL);
    dst.truncate(written);
    dst
}

/// Compresses `input` at the given level (-1..=4). Levels below 4 emit the
/// light format; level 4 emits the heavy format.
///
/// # Errors
///
/// Returns [`Error::InvalidLevel`] for any other level.
pub fn compress_level(input: &[u8], level: i8) -> Result<Vec<u8>, Error> {
    validate_level(level)?;
    let mut dst = vec![0u8; bound_for_level(input.len(), level)];
    let written = dispatch(input, &mut dst, level);
    dst.truncate(written);
    Ok(dst)
}

/// Compresses `input` into `dst` (level 1, default).
///
/// Returns the number of bytes written to `dst`.
///
/// # Errors
///
/// Returns [`Error::OutputTooSmall`] if `dst` is shorter than
/// [`compress_bound_level`] for the default level.
pub fn compress_into(input: &[u8], dst: &mut [u8]) -> Result<usize, Error> {
    compress_into_level(input, dst, DEFAULT_LEVEL)
}

/// Compresses `input` into `dst` at the given level.
///
/// Returns the number of bytes written to `dst`.
///
/// # Errors
///
/// Returns [`Error::InvalidLevel`] for levels outside `-1..=4` and
/// [`Error::OutputTooSmall`] if `dst` is shorter than
/// [`compress_bound_level`].
pub fn compress_into_level(input: &[u8], dst: &mut [u8], level: i8) -> Result<usize, Error> {
    validate_level(level)?;
    let bound = bound_for_level(input.len(), level);
    if dst.len() < bound {
        return Err(Error::OutputTooSmall {
            need: bound,
            have: dst.len(),
        });
    }
    Ok(dispatch(input, dst, level))
}

fn validate_level(level: i8) -> Result<(), Error> {
    if (MIN_LEVEL..=MAX_LEVEL).contains(&level) {
        Ok(())
    } else {
        Err(Error::InvalidLevel { level })
    }
}

/// Run the encoder of a valid `level`. `dst` holds the level's bound.
fn dispatch(src: &[u8], dst: &mut [u8], level: i8) -> usize {
    match level {
        -1 => greedy_level::<Latest6, Blitz>(src, dst),
        0 => greedy_level::<Swift, Loose>(src, dst),
        1 => greedy_level::<Ring<LOOSE_WIDTH>, Loose>(src, dst),
        2 => greedy_level::<Ring<KEEN_WIDTH>, Keen>(src, dst),
        3 => optimal_level(src, dst),
        _ => heavy_level(src, dst),
    }
}

#[inline(never)]
fn greedy_level<M: Matcher, P: Policy>(src: &[u8], dst: &mut [u8]) -> usize {
    fearless_simd::dispatch!(Level::new(), simd => greedy::compress::<_, M, P>(simd, src, dst))
}

#[inline(never)]
fn optimal_level(src: &[u8], dst: &mut [u8]) -> usize {
    light::optimal::with_workspace(
        |workspace| fearless_simd::dispatch!(Level::new(), simd => light::optimal::compress(simd, workspace, src, dst)),
    )
}

#[inline(never)]
fn heavy_level(src: &[u8], dst: &mut [u8]) -> usize {
    heavy::with_workspace(
        |workspace| fearless_simd::dispatch!(Level::new(), simd => heavy::compress(simd, workspace, src, dst)),
    )
}

#[cfg(test)]
mod tests;
