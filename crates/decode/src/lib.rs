#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(not(feature = "paranoid"))]
macro_rules! paranoid_unsafe_call {
    ($e:expr) => {
        // SAFETY: Every call site validates the bounds and aliasing invariants
        // documented by the selected primitive before reaching this macro.
        unsafe { $e }
    };
}

#[cfg(feature = "paranoid")]
macro_rules! paranoid_unsafe_call {
    ($e:expr) => {
        $e
    };
}

mod decode;
pub(crate) mod primitives;

#[cfg(feature = "alloc")]
pub use decode::decompress;
pub use decode::{decompress_into, decompressed_size};
