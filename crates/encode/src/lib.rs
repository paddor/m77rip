#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(not(feature = "paranoid"))]
#[allow(unused_macros)]
macro_rules! paranoid_unsafe_call {
    ($e:expr) => {
        // SAFETY: Default encoder call sites document and enforce each unsafe
        // primitive contract before invoking this macro.
        unsafe { $e }
    };
}

#[cfg(feature = "paranoid")]
#[allow(unused_macros)]
macro_rules! paranoid_unsafe_call {
    ($e:expr) => {
        $e
    };
}

mod encode;
mod sais;

pub use encode::{
    compress, compress_bound, compress_bound_level, compress_into, compress_into_level,
    compress_level,
};
