use m77rip::{Error, decompress, decompress_into, decompressed_size};
use m77rip_core::format::{
    EXT_HEADER_SIZE, FLAG_HEAVY, FLAG_HEAVY_COND, FLAG_SHIFT, HEAVY_LITERAL_SUFFIX, MAX_DISTANCE,
    MIN_MATCH_LEN, TOKEN_LIT_MAX, TOKEN_MATCH_BITS,
};

fn push_literal_extension(out: &mut Vec<u8>, literal_len: usize) {
    let mut remaining = literal_len - TOKEN_LIT_MAX;
    while remaining >= 255 {
        out.push(255);
        remaining -= 255;
    }
    out.push(remaining as u8);
}

fn heavy_header(size: usize, flags: u8) -> [u8; 8] {
    ((size as u64) | ((flags as u64) << FLAG_SHIFT)).to_le_bytes()
}

fn heavy_token(lit_len: usize, match_code: usize, distance_delta: u32) -> [u8; 4] {
    let token = ((lit_len as u32) << 26) | ((match_code as u32) << 20) | distance_delta;
    token.to_le_bytes()
}

#[test]
fn decompressed_size_valid() {
    let mut data = vec![0u8; 16];
    data[..8].copy_from_slice(&42u64.to_le_bytes());
    assert_eq!(decompressed_size(&data), Some(42));

    data[..8].copy_from_slice(&heavy_header(42, FLAG_HEAVY | FLAG_HEAVY_COND));
    assert_eq!(decompressed_size(&data), Some(42));
}

#[test]
fn decompressed_size_too_short() {
    assert_eq!(decompressed_size(&[1, 2, 3]), None);
}

#[test]
fn decompressed_size_zero() {
    let data = 0u64.to_le_bytes();
    assert_eq!(decompressed_size(&data), Some(0));
}

#[test]
fn decompress_empty() {
    let header = 0u64.to_le_bytes();
    let result = decompress(&header, 0).unwrap();
    assert!(result.is_empty());
}

#[test]
fn decompress_input_too_short() {
    let err = decompress(&[1, 0, 0], 1).unwrap_err();
    assert_eq!(err, Error::InputTooShort);
}

#[test]
fn decompress_output_too_small() {
    let mut compressed = (10u64).to_le_bytes().to_vec();
    compressed.extend_from_slice(&[0u8; 10]);
    let mut dst = [0u8; 5];
    let err = decompress_into(&compressed, &mut dst).unwrap_err();
    assert_eq!(err, Error::OutputTooSmall { need: 10, have: 5 });
}

#[test]
fn decompress_small_raw() {
    let data = b"hello world!";
    let mut compressed = (data.len() as u64).to_le_bytes().to_vec();
    compressed.extend_from_slice(data);
    let result = decompress(&compressed, data.len()).unwrap();
    assert_eq!(&result[..], data);
}

#[test]
fn decompress_small_boundary() {
    let data = vec![0xFF; 32];
    let mut compressed = (data.len() as u64).to_le_bytes().to_vec();
    compressed.extend_from_slice(&data);
    let result = decompress(&compressed, data.len()).unwrap();
    assert_eq!(result, data);
}

#[test]
fn decompress_heavy_small_raw() {
    let data = vec![0xA5; 64];
    let mut compressed = heavy_header(data.len(), FLAG_HEAVY).to_vec();
    compressed.extend_from_slice(&data);
    let result = decompress(&compressed, data.len()).unwrap();
    assert_eq!(result, data);
}

#[test]
fn decompress_all_suffix() {
    let data = vec![0x42; 100];
    let mut compressed = Vec::new();
    compressed.extend_from_slice(&(data.len() as u64).to_le_bytes());
    compressed.extend_from_slice(&(data.len() as u64).to_le_bytes());
    compressed.extend_from_slice(&data);
    let result = decompress(&compressed, data.len()).unwrap();
    assert_eq!(result, data);
}

#[test]
fn decompress_corrupt_suffix_cnt_too_large() {
    let mut compressed = Vec::new();
    compressed.extend_from_slice(&(50u64).to_le_bytes());
    compressed.extend_from_slice(&(100u64).to_le_bytes());
    compressed.extend_from_slice(&[0u8; 50]);
    let err = decompress(&compressed, 50).unwrap_err();
    assert_eq!(err, Error::CorruptInput);
}

