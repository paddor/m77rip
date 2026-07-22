#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let Some(original_size) = m77rip::decompressed_size(data).and_then(|n| n.try_into().ok())
    else {
        return;
    };
    if original_size > 10 * 1024 * 1024 {
        return;
    }
    // Must not panic or corrupt memory. Errors are fine.
    let _ = m77rip::decompress(data, original_size);
});
