//! Error type of this crate.
use core::fmt;

/// Why a stream could not be parsed or decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The data ended before the structure was complete.
    Truncated,
    /// The data violates the specification.
    Invalid(&'static str),
    /// Valid, but outside what is implemented (a profile other than Main and Main 10, 4:2:2, 12 bits, layers, ...).
    Unsupported(&'static str),
}

/// Result alias for this crate.
pub type Result<T> = core::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated => f.write_str("hevc: unexpected end of data"),
            Error::Invalid(s) => write!(f, "hevc: invalid data: {s}"),
            Error::Unsupported(s) => write!(f, "hevc: unsupported: {s}"),
        }
    }
}

impl core::error::Error for Error {}

impl From<Error> for rvp_core::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Truncated => rvp_core::Error::Truncated,
            Error::Invalid(s) => rvp_core::Error::Invalid(alloc::string::String::from(s)),
            Error::Unsupported(s) => rvp_core::Error::Unsupported(alloc::string::String::from(s)),
        }
    }
}
