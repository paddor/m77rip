#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]
use libfuzzer_sys::fuzz_target;

// SAFETY: These declarations match the C wrapper ABI. Calls validate pointer
// lifetimes and capacities at each call site below.
unsafe extern "C" {
    fn misa77_compress_bound(src_size: u64, level: u8) -> u64;
    fn misa77_compress(
        src: *const u8,
        src_size: u64,
        dst: *mut u8,
        dst_cap: u64,
        level: u8,
    ) -> u64;
}

fuzz_target!(|data: &[u8]| {
    for level in 0..=2 {
        if level == 2 && data.len() > 4096 {
            continue;
        }
        // SAFETY: C function has no pointer arguments and accepts any u64 size.
        let bound = unsafe { misa77_compress_bound(data.len() as u64, level) } as usize;
        let mut compressed = vec![0u8; bound];
        // SAFETY: Pointers come from live `data` and `compressed` slices.
        // Capacity matches the bound returned by the reference compressor.
        let csize = unsafe {
            misa77_compress(
                data.as_ptr(),
                data.len() as u64,
                compressed.as_mut_ptr(),
                compressed.len() as u64,
                level,
            )
        } as usize;
        if csize == 0 {
            continue;
        }
        compressed.truncate(csize);
        let decompressed = m77rip::decompress(&compressed, data.len()).unwrap();
        assert_eq!(&decompressed, data);
    }
});
