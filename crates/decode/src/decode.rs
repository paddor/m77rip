#[cfg(feature = "alloc")]
use alloc::vec;

use crate::primitives;
use m77rip_core::Error;
use m77rip_core::format::*;

#[cold]
#[inline(never)]
fn corrupt_input() -> Result<usize, Error> {
    Err(Error::CorruptInput)
}

#[cfg(not(feature = "paranoid"))]
const RED_SLACK: usize = 8;
#[cfg(not(feature = "paranoid"))]
const HEAVY_DECODE_LITERAL_COPY: usize = 16;
#[cfg(not(feature = "paranoid"))]
const HEAVY_FAST_INLINE_LIT_MAX: usize = HEAVY_TOKEN_LIT_MAX - 1;
#[cfg(not(feature = "paranoid"))]
#[cfg(any(target_arch = "x86_64", kani))]
const HEAVY_FAST_SOURCE_GAP: usize = 4 + HEAVY_FAST_INLINE_LIT_MAX;
#[cfg(not(feature = "paranoid"))]
const HEAVY_FAST_OUTPUT_GAP: usize = HEAVY_FAST_INLINE_LIT_MAX + HEAVY_MAX_MATCH_LEN;
#[cfg(not(feature = "paranoid"))]
const MAX_INLINE_LIT_LEN: usize = TOKEN_LIT_MAX - 1;
#[cfg(not(feature = "paranoid"))]
const MAX_TOKEN_MATCH_LEN: usize = (TOKEN_MATCH_MASK as usize) + MIN_MATCH_LEN - 1;

#[cfg(not(feature = "paranoid"))]
const _: () = assert!(RED_SLACK >= MIN_MATCH_LEN);
#[cfg(not(feature = "paranoid"))]
const _: () = assert!(LITERAL_SUFFIX >= VECTOR_WIDTH);
#[cfg(not(feature = "paranoid"))]
const _: () = assert!(MAX_INLINE_LIT_LEN + VECTOR_WIDTH - (RED_SLACK + 1) <= LITERAL_SUFFIX);
#[cfg(not(feature = "paranoid"))]
const _: () = assert!(MAX_INLINE_LIT_LEN + MAX_TOKEN_MATCH_LEN - (RED_SLACK + 1) <= LITERAL_SUFFIX);
const _: () = assert!(HEAVY_MIN_DISTANCE > VECTOR_WIDTH);
const _: () = assert!(HEAVY_LITERAL_SUFFIX >= 2 * VECTOR_WIDTH);
#[cfg(not(feature = "paranoid"))]
const _: () = assert!(HEAVY_FAST_INLINE_LIT_MAX < HEAVY_TOKEN_LIT_MAX);
#[cfg(not(feature = "paranoid"))]
const _: () = assert!(HEAVY_FAST_OUTPUT_GAP <= HEAVY_MAX_DISTANCE);

#[cfg(not(feature = "paranoid"))]
#[allow(unused_imports)]
use fearless_simd::Level;
#[cfg(not(feature = "paranoid"))]
use fearless_simd::Simd;

#[cfg(not(feature = "paranoid"))]
#[inline(never)]
fn guarded_step_default(
    src: &[u8],
    dst: &mut [u8],
    control: &mut usize,
    literals: &mut usize,
    out: &mut usize,
    token_output_end: usize,
) -> Result<(), Error> {
    debug_assert!(*control < *literals);
    debug_assert!(*literals + LITERAL_SUFFIX <= src.len());

    let token = paranoid_unsafe_call!(primitives::read_byte(src, *control));
    let mut lit_len = (token >> TOKEN_MATCH_BITS) as usize;
    let match_len = (token & TOKEN_MATCH_MASK) as usize + MIN_MATCH_LEN - 1;
    let dis =
        paranoid_unsafe_call!(primitives::read_u16_le(src, *control + 1)) as usize + MIN_DISTANCE;
    *control += 3;

    if lit_len == TOKEN_LIT_MAX {
        loop {
            if *control >= *literals {
                return Err(Error::CorruptInput);
            }
            let extra = paranoid_unsafe_call!(primitives::read_byte(src, *control)) as usize;
            *control += 1;
            lit_len = lit_len.checked_add(extra).ok_or(Error::CorruptInput)?;
            if extra < 255 {
                break;
            }
        }
    }

    if lit_len > *literals {
        return Err(Error::CorruptInput);
    }
    *literals -= lit_len;
    let after_literals = (*out).checked_add(lit_len).ok_or(Error::CorruptInput)?;
    if dis > after_literals {
        return Err(Error::CorruptInput);
    }
    let token_end = after_literals
        .checked_add(match_len)
        .ok_or(Error::CorruptInput)?;
    if token_end > token_output_end {
        return Err(Error::CorruptInput);
    }

    paranoid_unsafe_call!(primitives::wild_copy_literals_16(
        src, *literals, dst, *out, lit_len,
    ));
    if lit_len > 16 {
        paranoid_unsafe_call!(primitives::wild_copy_literals_16(
            src,
            *literals + 16,
            dst,
            *out + 16,
            lit_len - 16,
        ));
        if lit_len > 32 {
            paranoid_unsafe_call!(primitives::copy_from_src(
                src,
                *literals + 32,
                dst,
                *out + 32,
                lit_len - 32,
            ));
        }
    }
    *out = after_literals;
    let match_src = *out - dis;
    paranoid_unsafe_call!(primitives::wild_copy_match_32(
        dst, match_src, *out, match_len,
    ));
    *out = token_end;
    Ok(())
}

