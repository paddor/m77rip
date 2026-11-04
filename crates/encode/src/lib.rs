//! misa77 block compression.

#![forbid(unsafe_code)]

mod compress;
mod heavy;
mod light;
mod read;
mod sa;
mod sais;
mod stream;

pub use compress::{
    compress, compress_bound, compress_bound_level, compress_into, compress_into_level,
    compress_level,
};
