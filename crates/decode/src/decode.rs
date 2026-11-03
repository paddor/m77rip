#[cfg(feature = "alloc")]
use alloc::vec;

use crate::{heavy, light};
use m77rip_core::Error;
use m77rip_core::format::{
    EXT_HEADER_SIZE, FLAG_HEAVY, FLAG_HEAVY_COND, FLAG_SHIFT, HEADER_SIZE, HEAVY_LITERAL_SUFFIX,
    HEAVY_SMALL_LIM, LITERAL_SUFFIX, SIZE_MASK, SMALL_LIM,
};

/// Stream regions shared by the light and heavy decoders.
///
/// Tokens run forward from `EXT_HEADER_SIZE`, literal runs backward from
/// `suffix_start`, and the raw suffix fills the output after
/// `token_output_end`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    pub(crate) suffix_start: usize,
    pub(crate) token_output_end: usize,
}

/// Decoder positions: next token byte, end of the unread literal runs, and
/// output cursor.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cursor {
    pub(crate) control: usize,
    pub(crate) literals: usize,
    pub(crate) out: usize,
}

impl Layout {
    /// Validate the extended header. `dst` is the output, cut to the
    /// decompressed size.
    fn parse(src: &[u8], dst: &[u8], min_suffix: usize) -> Result<Self, Error> {
        let header: [u8; 8] = src
            .get(HEADER_SIZE..EXT_HEADER_SIZE)
            .ok_or(Error::InputTooShort)?
            .try_into()
            .unwrap();
        let suffix_u64 = u64::from_le_bytes(header);
        let suffix: usize = suffix_u64
            .try_into()
            .map_err(|_| Error::SizeOverflow { size: suffix_u64 })?;
        if suffix < min_suffix || suffix > dst.len() {
            return Err(Error::CorruptInput);
        }
        let suffix_start = src
            .len()
            .checked_sub(suffix)
            .filter(|&start| start >= EXT_HEADER_SIZE)
            .ok_or(Error::InputTooShort)?;
        Ok(Self {
            suffix_start,
            token_output_end: dst.len() - suffix,
        })
    }

    pub(crate) fn start(self) -> Cursor {
        Cursor {
            control: EXT_HEADER_SIZE,
            literals: self.suffix_start,
            out: 0,
        }
    }

    /// Check that the tokens filled the output up to the suffix, then copy
    /// the suffix.
    fn finish(self, src: &[u8], dst: &mut [u8], cursor: Cursor) -> Result<(), Error> {
        if cursor.out != self.token_output_end || cursor.control != cursor.literals {
            return Err(Error::CorruptInput);
        }
        dst[self.token_output_end..].copy_from_slice(&src[self.suffix_start..]);
        Ok(())
    }
}

fn header_fields(src: &[u8]) -> Result<(usize, u8), Error> {
    let header = src
        .first_chunk::<HEADER_SIZE>()
        .map(|header| u64::from_le_bytes(*header))
        .ok_or(Error::InputTooShort)?;
    let size = header & SIZE_MASK;
    let size = size.try_into().map_err(|_| Error::SizeOverflow { size })?;
    Ok((size, (header >> FLAG_SHIFT) as u8))
}

/// Reads the decompressed size from the first 8 bytes of a compressed stream.
///
/// Returns `None` if `src` is shorter than 8 bytes.
#[must_use]
pub fn decompressed_size(src: &[u8]) -> Option<u64> {
    src.first_chunk::<HEADER_SIZE>()
        .map(|header| u64::from_le_bytes(*header) & SIZE_MASK)
}

/// Decompresses a misa77-compressed stream into a new `Vec<u8>`.
///
/// `expected_len` is the expected decompressed size. It must match the size
/// encoded in the stream header.
///
/// # Errors
///
/// Returns [`Error::SizeMismatch`] if the header disagrees with
/// `expected_len`, and the errors of [`decompress_into`].
#[cfg(feature = "alloc")]
pub fn decompress(src: &[u8], expected_len: usize) -> Result<alloc::vec::Vec<u8>, Error> {
    let (actual_len, _) = header_fields(src)?;
    if actual_len != expected_len {
        return Err(Error::SizeMismatch {
            expected: expected_len,
            actual: actual_len,
        });
    }
    let mut dst = vec![0u8; expected_len];
    let written = decompress_into(src, &mut dst)?;
    debug_assert_eq!(written, expected_len);
    Ok(dst)
}

/// Decompresses a misa77-compressed stream into the provided buffer.
///
/// Returns the number of bytes written to `dst`.
///
/// # Errors
///
/// Returns [`Error::OutputTooSmall`] if `dst` cannot hold the decompressed
/// size, [`Error::InputTooShort`] for a truncated stream, and
/// [`Error::CorruptInput`] for a malformed one.
pub fn decompress_into(src: &[u8], dst: &mut [u8]) -> Result<usize, Error> {
    let (original_size, flags) = header_fields(src)?;
    let have = dst.len();
    let dst = dst.get_mut(..original_size).ok_or(Error::OutputTooSmall {
        need: original_size,
        have,
    })?;

    let heavy = flags & FLAG_HEAVY != 0;
    let (raw_limit, min_suffix) = if heavy {
        (HEAVY_SMALL_LIM, HEAVY_LITERAL_SUFFIX)
    } else {
        (SMALL_LIM, LITERAL_SUFFIX)
    };
    if original_size <= raw_limit {
        let payload = src
            .get(HEADER_SIZE..HEADER_SIZE + original_size)
            .ok_or(Error::InputTooShort)?;
        dst.copy_from_slice(payload);
        return Ok(original_size);
    }

    let layout = Layout::parse(src, dst, min_suffix)?;
    #[cfg(feature = "std")]
    let level = fearless_simd::Level::new();
    #[cfg(not(feature = "std"))]
    let level = fearless_simd::Level::baseline();
    let cursor = match (heavy, flags & FLAG_HEAVY_COND != 0) {
        (false, _) => {
            fearless_simd::dispatch!(level, simd => light::decode(simd, src, dst, layout))
        }
        (true, true) => fearless_simd::dispatch!(
            level, simd => heavy::decode::<_, true>(simd, src, dst, layout)
        ),
        (true, false) => fearless_simd::dispatch!(
            level, simd => heavy::decode::<_, false>(simd, src, dst, layout)
        ),
    }?;
    layout.finish(src, dst, cursor)?;
    Ok(original_size)
}

#[cfg(test)]
mod tests;
