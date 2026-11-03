use super::*;
use m77rip_core::format::{MIN_MATCH_LEN, TOKEN_LIT_MAX, TOKEN_MATCH_BITS};

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[test]
fn empty_input() {
    let mut header = 0u64.to_le_bytes().to_vec();
    let result = decompress(&header, 0).unwrap();
    assert_eq!(result, [] as [u8; 0]);

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
