#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let original_size =
        u64::from_le_bytes(data[..8].try_into().unwrap()) as usize;
    if original_size > 10 * 1024 * 1024 {
        return;
    }
    // Must not panic or corrupt memory. Errors are fine.
    let _ = m77rip::decompress(data, original_size);
});
