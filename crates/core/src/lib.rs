#![forbid(unsafe_code)]
#![cfg_attr(not(feature = "std"), no_std)]

mod error;
pub mod format;

pub use error::Error;
