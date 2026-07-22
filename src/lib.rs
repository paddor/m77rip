pub use m77rip_core::{Error, format};
#[cfg(feature = "alloc")]
pub use m77rip_decode::decompress;
pub use m77rip_decode::{decompress_into, decompressed_size};
pub use m77rip_encode::{
    compress, compress_bound, compress_bound_level, compress_into, compress_into_level,
    compress_level,
};
