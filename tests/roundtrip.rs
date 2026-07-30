mod common;

use common::*;
use std::iter;

#[cfg(feature = "c-reference")]
unsafe extern "C" {
    fn misa77_compress_bound(src_size: u64, level: i8) -> u64;
    fn misa77_compress(src: *const u8, src_size: u64, dst: *mut u8, dst_cap: u64, level: i8)
    -> u64;
    fn misa77_decompress(src: *const u8, src_size: u64, dst: *mut u8, dst_cap: u64) -> u64;
}

#[test]
fn test_end_offset() {
    test_roundtrip("AAAAAAAAAAAAAAAAAAAAAAAAaAAAAAAAAAAAAAAAAAAAAAAAA");
    test_roundtrip("AAAAAAAAAAAAAAAAAAAAAAAABBBBBBBBBaAAAAAAAAAAAAAAAAAAAAAAAA");
}

#[test]
fn small_compressible_1() {
    test_roundtrip("AAAAAAAAAAAAAAAAAAAAAAAABBBBBBBBBaAAAAAAAAAAAAAAAAAAAAAAAABBBBBBBBBa");
}

#[test]
fn small_compressible_2() {
    test_roundtrip("AAAAAAAAAAAZZZZZZZZAAAAAAAA");
}

#[test]
fn shakespear1() {
    test_roundtrip("to live or not to live");
}

#[test]
fn shakespear2() {
    test_roundtrip("Love is a wonderful terrible thing");
}

#[test]
fn shakespear3() {
    test_roundtrip("There is nothing either good or bad, but thinking makes it so.");
}

#[test]
fn shakespear4() {
    test_roundtrip("I burn, I pine, I perish.");
}

#[test]
fn text_text() {
    test_roundtrip("Save water, it doesn't grow on trees.");
    test_roundtrip("The panda bear has an amazing black-and-white fur.");
    test_roundtrip("The average panda eats as much as 9 to 14 kg of bamboo shoots a day.");
    test_roundtrip("You are 60% water. Save 60% of yourself!");
    test_roundtrip("To cute to die! Save the red panda!");
}

#[test]
fn not_compressible() {
    test_roundtrip("as6yhol.;jrew5tyuikbfewedfyjltre22459ba");
    test_roundtrip("jhflkdjshaf9p8u89ybkvjsdbfkhvg4ut08yfrr");
}

#[test]
fn short_1() {
    test_roundtrip("ahhd");
    test_roundtrip("ahd");
    test_roundtrip("x-29");
    test_roundtrip("x");
    test_roundtrip("k");
    test_roundtrip(".");
    test_roundtrip("ajsdh");
    test_roundtrip("aaaaaa");
}

#[test]
fn short_2() {
    test_roundtrip("aaaaaabcbcbcbc");
}

#[test]
fn empty_string() {
    test_roundtrip("");
}

