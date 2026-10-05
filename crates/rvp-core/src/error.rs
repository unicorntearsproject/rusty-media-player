//! Error type shared by the crates.
use alloc::string::String;
use core::fmt;

/// Result alias using [`Error`].
pub type Result<T> = core::result::Result<T, Error>;

/// Errors that cross crate boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Input ended before a complete structure was read.
    Truncated,
    /// The data is malformed; the string says where.
    Invalid(String),
    /// A valid feature that this player does not support (codec, profile, container feature).
    Unsupported(String),
    /// The host failed (I/O, device).
    Host(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated => f.write_str("unexpected end of data"),
            Error::Invalid(s) => write!(f, "invalid data: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Host(s) => write!(f, "host error: {s}"),
        }
    }
}

impl core::error::Error for Error {}