#[test]
fn decompress_rejects_match_before_output() {
    let mut compressed = Vec::new();
    compressed.extend_from_slice(&(100u64).to_le_bytes());
    compressed.extend_from_slice(&(32u64).to_le_bytes());
    compressed.extend_from_slice(&[7 << 5, 0, 0]);
    compressed.extend_from_slice(&[0u8; 32]);

    let err = decompress(&compressed, 100).unwrap_err();
    assert_eq!(err, Error::CorruptInput);
}

#[test]
fn decompress_rejects_fast_phase_literal_overrun() {
    let suffix_cnt = 32usize;
    let first_literals = MAX_DISTANCE;
    let bad_literals = 1000usize;
    let token_output_end = first_literals + MIN_MATCH_LEN - 1 + 29;
    let original_size = token_output_end + suffix_cnt;

    let mut compressed = Vec::new();
    compressed.extend_from_slice(&(original_size as u64).to_le_bytes());
    compressed.extend_from_slice(&(suffix_cnt as u64).to_le_bytes());

    compressed.push((TOKEN_LIT_MAX as u8) << TOKEN_MATCH_BITS);
    compressed.extend_from_slice(&0u16.to_le_bytes());
    push_literal_extension(&mut compressed, first_literals);

    compressed.push((TOKEN_LIT_MAX as u8) << TOKEN_MATCH_BITS);
    compressed.extend_from_slice(&0u16.to_le_bytes());
    push_literal_extension(&mut compressed, bad_literals);

    compressed.extend(std::iter::repeat_n(0x11, bad_literals));
    compressed.extend(std::iter::repeat_n(0x22, first_literals));
    compressed.extend(std::iter::repeat_n(0x33, suffix_cnt));

    assert!(compressed.len() > EXT_HEADER_SIZE + suffix_cnt);
    let err = decompress(&compressed, original_size).unwrap_err();
    assert_eq!(err, Error::CorruptInput);
}

#[test]
fn decompress_handcrafted_one_sequence() {
    // One sequence: 33 literal bytes, match_len=4, distance=33.
    // output[0..33] = literals, output[33..37] = copy of output[0..4].
    // Suffix: output[37..69] = 32 bytes.
    let mut original = vec![0u8; 69];
    for (i, byte) in original.iter_mut().enumerate().take(33) {
        *byte = (i as u8).wrapping_add(0x10);
    }
    original[33] = original[0];
    original[34] = original[1];
    original[35] = original[2];
    original[36] = original[3];

    let m_prime: u8 = 1;
    let l_field: u8 = 7;
    let token: u8 = (l_field << 5) | m_prime;
    let dis_small: u16 = 0;

    let mut compressed = Vec::new();
    compressed.extend_from_slice(&(69u64).to_le_bytes());
    compressed.extend_from_slice(&(32u64).to_le_bytes());
    compressed.push(token);
    compressed.extend_from_slice(&dis_small.to_le_bytes());
    compressed.push(26); // extension: 33 - 7
    compressed.extend_from_slice(&original[..33]);
    compressed.extend_from_slice(&original[37..69]);

    let result = decompress(&compressed, 69).unwrap();
    assert_eq!(result, original);
}

#[test]
fn decompress_heavy_handcrafted_one_sequence() {
    let literal_len = 33usize;
    let match_len = 4usize;
    let original_size = literal_len + match_len + HEAVY_LITERAL_SUFFIX;

    let mut original = vec![0u8; original_size];
    for (i, byte) in original.iter_mut().enumerate().take(literal_len) {
        *byte = (i as u8).wrapping_add(0x20);
    }
    original[literal_len..literal_len + match_len].copy_from_slice(&[0x20, 0x21, 0x22, 0x23]);
    for (i, byte) in original[literal_len + match_len..].iter_mut().enumerate() {
        *byte = (i as u8).wrapping_mul(3).wrapping_add(7);
    }

    let mut compressed = Vec::new();
    compressed.extend_from_slice(&heavy_header(original_size, FLAG_HEAVY));
    compressed.extend_from_slice(&(HEAVY_LITERAL_SUFFIX as u64).to_le_bytes());
    compressed.extend_from_slice(&heavy_token(literal_len, 1, 0));
    compressed.extend_from_slice(&original[..literal_len]);
    compressed.extend_from_slice(&original[literal_len + match_len..]);

    let result = decompress(&compressed, original_size).unwrap();
    assert_eq!(result, original);
}

#[test]
fn decompress_heavy_rejects_short_suffix() {
    let original_size = 100usize;
    let mut compressed = Vec::new();
    compressed.extend_from_slice(&heavy_header(original_size, FLAG_HEAVY));
    compressed.extend_from_slice(&32u64.to_le_bytes());
    compressed.extend(std::iter::repeat_n(0u8, 32));

    let err = decompress(&compressed, original_size).unwrap_err();
    assert_eq!(err, Error::CorruptInput);
}

