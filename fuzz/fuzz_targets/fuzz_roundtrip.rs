#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let compressed = m77rip::compress(data);
    let decompressed = m77rip::decompress(&compressed, data.len()).unwrap();
    assert_eq!(&decompressed, data);
});
