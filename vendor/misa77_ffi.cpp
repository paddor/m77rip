// C wrapper around the misa77 C++ API for Rust FFI.
#include "misa77/misa77.h"
#include <cstdint>

extern "C" {

uint64_t misa77_compress_bound(uint64_t src_size) {
    return misa77::compress_bound(src_size);
}

uint64_t misa77_compress(const uint8_t* src,
                         uint64_t src_size,
                         uint8_t* dst,
                         uint64_t dst_cap,
                         uint8_t level) {
    return misa77::compress(src, src_size, dst, dst_cap, misa77::config(level));
}

uint64_t misa77_decompressed_size(const uint8_t* src) {
    return misa77::decompressed_size(src);
}

uint64_t misa77_decompressed_buffer_bound(uint64_t src_size) {
    return misa77::decompressed_buffer_bound(src_size);
}

uint64_t misa77_decompress(const uint8_t* src,
                           uint64_t src_size,
                           uint8_t* dst,
                           uint64_t dst_cap) {
    return misa77::decompress(src, src_size, dst, dst_cap);
}

} // extern "C"
