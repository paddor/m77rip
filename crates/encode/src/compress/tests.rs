use super::*;
use m77rip_core::format::HEAVY_SMALL_LIM;

#[test]
fn compress_empty() {
    let compressed = compress(b"");
    assert_eq!(&compressed[..8], &0u64.to_le_bytes());
    assert_eq!(compressed.len(), 8);
}

#[test]
fn compress_small() {
    let data = b"hello";
    let compressed = compress(data);
    assert_eq!(&compressed[..8], &(data.len() as u64).to_le_bytes());
    assert_eq!(&compressed[8..], data);
}

#[test]
fn compress_small_max() {
    let data = vec![0xAB; SMALL_LIM];
    let compressed = compress(&data);
    assert_eq!(&compressed[..8], &(data.len() as u64).to_le_bytes());
    assert_eq!(&compressed[8..], &data[..]);
}

#[test]
fn compress_header_valid() {
    let data = vec![0x42; 200];
    let compressed = compress(&data);
    let size = u64::from_le_bytes(compressed[..8].try_into().unwrap()) as usize;
    assert_eq!(size, 200);
}

#[test]
fn compress_bound_covers() {
    let data = vec![0x42; 200];
    let compressed = compress(&data);
    assert!(compressed.len() <= compress_bound(data.len()));
}

#[test]
fn compress_bound_covers_level0() {
    let data = vec![0x42; 200];
    let compressed = compress_level(&data, 0).unwrap();
    assert!(compressed.len() <= compress_bound(data.len()));
}

#[test]
fn compress_bound_covers_level4() {
    let data = vec![0x42; HEAVY_SMALL_LIM];
    let compressed = compress_level(&data, 4).unwrap();
    assert!(compressed.len() <= compress_bound(data.len()));
    assert!(compressed.len() <= compress_bound_level(data.len(), 4).unwrap());
}

#[test]
fn invalid_level_is_rejected() {
    assert_eq!(
        compress_level(b"input", -2),
        Err(Error::InvalidLevel { level: -2 })
    );
    assert_eq!(
        compress_into_level(b"input", &mut [0; 64], 5),
        Err(Error::InvalidLevel { level: 5 })
    );
}