#[inline]
fn header_fields(src: &[u8]) -> Result<(usize, u8), Error> {
    let header = src
        .get(..HEADER_SIZE)
        .ok_or(Error::InputTooShort)
        .map(|header| u64::from_le_bytes(header.try_into().unwrap()))?;
    let original_size_u64 = header & SIZE_MASK;
    let original_size = original_size_u64
        .try_into()
        .map_err(|_| Error::SizeOverflow {
            size: original_size_u64,
        })?;
    Ok((original_size, (header >> FLAG_SHIFT) as u8))
}

/// Reads the decompressed size from the first 8 bytes of a compressed stream.
///
/// Returns `None` if `src` is shorter than 8 bytes.
pub fn decompressed_size(src: &[u8]) -> Option<u64> {
    if src.len() < HEADER_SIZE {
        return None;
    }
    Some(u64::from_le_bytes(src[..8].try_into().unwrap()) & SIZE_MASK)
}

/// Decompresses a misa77-compressed stream into a new `Vec<u8>`.
///
/// `expected_len` is the expected decompressed size. It must match the size
/// encoded in the stream header.
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
pub fn decompress_into(src: &[u8], dst: &mut [u8]) -> Result<usize, Error> {
    let (original_size, flags) = header_fields(src)?;

    if original_size == 0 {
        return Ok(0);
    }

    if dst.len() < original_size {
        return Err(Error::OutputTooSmall {
            need: original_size,
            have: dst.len(),
        });
    }

    if flags & FLAG_HEAVY != 0 {
        return decompress_into_heavy(src, dst, original_size, flags);
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
        fearless_simd::dispatch!(level, simd => decompress_loop_fast(
            simd, src, dst, original_size, literal_suffix_cnt,
            suffix_start_in_src, token_output_end,
        ))
    }
    #[cfg(feature = "paranoid")]
    decompress_loop_impl(
        src,
        dst,
        original_size,
        literal_suffix_cnt,
        suffix_start_in_src,
        token_output_end,
    )
}