#[test]
fn decompress_heavy_rejects_zero_match_code() {
    let original_size = 33 + 4 + HEAVY_LITERAL_SUFFIX;
    let mut compressed = Vec::new();
    compressed.extend_from_slice(&heavy_header(original_size, FLAG_HEAVY));
    compressed.extend_from_slice(&(HEAVY_LITERAL_SUFFIX as u64).to_le_bytes());
    compressed.extend_from_slice(&heavy_token(33, 0, 0));
    compressed.extend(std::iter::repeat_n(0x11, 33));
    compressed.extend(std::iter::repeat_n(0x22, HEAVY_LITERAL_SUFFIX));

    let err = decompress(&compressed, original_size).unwrap_err();
    assert_eq!(err, Error::CorruptInput);
}

#[test]
fn decompress_heavy_rejects_match_before_output() {
    let original_size = 1 + 4 + HEAVY_LITERAL_SUFFIX;
    let mut compressed = Vec::new();
    compressed.extend_from_slice(&heavy_header(original_size, FLAG_HEAVY));
    compressed.extend_from_slice(&(HEAVY_LITERAL_SUFFIX as u64).to_le_bytes());
    compressed.extend_from_slice(&heavy_token(1, 1, 0));
    compressed.push(0x11);
    compressed.extend(std::iter::repeat_n(0x22, HEAVY_LITERAL_SUFFIX));

    let err = decompress(&compressed, original_size).unwrap_err();
    assert_eq!(err, Error::CorruptInput);
}

#[test]
fn decompress_heavy_rejects_literal_stream_overrun() {
    let original_size = 40 + 4 + HEAVY_LITERAL_SUFFIX;
    let mut compressed = Vec::new();
    compressed.extend_from_slice(&heavy_header(original_size, FLAG_HEAVY));
    compressed.extend_from_slice(&(HEAVY_LITERAL_SUFFIX as u64).to_le_bytes());
    compressed.extend_from_slice(&heavy_token(40, 1, 0));
    compressed.extend(std::iter::repeat_n(0x22, HEAVY_LITERAL_SUFFIX));

    let err = decompress(&compressed, original_size).unwrap_err();
    assert_eq!(err, Error::CorruptInput);
}

#[test]
fn decompress_handcrafted_with_literals() {
    // Original: 50 bytes
    // Sequence: 5 literals + match_len=4, distance=33
    //   lit_len=5: stored inline (< 7), no extension
    //   The 5 literal bytes go into literal stream
    //   Match copies from out[5+5-33] ... hmm, let me think.
    //
    // Build it step by step:
    //   suffix = original[11..50] = 39 bytes
    //   Decode produces:
    //     1. 5 literal bytes -> out[0..5]
    //     2. match from out[5-33] -> invalid (5 < 33)
    //
    // Need more data before the match. Let's make it bigger.
    //
    // Original: 80 bytes
    // suffix = original[41..80] = 39 bytes
    // Sequence: 37 literals (original[0..37]), match_len=4, distance=33
    //   output[0..37] = literals
    //   output[37..41] = copy of output[37-33..37-33+4] = output[4..8]
    let mut original = vec![0u8; 80];
    for (i, byte) in original.iter_mut().enumerate().take(37) {
        *byte = (i as u8).wrapping_add(0x10);
    }
    // output[37..41] = output[4..8]
    original[37] = original[4];
    original[38] = original[5];
    original[39] = original[6];
    original[40] = original[7];

    let m_prime: u8 = 1; // match_len = 4
    let l_field: u8 = 7; // lit_len >= 7, so max inline
    let token: u8 = (l_field << 5) | m_prime;
    let dis_small: u16 = 0;

    let mut compressed = Vec::new();
    compressed.extend_from_slice(&(80u64).to_le_bytes());
    compressed.extend_from_slice(&(39u64).to_le_bytes());

    // control stream
    compressed.push(token);
    compressed.extend_from_slice(&dis_small.to_le_bytes());
    // extension bytes: 37 - 7 = 30
    compressed.push(30);

    // literal stream (reversed block order, but only one block)
    compressed.extend_from_slice(&original[..37]);

    // suffix
    compressed.extend_from_slice(&original[41..80]);

    let result = decompress(&compressed, 80).unwrap();
    assert_eq!(result, original);
}
