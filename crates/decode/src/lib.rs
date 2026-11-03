#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

mod decode;
mod heavy;
mod light;
mod primitives;

#[cfg(feature = "alloc")]
pub use decode::decompress;
pub use decode::{decompress_into, decompressed_size};