fn decompress_into_heavy(
    src: &[u8],
    dst: &mut [u8],
    original_size: usize,
    flags: u8,
) -> Result<usize, Error> {
    #[cfg(feature = "paranoid")]
    let _ = flags;

    if original_size <= HEAVY_SMALL_LIM {
        let raw_end = HEADER_SIZE
            .checked_add(original_size)
            .ok_or(Error::CorruptInput)?;
        let payload = src.get(HEADER_SIZE..raw_end).ok_or(Error::InputTooShort)?;
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

    if literal_suffix_cnt < HEAVY_LITERAL_SUFFIX || literal_suffix_cnt > original_size {
        return corrupt_input();
    }
    let Some(non_suffix_len) = src.len().checked_sub(literal_suffix_cnt) else {
        return corrupt_input();
    };
    if non_suffix_len < EXT_HEADER_SIZE {
        return corrupt_input();
    }

    let token_output_end = original_size - literal_suffix_cnt;
    #[cfg(not(feature = "paranoid"))]
    {
        #[cfg(all(feature = "std", target_arch = "x86_64"))]
        if std::arch::is_x86_feature_detected!("avx2") {
            // SAFETY: runtime feature check guarantees AVX2. The decoder loop
            // performs the same format bounds checks as the scalar fast path.
            return unsafe {
                if flags & FLAG_HEAVY_COND != 0 {
                    decompress_heavy_loop_fast_avx2::<true>(
                        src,
                        dst,
                        original_size,
                        literal_suffix_cnt,
                        non_suffix_len,
                        token_output_end,
                    )
                } else {
                    decompress_heavy_loop_fast_avx2::<false>(
                        src,
                        dst,
                        original_size,
                        literal_suffix_cnt,
                        non_suffix_len,
                        token_output_end,
                    )
                }
            };
        }

        if flags & FLAG_HEAVY_COND != 0 {
            decompress_heavy_loop_fast::<true>(
                src,
                dst,
                original_size,
                literal_suffix_cnt,
                non_suffix_len,
                token_output_end,
            )
        } else {
            decompress_heavy_loop_fast::<false>(
                src,
                dst,
                original_size,
                literal_suffix_cnt,
                non_suffix_len,
                token_output_end,
            )
        }
    }
    #[cfg(feature = "paranoid")]
    decompress_heavy_loop(
        src,
        dst,
        original_size,
        literal_suffix_cnt,
        non_suffix_len,
        token_output_end,
    )
}

#[cfg(feature = "paranoid")]
fn decompress_heavy_loop(
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

    while control < literals {
        if control.checked_add(4).is_none_or(|end| end > literals) {
            return corrupt_input();
        }
        let token = paranoid_unsafe_call!(primitives::read_u32_le(src, control));
        control += 4;

        let mut lit_len = (token >> 26) as usize;
        let match_code = ((token >> 20) & 0x3F) as usize;
        if match_code == 0 {
            return corrupt_input();
        }
        let match_len = HEAVY_LEN_OF[match_code] as usize;
        let dis = (token & HEAVY_DIS_MASK) as usize + HEAVY_MIN_DISTANCE;

        if lit_len == HEAVY_TOKEN_LIT_MAX {
            loop {
                if control >= literals {
                    return corrupt_input();
                }
                let extra = paranoid_unsafe_call!(primitives::read_byte(src, control)) as usize;
                control += 1;
                lit_len = lit_len.checked_add(extra).ok_or(Error::CorruptInput)?;
                if extra < 255 {
                    break;
                }
            }
        }

        if lit_len > literals - control {
            return corrupt_input();
        }
        literals -= lit_len;

        let after_literals = out.checked_add(lit_len).ok_or(Error::CorruptInput)?;
        let token_end = after_literals
            .checked_add(match_len)
            .ok_or(Error::CorruptInput)?;
        if token_end > token_output_end {
            return corrupt_input();
        }

        paranoid_unsafe_call!(primitives::copy_from_src(src, literals, dst, out, lit_len));
        out = after_literals;

        if dis > out {
            return corrupt_input();
        }
        copy_heavy_match(dst, out - dis, out, match_len)?;
        out = token_end;
    }

    if out != token_output_end {
        return corrupt_input();
    }
    paranoid_unsafe_call!(primitives::copy_from_src(
        src,
        suffix_start_in_src,
        dst,
        out,
        literal_suffix_cnt,
    ));
    Ok(original_size)
}

#[cfg(not(feature = "paranoid"))]
#[allow(clippy::too_many_arguments)]
fn decompress_heavy_loop_fast<const CONDITIONAL_MATCH_COPY: bool>(
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

    while control < literals {
        let token = paranoid_unsafe_call!(primitives::read_u32_le(src, control));
        control += 4;

        let mut lit_len = (token >> 26) as usize;
        let match_code = ((token >> 20) & 0x3F) as usize;
        if match_code == 0 {
            return corrupt_input();
        }
        let match_len = HEAVY_LEN_OF[match_code] as usize;
        let dis = (token & HEAVY_DIS_MASK) as usize + HEAVY_MIN_DISTANCE;

        if lit_len == HEAVY_TOKEN_LIT_MAX {
            loop {
                if control >= literals {
                    return corrupt_input();
                }
                let extra = paranoid_unsafe_call!(primitives::read_byte(src, control)) as usize;
                control += 1;
                lit_len = lit_len.checked_add(extra).ok_or(Error::CorruptInput)?;
                if extra < 255 {
                    break;
                }
            }
        }

        if control > literals || lit_len > literals - control {
            return corrupt_input();
        }
        literals -= lit_len;

        let after_literals = out.checked_add(lit_len).ok_or(Error::CorruptInput)?;
        let token_end = after_literals
            .checked_add(match_len)
            .ok_or(Error::CorruptInput)?;
        if token_end > token_output_end {
            return corrupt_input();
        }

        copy_heavy_literals_fast(src, literals, dst, out, lit_len);
        out = after_literals;

        if dis > out {
            return corrupt_input();
        }
        copy_heavy_match_fast::<CONDITIONAL_MATCH_COPY>(dst, out - dis, out, match_len);
        out = token_end;
    }

    if out != token_output_end {
        return corrupt_input();
    }
    paranoid_unsafe_call!(primitives::copy_from_src(
        src,
        suffix_start_in_src,
        dst,
        out,
        literal_suffix_cnt,
    ));
    Ok(original_size)
}

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
#[allow(clippy::too_many_arguments)]
unsafe fn decompress_heavy_loop_fast_avx2<const CONDITIONAL_MATCH_COPY: bool>(
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

    while control < literals {
        while heavy_fast_zone(control, literals, out, token_output_end) {
            let token_pos = control;
            let token = paranoid_unsafe_call!(primitives::read_u32_le(src, control));
            control += 4;

            let lit_len = (token >> 26) as usize;
            if lit_len == HEAVY_TOKEN_LIT_MAX {
                control = token_pos;
                break;
            }

            let match_code = ((token >> 20) & 0x3F) as usize;
            if match_code == 0 {
                return corrupt_input();
            }
            let match_len = HEAVY_LEN_OF[match_code] as usize;
            let dis = (token & HEAVY_DIS_MASK) as usize + HEAVY_MIN_DISTANCE;

            literals -= lit_len;
            copy_heavy_literals_fast_avx2(src, literals, dst, out, lit_len);

            let after_literals = out + lit_len;
            if dis > after_literals {
                return corrupt_input();
            }
            copy_heavy_match_fast_avx2::<CONDITIONAL_MATCH_COPY>(
                dst,
                after_literals - dis,
                after_literals,
                match_len,
            );
            out = after_literals + match_len;
        }

        if control < literals {
            checked_heavy_step_fast_avx2::<CONDITIONAL_MATCH_COPY>(
                src,
                dst,
                &mut control,
                &mut literals,
                &mut out,
                token_output_end,
            )?;
        }
    }

    if out != token_output_end {
        return corrupt_input();
    }
    paranoid_unsafe_call!(primitives::copy_from_src(
        src,
        suffix_start_in_src,
        dst,
        out,
        literal_suffix_cnt,
    ));
    Ok(original_size)
}

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[inline(always)]
fn heavy_fast_zone(control: usize, literals: usize, out: usize, token_output_end: usize) -> bool {
    debug_assert!(control <= literals);
    debug_assert!(out <= token_output_end);
    literals - control >= HEAVY_FAST_SOURCE_GAP && token_output_end - out >= HEAVY_FAST_OUTPUT_GAP
}

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn checked_heavy_step_fast_avx2<const CONDITIONAL_MATCH_COPY: bool>(
    src: &[u8],
    dst: &mut [u8],
    control: &mut usize,
    literals: &mut usize,
    out: &mut usize,
    token_output_end: usize,
) -> Result<(), Error> {
    let token = paranoid_unsafe_call!(primitives::read_u32_le(src, *control));
    *control += 4;

    let mut lit_len = (token >> 26) as usize;
    let match_code = ((token >> 20) & 0x3F) as usize;
    if match_code == 0 {
        return corrupt_input().map(|_| ());
    }
    let match_len = HEAVY_LEN_OF[match_code] as usize;
    let dis = (token & HEAVY_DIS_MASK) as usize + HEAVY_MIN_DISTANCE;

    if lit_len == HEAVY_TOKEN_LIT_MAX {
        loop {
            if *control >= *literals {
                return corrupt_input().map(|_| ());
            }
            let extra = paranoid_unsafe_call!(primitives::read_byte(src, *control)) as usize;
            *control += 1;
            lit_len = lit_len.checked_add(extra).ok_or(Error::CorruptInput)?;
            if extra < 255 {
                break;
            }
        }
    }

    if *control > *literals || lit_len > *literals - *control {
        return corrupt_input().map(|_| ());
    }
    *literals -= lit_len;

    let after_literals = out.checked_add(lit_len).ok_or(Error::CorruptInput)?;
    let token_end = after_literals
        .checked_add(match_len)
        .ok_or(Error::CorruptInput)?;
    if token_end > token_output_end {
        return corrupt_input().map(|_| ());
    }

    copy_heavy_literals_fast_avx2(src, *literals, dst, *out, lit_len);
    *out = after_literals;

    if dis > *out {
        return corrupt_input().map(|_| ());
    }
    copy_heavy_match_fast_avx2::<CONDITIONAL_MATCH_COPY>(dst, *out - dis, *out, match_len);
    *out = token_end;
    Ok(())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn copy_heavy_literals_fast(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: usize,
    lit_len: usize,
) {
    if lit_len > HEAVY_DECODE_LITERAL_COPY {
        paranoid_unsafe_call!(primitives::wild_copy_literals_32(
            src, src_pos, dst, dst_pos,
        ));
        if lit_len > VECTOR_WIDTH {
            paranoid_unsafe_call!(primitives::copy_from_src(
                src,
                src_pos + VECTOR_WIDTH,
                dst,
                dst_pos + VECTOR_WIDTH,
                lit_len - VECTOR_WIDTH,
            ));
        }
    } else {
        paranoid_unsafe_call!(primitives::wild_copy_literals_16(
            src, src_pos, dst, dst_pos, lit_len,
        ));
    }
}

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[inline(always)]
fn copy_heavy_literals_fast_avx2(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: usize,
    lit_len: usize,
) {
    if lit_len > HEAVY_DECODE_LITERAL_COPY {
        paranoid_unsafe_call!(primitives::avx2_copy_literals_32(
            src, src_pos, dst, dst_pos,
        ));
        if lit_len > VECTOR_WIDTH {
            paranoid_unsafe_call!(primitives::copy_from_src(
                src,
                src_pos + VECTOR_WIDTH,
                dst,
                dst_pos + VECTOR_WIDTH,
                lit_len - VECTOR_WIDTH,
            ));
        }
    } else {
        paranoid_unsafe_call!(primitives::wild_copy_literals_16(
            src, src_pos, dst, dst_pos, lit_len,
        ));
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn copy_heavy_match_fast<const CONDITIONAL_MATCH_COPY: bool>(
    dst: &mut [u8],
    match_src: usize,
    out: usize,
    match_len: usize,
) {
    paranoid_unsafe_call!(primitives::wild_copy_match_32(
        dst,
        match_src,
        out,
        VECTOR_WIDTH,
    ));

    if CONDITIONAL_MATCH_COPY && match_len <= VECTOR_WIDTH {
        return;
    }

    paranoid_unsafe_call!(primitives::wild_copy_match_32(
        dst,
        match_src + VECTOR_WIDTH,
        out + VECTOR_WIDTH,
        VECTOR_WIDTH,
    ));

    let mut copied = 2 * VECTOR_WIDTH;
    while copied < match_len {
        paranoid_unsafe_call!(primitives::wild_copy_match_32(
            dst,
            match_src + copied,
            out + copied,
            VECTOR_WIDTH,
        ));
        copied += VECTOR_WIDTH;
    }
}

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[inline(always)]
fn copy_heavy_match_fast_avx2<const CONDITIONAL_MATCH_COPY: bool>(
    dst: &mut [u8],
    match_src: usize,
    out: usize,
    match_len: usize,
) {
    paranoid_unsafe_call!(primitives::avx2_copy_match_32(dst, match_src, out));

    if CONDITIONAL_MATCH_COPY && match_len <= VECTOR_WIDTH {
        return;
    }

    paranoid_unsafe_call!(primitives::avx2_copy_match_32(
        dst,
        match_src + VECTOR_WIDTH,
        out + VECTOR_WIDTH,
    ));

    let mut copied = 2 * VECTOR_WIDTH;
    while copied < match_len {
        paranoid_unsafe_call!(primitives::avx2_copy_match_32(
            dst,
            match_src + copied,
            out + copied,
        ));
        copied += VECTOR_WIDTH;
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn copy_heavy_match(
    dst: &mut [u8],
    match_src: usize,
    out: usize,
    match_len: usize,
) -> Result<(), Error> {
    let rounded = match_len
        .checked_add(VECTOR_WIDTH - 1)
        .map(|n| n & !(VECTOR_WIDTH - 1))
        .ok_or(Error::CorruptInput)?;
    let end = out.checked_add(rounded).ok_or(Error::CorruptInput)?;
    if end > dst.len() {
        return Err(Error::CorruptInput);
    }

    let mut copied = 0usize;
    while copied < match_len {
        paranoid_unsafe_call!(primitives::wild_copy_match_32(
            dst,
            match_src + copied,
            out + copied,
            VECTOR_WIDTH,
        ));
        copied += VECTOR_WIDTH;
    }
    Ok(())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn decompress_loop_fast<S: Simd>(
    _simd: S,
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

    let prefix_end = MAX_DISTANCE.min(token_output_end);
    let red = token_output_end.saturating_sub(RED_SLACK);

    while control < literals && out < prefix_end {
        guarded_step_default(
            src,
            dst,
            &mut control,
            &mut literals,
            &mut out,
            token_output_end,
        )?;
    }

    while control < literals && out < red {
        debug_assert!(out >= MAX_DISTANCE);
        debug_assert!(literals + LITERAL_SUFFIX <= src.len());
        debug_assert!(out + MAX_INLINE_LIT_LEN + VECTOR_WIDTH <= original_size);
        debug_assert!(out + MAX_INLINE_LIT_LEN + MAX_TOKEN_MATCH_LEN <= original_size);

        let token = paranoid_unsafe_call!(primitives::read_byte(src, control));
        let mut lit_len = (token >> TOKEN_MATCH_BITS) as usize;
        let match_len = (token & TOKEN_MATCH_MASK) as usize + MIN_MATCH_LEN - 1;
        let dis = paranoid_unsafe_call!(primitives::read_u16_le(src, control + 1)) as usize
            + MIN_DISTANCE;
        control += 3;

        if lit_len == TOKEN_LIT_MAX {
            let extra = paranoid_unsafe_call!(primitives::read_byte(src, control)) as usize;
            control += 1;
            lit_len = lit_len.checked_add(extra).ok_or(Error::CorruptInput)?;

            if extra == 255 {
                loop {
                    if control >= literals {
                        return corrupt_input();
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
                return corrupt_input();
            }
            if lit_len - (RED_SLACK - MIN_MATCH_LEN) > red - out {
                return corrupt_input();
            }
        }

        literals -= lit_len;
        paranoid_unsafe_call!(primitives::wild_copy_literals_16(
            src, literals, dst, out, lit_len,
        ));
        if lit_len > 16 {
            paranoid_unsafe_call!(primitives::wild_copy_literals_16(
                src,
                literals + 16,
                dst,
                out + 16,
                lit_len - 16,
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

        let match_src = out - dis;
        paranoid_unsafe_call!(primitives::wild_copy_match_32(
            dst, match_src, out, match_len,
        ));
        out += match_len;
    }

    while control < literals {
        guarded_step_default(
            src,
            dst,
            &mut control,
            &mut literals,
            &mut out,
            token_output_end,
        )?;
    }

    if out != token_output_end {
        return corrupt_input();
    }
    paranoid_unsafe_call!(primitives::copy_from_src(
        src,
        suffix_start_in_src,
        dst,
        out,
        literal_suffix_cnt,
    ));
    Ok(original_size)
}

#[cfg(feature = "paranoid")]
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

    // Maximum representable displacement.
    const DIS_CEIL: usize = DIS_LIM + MIN_DISTANCE;
    // Slack before token_output_end where the fast loop must stop. Non-extension
    // tokens produce at most 6 literals + 34 match bytes = 40. Keep enough room
    // so that non-extension tokens cannot overrun.
    const RED_SLACK: usize = 8;

    let prefix_end = DIS_CEIL.min(token_output_end);
    let red = token_output_end.saturating_sub(RED_SLACK);

    // --- Phase 1: guarded prefix (match distance can underflow dst[0]) ---
    while control.checked_add(3).is_some_and(|end| end <= literals) && out < prefix_end {
        let token = paranoid_unsafe_call!(primitives::read_byte(src, control));
        let mut lit_len = (token >> TOKEN_MATCH_BITS) as usize;
        let match_len = (token & TOKEN_MATCH_MASK) as usize + MIN_MATCH_LEN - 1;
        let dis = paranoid_unsafe_call!(primitives::read_u16_le(src, control + 1)) as usize
            + MIN_DISTANCE;
        control += 3;

        if lit_len == TOKEN_LIT_MAX {
            loop {
                if control >= literals {
                    return corrupt_input();
                }
                let extra = paranoid_unsafe_call!(primitives::read_byte(src, control)) as usize;
                control = control.checked_add(1).ok_or(Error::CorruptInput)?;
                lit_len = lit_len.checked_add(extra).ok_or(Error::CorruptInput)?;
                if extra < 255 {
                    break;
                }
            }
        }

        if lit_len > literals {
            return corrupt_input();
        }
        literals -= lit_len;
        let after_literals = out.checked_add(lit_len).ok_or(Error::CorruptInput)?;
        let token_end = after_literals
            .checked_add(match_len)
            .ok_or(Error::CorruptInput)?;
        if token_end > token_output_end {
            return corrupt_input();
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
                lit_len - 16,
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
        out = after_literals;

        if dis > out {
            return corrupt_input();
        }
        let match_src = out - dis;
        paranoid_unsafe_call!(primitives::wild_copy_match_32(
            dst, match_src, out, match_len,
        ));
        out = token_end;
    }

    // --- Phase 2: fast main loop (dis can never underflow, non-extension
    // tokens can never overrun due to slack) ---
    while control.checked_add(3).is_some_and(|end| end <= literals) && out < red {
        let token = paranoid_unsafe_call!(primitives::read_byte(src, control));
        let mut lit_len = (token >> TOKEN_MATCH_BITS) as usize;
        let match_len = (token & TOKEN_MATCH_MASK) as usize + MIN_MATCH_LEN - 1;
        let dis = paranoid_unsafe_call!(primitives::read_u16_le(src, control + 1)) as usize
            + MIN_DISTANCE;
        control += 3;

        if lit_len == TOKEN_LIT_MAX {
            loop {
                if control >= literals {
                    return corrupt_input();
                }
                let extra = paranoid_unsafe_call!(primitives::read_byte(src, control)) as usize;
                control = control.checked_add(1).ok_or(Error::CorruptInput)?;
                lit_len = lit_len.checked_add(extra).ok_or(Error::CorruptInput)?;
                if extra < 255 {
                    break;
                }
            }
        }

        if lit_len > literals {
            return corrupt_input();
        }
        let after_literals = out.checked_add(lit_len).ok_or(Error::CorruptInput)?;
        let token_end = after_literals
            .checked_add(match_len)
            .ok_or(Error::CorruptInput)?;
        if token_end > token_output_end {
            return corrupt_input();
        }

        literals -= lit_len;

        #[cfg(feature = "paranoid")]
        {
            let literal_src = &src[literals..];
            let output = &mut dst[out..];
            primitives::wild_copy_literals_16_slices(literal_src, output);
            if lit_len > 16 {
                primitives::wild_copy_literals_16_slices(&literal_src[16..], &mut output[16..]);
                if lit_len > 32 {
                    output[32..lit_len].copy_from_slice(&literal_src[32..lit_len]);
                }
            }
        }
        #[cfg(not(feature = "paranoid"))]
        {
            paranoid_unsafe_call!(primitives::wild_copy_literals_16(
                src, literals, dst, out, lit_len,
            ));
            if lit_len > 16 {
                paranoid_unsafe_call!(primitives::wild_copy_literals_16(
                    src,
                    literals + 16,
                    dst,
                    out + 16,
                    lit_len - 16,
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
        }
        out = after_literals;

        let match_src = out - dis;
        #[cfg(feature = "paranoid")]
        {
            let (prefix, output) = dst.split_at_mut(out);
            primitives::wild_copy_match_32_slices(&prefix[match_src..], output);
        }
        #[cfg(not(feature = "paranoid"))]
        paranoid_unsafe_call!(primitives::wild_copy_match_32(
            dst, match_src, out, match_len,
        ));
        out = token_end;
    }

    // --- Phase 3: guarded tail ---
    while control.checked_add(3).is_some_and(|end| end <= literals) {
        let token = paranoid_unsafe_call!(primitives::read_byte(src, control));
        let mut lit_len = (token >> TOKEN_MATCH_BITS) as usize;
        let match_len = (token & TOKEN_MATCH_MASK) as usize + MIN_MATCH_LEN - 1;
        let dis = paranoid_unsafe_call!(primitives::read_u16_le(src, control + 1)) as usize
            + MIN_DISTANCE;
        control += 3;

        if lit_len == TOKEN_LIT_MAX {
            loop {
                if control >= literals {
                    return corrupt_input();
                }
                let extra = paranoid_unsafe_call!(primitives::read_byte(src, control)) as usize;
                control = control.checked_add(1).ok_or(Error::CorruptInput)?;
                lit_len = lit_len.checked_add(extra).ok_or(Error::CorruptInput)?;
                if extra < 255 {
                    break;
                }
            }
        }

        if lit_len > literals {
            return corrupt_input();
        }
        literals -= lit_len;
        let after_literals = out.checked_add(lit_len).ok_or(Error::CorruptInput)?;
        let token_end = after_literals
            .checked_add(match_len)
            .ok_or(Error::CorruptInput)?;
        if token_end > token_output_end {
            return corrupt_input();
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
                lit_len - 16,
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
        out = after_literals;

        if dis > out {
            return corrupt_input();
        }
        let match_src = out - dis;
        paranoid_unsafe_call!(primitives::wild_copy_match_32(
            dst, match_src, out, match_len,
        ));
        out = token_end;
    }

    if literal_suffix_cnt > 0 {
        if out
            .checked_add(literal_suffix_cnt)
            .is_none_or(|end| end != original_size)
        {
            return corrupt_input();
        }
        paranoid_unsafe_call!(primitives::copy_from_src(
            src,
            suffix_start_in_src,
            dst,
            out,
            literal_suffix_cnt,
        ));
        out = original_size;
    }

    if out != original_size {
        return corrupt_input();
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

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    /// Fixed-width primitive preconditions match their documented bounds.
    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn fixed_width_operations_are_in_bounds() {
        let src = [0u8; 96];
        let mut dst = [0u8; 96];
        let src_pos: usize = kani::any();
        let dst_pos: usize = kani::any();
        kani::assume(src_pos <= 80);
        kani::assume(dst_pos <= 64);

        unsafe {
            primitives::wild_copy_literals_16(&src, src_pos, &mut dst, dst_pos, 1);
        }

        let match_src = dst_pos.saturating_sub(33);
        kani::assume(dst_pos >= 33);
        unsafe {
            primitives::wild_copy_match_32(&mut dst, match_src, dst_pos, 1);
        }
    }

    /// `control < literals` is enough for fixed-width token reads in the fast
    /// phase because the format requires a literal suffix that also acts as
    /// source overread padding.
    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn fast_phase_header_and_first_extension_reads_are_in_bounds() {
        let src_len: usize = kani::any();
        let literals: usize = kani::any();
        let control: usize = kani::any();

        let suffix_end = literals.checked_add(LITERAL_SUFFIX);
        kani::assume(suffix_end.is_some());
        kani::assume(suffix_end.unwrap() <= src_len);
        kani::assume(control < literals);

        let dis_hi = control.checked_add(2).unwrap();
        let first_extension = control.checked_add(3).unwrap();
        assert!(dis_hi < src_len);
        assert!(first_extension < src_len);
    }

    /// Fast middle-phase non-extension tokens stay inside the output buffer
    /// because `red` leaves enough suffix slack for wild copies.
    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn fast_phase_non_extension_geometry_is_in_bounds() {
        let token_output_end: usize = kani::any();
        kani::assume(token_output_end <= usize::MAX - LITERAL_SUFFIX);
        let original_size = token_output_end + LITERAL_SUFFIX;
        let red = token_output_end.saturating_sub(RED_SLACK);

        let out: usize = kani::any();
        let lit_len: usize = kani::any();
        let match_len: usize = kani::any();
        let dis: usize = kani::any();

        kani::assume(out >= MAX_DISTANCE);
        kani::assume(out < red);
        kani::assume(lit_len <= MAX_INLINE_LIT_LEN);
        kani::assume(match_len <= MAX_TOKEN_MATCH_LEN);
        kani::assume(dis >= MIN_DISTANCE);
        kani::assume(dis <= MAX_DISTANCE);

        let after_literals = out.checked_add(lit_len).unwrap();
        assert!(after_literals >= dis);
        assert!(out.checked_add(16).unwrap() <= original_size);
        assert!(after_literals.checked_add(VECTOR_WIDTH).unwrap() <= original_size);

        let match_src = after_literals - dis;
        assert!(match_src.checked_add(VECTOR_WIDTH).unwrap() <= after_literals);
    }

    /// Fast middle-phase extension tokens are guarded before any unchecked
    /// literal or match copy can run past the destination buffer.
    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn fast_phase_extension_geometry_is_in_bounds() {
        let token_output_end: usize = kani::any();
        kani::assume(token_output_end <= usize::MAX - LITERAL_SUFFIX);
        let original_size = token_output_end + LITERAL_SUFFIX;
        let red = token_output_end.saturating_sub(RED_SLACK);

        let out: usize = kani::any();
        let lit_len: usize = kani::any();
        let match_len: usize = kani::any();
        let dis: usize = kani::any();

        kani::assume(out >= MAX_DISTANCE);
        kani::assume(out < red);
        kani::assume(lit_len >= TOKEN_LIT_MAX);
        kani::assume(match_len <= MAX_TOKEN_MATCH_LEN);
        kani::assume(dis >= MIN_DISTANCE);
        kani::assume(dis <= MAX_DISTANCE);
        kani::assume(lit_len - (RED_SLACK - MIN_MATCH_LEN) <= red - out);

        let after_literals = out.checked_add(lit_len).unwrap();
        assert!(after_literals >= dis);
        assert!(out.checked_add(16).unwrap() <= original_size);
        assert!(after_literals.checked_add(VECTOR_WIDTH).unwrap() <= original_size);

        let token_end = after_literals.checked_add(match_len).unwrap();
        assert!(token_end <= original_size);

        let match_src = after_literals - dis;
        assert!(match_src.checked_add(VECTOR_WIDTH).unwrap() <= after_literals);
    }

    /// Malformed input must return an error before unchecked copy preconditions
    /// can be violated.
    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn malformed_match_is_rejected_before_unsafe_copy() {
        let src = [0u8; 51];
        let mut dst = [0u8; 100];
        let mut control = EXT_HEADER_SIZE;
        let mut literals = EXT_HEADER_SIZE + 3;
        let mut out = 0usize;
        let result =
            guarded_step_default(&src, &mut dst, &mut control, &mut literals, &mut out, 68);
        assert!(result.is_err());
    }

    #[kani::proof]
    fn heavy_header_masks_flags() {
        let size: u64 = kani::any();
        let flags: u8 = kani::any();
        kani::assume(size <= SIZE_MASK);

        let header = size | ((flags as u64) << FLAG_SHIFT);
        assert_eq!(header & SIZE_MASK, size);
        assert_eq!((header >> FLAG_SHIFT) as u8, flags);
    }

    #[kani::proof]
    fn heavy_token_fields_are_bounded() {
        let token: u32 = kani::any();

        let lit_len = (token >> 26) as usize;
        let match_code = ((token >> 20) & 0x3F) as usize;
        let dis = (token & HEAVY_DIS_MASK) as usize + HEAVY_MIN_DISTANCE;

        assert!(lit_len <= HEAVY_TOKEN_LIT_MAX);
        assert!(match_code < HEAVY_LEN_OF.len());
        assert!(dis >= HEAVY_MIN_DISTANCE);
        assert!(dis <= HEAVY_MAX_DISTANCE);
    }

    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn heavy_token_read_can_use_suffix_slack() {
        let non_suffix_len: usize = kani::any();
        let literal_suffix_cnt: usize = kani::any();
        kani::assume(literal_suffix_cnt >= HEAVY_LITERAL_SUFFIX);
        kani::assume(non_suffix_len <= usize::MAX - literal_suffix_cnt);
        let src_len = non_suffix_len + literal_suffix_cnt;

        let literals: usize = kani::any();
        let control: usize = kani::any();
        kani::assume(literals <= non_suffix_len);
        kani::assume(control < literals);

        let read_end = control.checked_add(4);
        assert!(read_end.is_some());
        assert!(read_end.unwrap() <= src_len);
    }

    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn heavy_literal_copy_geometry_is_in_bounds() {
        let src_len: usize = kani::any();
        kani::assume(src_len >= HEAVY_LITERAL_SUFFIX);
        let suffix_start = src_len - HEAVY_LITERAL_SUFFIX;

        let token_output_end: usize = kani::any();
        kani::assume(token_output_end <= usize::MAX - HEAVY_LITERAL_SUFFIX);
        let original_size = token_output_end + HEAVY_LITERAL_SUFFIX;

        let old_literals: usize = kani::any();
        let control: usize = kani::any();
        let lit_len: usize = kani::any();
        let match_len: usize = kani::any();
        let out: usize = kani::any();

        kani::assume(old_literals <= suffix_start);
        kani::assume(control <= old_literals);
        kani::assume(lit_len > HEAVY_DECODE_LITERAL_COPY);
        kani::assume(lit_len <= old_literals - control);
        kani::assume(match_len >= MIN_MATCH_LEN);
        kani::assume(match_len <= HEAVY_MAX_MATCH_LEN);

        let after_literals = out.checked_add(lit_len);
        kani::assume(after_literals.is_some());
        let token_end = after_literals.unwrap().checked_add(match_len);
        kani::assume(token_end.is_some());
        kani::assume(token_end.unwrap() <= token_output_end);

        let src_pos = old_literals - lit_len;
        assert!(src_pos.checked_add(VECTOR_WIDTH).unwrap() <= src_len);
        assert!(out.checked_add(VECTOR_WIDTH).unwrap() <= original_size);
    }

    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn heavy_fast_non_extension_geometry_is_in_bounds() {
        let src_len: usize = kani::any();
        kani::assume(src_len >= HEAVY_LITERAL_SUFFIX);
        let suffix_start = src_len - HEAVY_LITERAL_SUFFIX;

        let token_output_end: usize = kani::any();
        kani::assume(token_output_end <= usize::MAX - HEAVY_LITERAL_SUFFIX);
        let original_size = token_output_end + HEAVY_LITERAL_SUFFIX;

        let control: usize = kani::any();
        let literals: usize = kani::any();
        let out: usize = kani::any();
        let lit_len: usize = kani::any();
        let match_len: usize = kani::any();
        let dis: usize = kani::any();

        kani::assume(control <= literals);
        kani::assume(literals <= suffix_start);
        kani::assume(literals - control >= HEAVY_FAST_SOURCE_GAP);
        kani::assume(out <= token_output_end);
        kani::assume(token_output_end - out >= HEAVY_FAST_OUTPUT_GAP);
        kani::assume(lit_len <= HEAVY_FAST_INLINE_LIT_MAX);
        kani::assume(match_len >= MIN_MATCH_LEN);
        kani::assume(match_len <= HEAVY_MAX_MATCH_LEN);
        kani::assume(dis >= HEAVY_MIN_DISTANCE);
        kani::assume(dis <= HEAVY_MAX_DISTANCE);

        let control_after = control.checked_add(4);
        assert!(control_after.is_some());
        assert!(control_after.unwrap() <= src_len);
        assert!(lit_len <= literals - control_after.unwrap());

        let literal_src = literals - lit_len;
        assert!(literal_src.checked_add(VECTOR_WIDTH).unwrap() <= src_len);
        assert!(out.checked_add(VECTOR_WIDTH).unwrap() <= original_size);

        let after_literals = out.checked_add(lit_len);
        assert!(after_literals.is_some());
        kani::assume(dis <= after_literals.unwrap());
        assert!(after_literals.unwrap() >= dis);
        let token_end = after_literals.unwrap().checked_add(match_len);
        assert!(token_end.is_some());
        assert!(token_end.unwrap() <= token_output_end);

        let rounded = (match_len + (VECTOR_WIDTH - 1)) & !(VECTOR_WIDTH - 1);
        let copy_end = after_literals.unwrap().checked_add(rounded);
        assert!(copy_end.is_some());
        assert!(copy_end.unwrap() <= original_size);

        let match_src = after_literals.unwrap() - dis;
        assert!(match_src.checked_add(VECTOR_WIDTH).unwrap() <= after_literals.unwrap());
    }

    #[kani::proof]
    #[cfg(not(feature = "paranoid"))]
    fn heavy_match_copy_geometry_is_in_bounds() {
        let token_output_end: usize = kani::any();
        kani::assume(token_output_end <= usize::MAX - HEAVY_LITERAL_SUFFIX);
        let original_size = token_output_end + HEAVY_LITERAL_SUFFIX;

        let out: usize = kani::any();
        let match_len: usize = kani::any();
        let dis: usize = kani::any();

        kani::assume(match_len >= MIN_MATCH_LEN);
        kani::assume(match_len <= HEAVY_MAX_MATCH_LEN);
        kani::assume(dis >= HEAVY_MIN_DISTANCE);
        kani::assume(dis <= HEAVY_MAX_DISTANCE);
        kani::assume(out >= dis);
        let token_end = out.checked_add(match_len);
        kani::assume(token_end.is_some());
        kani::assume(token_end.unwrap() <= token_output_end);

        let rounded = (match_len + (VECTOR_WIDTH - 1)) & !(VECTOR_WIDTH - 1);
        assert!(rounded >= VECTOR_WIDTH);
        assert!(rounded <= match_len + (VECTOR_WIDTH - 1));
        let copy_end = out.checked_add(rounded);
        assert!(copy_end.is_some());
        assert!(copy_end.unwrap() <= original_size);

        let match_src = out - dis;
        let copied: usize = kani::any();
        kani::assume(copied <= rounded - VECTOR_WIDTH);

        let src_pos = match_src.checked_add(copied);
        let dst_pos = out.checked_add(copied);
        assert!(src_pos.is_some());
        assert!(dst_pos.is_some());
        assert!(src_pos.unwrap().checked_add(VECTOR_WIDTH).unwrap() <= dst_pos.unwrap());
        assert!(dst_pos.unwrap().checked_add(VECTOR_WIDTH).unwrap() <= original_size);
    }
}
