#[cfg(feature = "alloc")]
use alloc::vec;

use crate::primitives;
use m77rip_core::Error;
use m77rip_core::format::*;

#[cfg(not(feature = "paranoid"))]
#[allow(unused_imports)]
use fearless_simd::Level;
#[cfg(not(feature = "paranoid"))]
use fearless_simd::Simd;

/// Reads the decompressed size from the first 8 bytes of a compressed stream.
///
/// Returns `None` if `src` is shorter than 8 bytes.
pub fn decompressed_size(src: &[u8]) -> Option<u64> {
    if src.len() < HEADER_SIZE {
        return None;
    }
    Some(u64::from_le_bytes(src[..8].try_into().unwrap()))
}

/// Decompresses a misa77-compressed stream into a new `Vec<u8>`.
///
/// `expected_len` is the expected decompressed size. It must match the size
/// encoded in the stream header.
#[cfg(feature = "alloc")]
pub fn decompress(src: &[u8], expected_len: usize) -> Result<alloc::vec::Vec<u8>, Error> {
    let actual_len_u64 = src
        .get(..HEADER_SIZE)
        .ok_or(Error::InputTooShort)
        .map(|header| u64::from_le_bytes(header.try_into().unwrap()))?;
    let actual_len = actual_len_u64.try_into().map_err(|_| Error::SizeOverflow {
        size: actual_len_u64,
    })?;
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
pub fn decompress_into(src: &[u8], dst: &mut [u8]) -> Result<usize, Error> {
    if src.len() < HEADER_SIZE {
        return Err(Error::InputTooShort);
    }

    let original_size_u64 = u64::from_le_bytes(src[..8].try_into().unwrap());
    let original_size: usize = original_size_u64
        .try_into()
        .map_err(|_| Error::SizeOverflow {
            size: original_size_u64,
        })?;

    if original_size == 0 {
        return Ok(0);
    }

    if dst.len() < original_size {
        return Err(Error::OutputTooSmall {
            need: original_size,
            have: dst.len(),
        });
    }

    if original_size <= SMALL_LIM {
        let payload = src
            .get(HEADER_SIZE..HEADER_SIZE + original_size)
            .ok_or(Error::InputTooShort)?;
        dst[..original_size].copy_from_slice(payload);
        return Ok(original_size);
    }

    if src.len() < EXT_HEADER_SIZE {
        return Err(Error::InputTooShort);
    }

    let suffix_cnt_u64 = u64::from_le_bytes(src[8..16].try_into().unwrap());
    let literal_suffix_cnt: usize = suffix_cnt_u64.try_into().map_err(|_| Error::SizeOverflow {
        size: suffix_cnt_u64,
    })?;

    if literal_suffix_cnt < LITERAL_SUFFIX || literal_suffix_cnt > original_size {
        return Err(Error::CorruptInput);
    }

    let suffix_end = EXT_HEADER_SIZE
        .checked_add(literal_suffix_cnt)
        .ok_or(Error::CorruptInput)?;
    if src.len() < suffix_end {
        return Err(Error::InputTooShort);
    }

    let suffix_start_in_src = src.len() - literal_suffix_cnt;
    let token_output_end = original_size - literal_suffix_cnt;

    #[cfg(not(feature = "paranoid"))]
    {
        #[cfg(feature = "std")]
        let level = fearless_simd::Level::new();
        #[cfg(not(feature = "std"))]
        let level = fearless_simd::Level::baseline();
        fearless_simd::dispatch!(level, simd => decompress_loop(
            simd, src, dst, original_size, literal_suffix_cnt,
            suffix_start_in_src, token_output_end,
        ))
    }
    #[cfg(feature = "paranoid")]
    decompress_loop(
        src,
        dst,
        original_size,
        literal_suffix_cnt,
        suffix_start_in_src,
        token_output_end,
    )
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn decompress_loop<S: Simd>(
    _simd: S,
    src: &[u8],
    dst: &mut [u8],
    original_size: usize,
    literal_suffix_cnt: usize,
    suffix_start_in_src: usize,
    token_output_end: usize,
) -> Result<usize, Error> {
    decompress_loop_impl(
        src,
        dst,
        original_size,
        literal_suffix_cnt,
        suffix_start_in_src,
        token_output_end,
    )
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn decompress_loop(
    src: &[u8],
    dst: &mut [u8],
    original_size: usize,
    literal_suffix_cnt: usize,
    suffix_start_in_src: usize,
    token_output_end: usize,
) -> Result<usize, Error> {
    decompress_loop_impl(
        src,
        dst,
        original_size,
        literal_suffix_cnt,
        suffix_start_in_src,
        token_output_end,
    )
}

#[inline(always)]
fn decompress_loop_impl(
    src: &[u8],
    dst: &mut [u8],
    original_size: usize,
    literal_suffix_cnt: usize,
    suffix_start_in_src: usize,
    token_output_end: usize,
) -> Result<usize, Error> {
    let mut control = EXT_HEADER_SIZE;
    let mut literals = suffix_start_in_src;
    let mut out = 0usize;

    while control
        .checked_add(3)
        .is_some_and(|control_end| control_end <= literals)
    {
        let token = paranoid_unsafe_call!(primitives::read_byte(src, control));
        let mut lit_len = (token >> TOKEN_MATCH_BITS) as usize;
        let match_len = (token & TOKEN_MATCH_MASK) as usize + MIN_MATCH_LEN - 1;

        let dis_small = paranoid_unsafe_call!(primitives::read_u16_le(src, control + 1)) as usize;
        let dis = dis_small + MIN_DISTANCE;

        control += 3;

        if lit_len == TOKEN_LIT_MAX {
            loop {
                if control >= literals {
                    return Err(Error::CorruptInput);
                }
                let extra = paranoid_unsafe_call!(primitives::read_byte(src, control)) as usize;
                control += 1;
                lit_len = lit_len.checked_add(extra).ok_or(Error::CorruptInput)?;
                if extra < 255 {
                    break;
                }
            }
        }

        if lit_len > literals {
            return Err(Error::CorruptInput);
        }
        literals -= lit_len;

        let token_len = lit_len.checked_add(match_len).ok_or(Error::CorruptInput)?;
        if out
            .checked_add(token_len)
            .is_none_or(|token_end| token_end > token_output_end)
        {
            return Err(Error::CorruptInput);
        }

        paranoid_unsafe_call!(primitives::wild_copy_literals_16(
            src, literals, dst, out, lit_len,
        ));
        if lit_len > 16 {
            paranoid_unsafe_call!(primitives::wild_copy_literals_16(
                src,
                literals + 16,
                dst,
                out + 16,
                lit_len.saturating_sub(16),
            ));
            if lit_len > 32 {
                paranoid_unsafe_call!(primitives::copy_from_src(
                    src,
                    literals + 32,
                    dst,
                    out + 32,
                    lit_len - 32,
                ));
            }
        }
        out += lit_len;

        if dis > out {
            return Err(Error::CorruptInput);
        }

        let match_src = out - dis;
        paranoid_unsafe_call!(primitives::wild_copy_match_32(
            dst, match_src, out, match_len,
        ));
        out += match_len;
    }

    if literal_suffix_cnt > 0 {
        if out + literal_suffix_cnt != original_size {
            return Err(Error::CorruptInput);
        }
        paranoid_unsafe_call!(primitives::copy_from_src(
            src,
            suffix_start_in_src,
            dst,
            out,
            literal_suffix_cnt,
        ));
        out += literal_suffix_cnt;
    }

    if out != original_size {
        return Err(Error::CorruptInput);
    }

    Ok(original_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "alloc")]
    use alloc::vec::Vec;

    #[test]
    fn empty_input() {
        let mut header = 0u64.to_le_bytes().to_vec();
        let result = decompress(&header, 0).unwrap();
        assert!(result.is_empty());

        header.truncate(4);
        assert!(decompress(&header, 0).is_err());
    }

    #[test]
    fn small_raw() {
        let data = b"hello world";
        let mut compressed = (data.len() as u64).to_le_bytes().to_vec();
        compressed.extend_from_slice(data);
        let result = decompress(&compressed, data.len()).unwrap();
        assert_eq!(&result, data);
    }

    #[test]
    fn small_raw_max() {
        let data = vec![0xAB; SMALL_LIM];
        let mut compressed = (data.len() as u64).to_le_bytes().to_vec();
        compressed.extend_from_slice(&data);
        let result = decompress(&compressed, data.len()).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn all_suffix_no_matches() {
        let data = vec![0x42; 100];
        let mut compressed = Vec::new();
        compressed.extend_from_slice(&(data.len() as u64).to_le_bytes());
        compressed.extend_from_slice(&(data.len() as u64).to_le_bytes());
        compressed.extend_from_slice(&data);
        let result = decompress(&compressed, data.len()).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn single_sequence() {
        let original_size: usize = 69;
        let suffix_cnt: usize = 32;

        let mut original = vec![0u8; original_size];
        for (i, byte) in original.iter_mut().enumerate().take(33) {
            *byte = (i as u8).wrapping_add(0x10);
        }
        original[33] = original[0];
        original[34] = original[1];
        original[35] = original[2];
        original[36] = original[3];

        let m_prime: u8 = (4 - (MIN_MATCH_LEN - 1)) as u8;
        let l_field: u8 = TOKEN_LIT_MAX as u8;
        let token = (l_field << TOKEN_MATCH_BITS) | m_prime;
        let dis_small: u16 = 0;

        let mut compressed = Vec::new();
        compressed.extend_from_slice(&(original_size as u64).to_le_bytes());
        compressed.extend_from_slice(&(suffix_cnt as u64).to_le_bytes());
        compressed.push(token);
        compressed.extend_from_slice(&dis_small.to_le_bytes());
        compressed.push(26);
        compressed.extend_from_slice(&original[..33]);
        compressed.extend_from_slice(&original[37..69]);

        let result = decompress(&compressed, original_size).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn output_too_small() {
        let data = b"hello";
        let mut compressed = (data.len() as u64).to_le_bytes().to_vec();
        compressed.extend_from_slice(data);
        let err = decompress_into(&compressed, &mut [0u8; 3]).unwrap_err();
        assert!(matches!(err, Error::OutputTooSmall { .. }));
    }

    #[test]
    fn truncated_header() {
        assert!(decompress(&[1, 0, 0], 1).is_err());
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn expected_size_mismatch_is_rejected() {
        let compressed = 3u64.to_le_bytes();
        assert_eq!(
            decompress(&compressed, 4),
            Err(Error::SizeMismatch {
                expected: 4,
                actual: 3,
            })
        );
    }
}
