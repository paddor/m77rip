#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    for level in 0..=1 {
        let compressed = m77rip::compress_level(data, level).unwrap();
        let decompressed = m77rip::decompress(&compressed, data.len()).unwrap();
        assert_eq!(&decompressed, data);
    }
});
