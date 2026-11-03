#![forbid(unsafe_code)]

mod encode;
mod sais;

pub use encode::{
    compress, compress_bound, compress_bound_level, compress_into, compress_into_level,
    compress_level,
};