#[test]
fn nulls() {
    test_roundtrip("\0\0\0\0\0\0\0\0\0\0\0\0\0");
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_so_many_zeros() {
    let data: Vec<u8> = iter::repeat_n(0, 30_000).collect();
    test_roundtrip(data);
}

#[test]
fn repetitive_text() {
    let s = r#"An iterator that knows its exact length.
        Many Iterators don't know how many times they will iterate, but some do. If an iterator knows how many times it can iterate, providing access to that information can be useful. For example, if you want to iterate backwards, a good start is to know where the end is.
        When implementing an ExactSizeIterator, you must also implement Iterator. When doing so, the implementation of size_hint must return the exact size of the iterator.
        The len method has a default implementation, so you usually shouldn't implement it. However, you may be able to provide a more performant implementation than the default, so overriding it in this case makes sense."#;

    test_roundtrip(s);
}

#[test]
fn test_text_1k() {
    test_roundtrip(compression1k());
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_text_34k() {
    test_roundtrip(compression34k());
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_text_65k() {
    test_roundtrip(compression65());
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_json_66k() {
    test_roundtrip(compression66json());
}

#[test]
fn ascending_bytes() {
    let data: Vec<u8> = (0..=255).cycle().take(10_000).collect();
    test_roundtrip(data);
}

#[test]
fn alternating_pattern() {
    let mut data = Vec::with_capacity(5_000);
    for i in 0..5_000u16 {
        data.push((i % 3) as u8);
    }
    test_roundtrip(data);
}

#[test]
#[cfg_attr(miri, ignore)]
fn binary_with_runs() {
    let mut data = Vec::with_capacity(80_000);
    for n in 0..80_000u32 {
        data.push((n as u8).wrapping_mul(0xA).wrapping_add(33) ^ 0xA2);
    }
    test_roundtrip(data);
}

#[test]
#[cfg_attr(miri, ignore)]
fn level0_text_65k() {
    let data = compression65();
    let compressed = m77rip::compress_level(data, 0).unwrap();
    let decompressed = m77rip::decompress(&compressed, data.len()).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn level0_1k() {
    let data = compression1k();
    let compressed = m77rip::compress_level(data, 0).unwrap();
    let decompressed = m77rip::decompress(&compressed, data.len()).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
#[cfg_attr(miri, ignore)]
fn level4_1k() {
    let data = compression1k();
    let compressed = m77rip::compress_level(data, 4).unwrap();
    let decompressed = m77rip::decompress(&compressed, data.len()).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
#[cfg_attr(miri, ignore)]
fn level4_100k() {
    let text = b"the quick brown fox jumps over the lazy dog; test payload\n";
    let mut data = Vec::new();
    while data.len() < 100_000 {
        data.extend_from_slice(text);
    }
    data.truncate(100_000);
    let compressed = m77rip::compress_level(&data, 4).unwrap();
    let decompressed = m77rip::decompress(&compressed, data.len()).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
#[cfg_attr(miri, ignore)]
fn all_levels_dickens_chunks() {
    let Ok(data) = std::fs::read("corpus/silesia/dickens") else {
        return;
    };
    for &n in &[65536, 100000] {
        let chunk = &data[..n.min(data.len())];
        for level in [-1, 0, 1, 2, 3, 4] {
            let compressed = m77rip::compress_level(chunk, level).unwrap();
            match m77rip::decompress(&compressed, chunk.len()) {
                Ok(d) => assert_eq!(d, chunk, "level{level} mismatch at size {n}"),
                Err(e) => panic!("level{level} failed at size {n}: {e}"),
            }
        }
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn level0_500k() {
    let text = b"the quick brown fox jumps over the lazy dog; test payload\n";
    let mut data = Vec::new();
    while data.len() < 500_000 {
        data.extend_from_slice(text);
    }
    data.truncate(500_000);
    let compressed = m77rip::compress_level(&data, 0).unwrap();
    let decompressed = m77rip::decompress(&compressed, data.len()).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
#[cfg(feature = "c-reference")]
fn cpp_decompresses_m77rip_output() {
    let mut cases: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"small payload".to_vec(),
        compression1k().to_vec(),
        (0..=255).cycle().take(10_000).collect(),
    ];

    let mut repeated = Vec::new();
    while repeated.len() < 80_000 {
        repeated.extend_from_slice(b"the quick brown fox jumps over the lazy dog\n");
    }
    cases.push(repeated);

    for data in cases {
        for level in [-1, 0, 1, 2, 3, 4] {
            let compressed = m77rip::compress_level(&data, level).unwrap();
            let mut out = vec![0u8; data.len()];
            // SAFETY: pointers come from live slices. `out` capacity matches
            // the original input length encoded by `compressed`.
            let written = unsafe {
                misa77_decompress(
                    compressed.as_ptr(),
                    compressed.len() as u64,
                    out.as_mut_ptr(),
                    out.len() as u64,
                )
            } as usize;
            assert_eq!(written, data.len(), "C++ decode size mismatch");
            assert_eq!(out, data, "C++ decode content mismatch");
        }
    }
}

#[test]
#[cfg(feature = "c-reference")]
fn m77rip_decompresses_cpp_output() {
    let cases: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"small payload".to_vec(),
        compression1k().to_vec(),
        (0..=255).cycle().take(10_000).collect(),
    ];

    for data in cases {
        for level in [-1, 0, 1, 2, 3, 4] {
            // SAFETY: C function has no pointer arguments and accepts any u64 size.
            let bound = unsafe { misa77_compress_bound(data.len() as u64, level) } as usize;
            let mut compressed = vec![0u8; bound];
            // SAFETY: pointers come from live slices. `compressed` has the
            // bound returned by the same C++ compressor level.
            let written = unsafe {
                misa77_compress(
                    data.as_ptr(),
                    data.len() as u64,
                    compressed.as_mut_ptr(),
                    compressed.len() as u64,
                    level,
                )
            } as usize;
            assert!(written > 0, "C++ level {level} compression failed");
            compressed.truncate(written);

            let decompressed = m77rip::decompress(&compressed, data.len()).unwrap();
            assert_eq!(
                decompressed, data,
                "Rust decode of C++ level {level} mismatch"
            );
        }
    }
}

#[test]
#[cfg(feature = "c-reference")]
#[cfg_attr(miri, ignore)]
fn m77rip_matches_cpp_output_for_exact_levels() {
    let mut repeated = Vec::new();
    while repeated.len() < 100_000 {
        repeated.extend_from_slice(b"the quick brown fox jumps over the lazy dog; test payload\n");
    }
    repeated.truncate(100_000);

    let cases: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"small payload".to_vec(),
        compression1k().to_vec(),
        (0..=255).cycle().take(10_000).collect(),
        repeated,
    ];

    for data in cases {
        for level in [-1, 0, 1, 2, 4] {
            let rust = m77rip::compress_level(&data, level).unwrap();

            // SAFETY: C function has no pointer arguments and accepts any u64 size.
            let bound = unsafe { misa77_compress_bound(data.len() as u64, level) } as usize;
            let mut cpp = vec![0u8; bound];
            // SAFETY: pointers come from live slices. `cpp` has the bound
            // returned by the same C++ compressor level.
            let written = unsafe {
                misa77_compress(
                    data.as_ptr(),
                    data.len() as u64,
                    cpp.as_mut_ptr(),
                    cpp.len() as u64,
                    level,
                )
            } as usize;
            assert!(written > 0, "C++ level {level} compression failed");
            cpp.truncate(written);

            assert_eq!(rust, cpp, "level {level} output differs for {}", data.len());
        }
    }
}

use proptest::{prelude::*, test_runner::FileFailurePersistence};

fn vec_of_vec() -> impl Strategy<Value = Vec<Vec<u8>>> {
    const N: u8 = 200;
    let length = 0..N;
    length.prop_flat_map(vec_from_length)
}

fn vec_from_length(length: u8) -> impl Strategy<Value = Vec<Vec<u8>>> {
    const K: usize = u8::MAX as usize;
    let mut result = vec![];
    for index in 1..length {
        let inner = proptest::collection::vec(0..index, 0..K);
        result.push(inner);
    }
    result
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource("regressions"))),
        ..Default::default()
    })]

    #[test]
    #[cfg_attr(miri, ignore)]
    fn proptest_roundtrip(v in vec_of_vec()) {
        let data: Vec<u8> = v.iter().flat_map(|v| v.iter()).cloned().collect::<Vec<_>>();
        test_roundtrip(data);
    }
}
