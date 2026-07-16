use m77rip::{Error, decompress, decompress_into, decompressed_size};

#[test]
fn decompressed_size_valid() {
    let mut data = vec![0u8; 16];
    data[..8].copy_from_slice(&42u64.to_le_bytes());
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
