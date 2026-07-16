use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InputTooShort,
    OutputTooSmall { need: usize, have: usize },
    CorruptInput,
    SizeOverflow { size: u64 },
    SizeMismatch { expected: usize, actual: usize },
    InvalidLevel { level: u8 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InputTooShort => write!(f, "input too short"),
            Error::OutputTooSmall { need, have } => {
                write!(f, "output buffer too small: need {need} bytes, have {have}")
            }
            Error::CorruptInput => write!(f, "corrupt input"),
            Error::SizeOverflow { size } => {
                write!(f, "size {size} exceeds platform address space")
            }
            Error::SizeMismatch { expected, actual } => {
                write!(
                    f,
                    "size mismatch: expected {expected} bytes, stream has {actual}"
                )
            }
            Error::InvalidLevel { level } => write!(f, "invalid compression level: {level}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
