//! Error type of this crate. Cheap to create (no allocation) so parsers can fail in hot paths.
use core::fmt;

/// Why a bitstream could not be parsed or decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The data ended before the structure was complete.
    Truncated,
    /// The data violates the specification.
    Invalid(&'static str),
    /// Valid, but outside what this decoder implements (interlace, FMO, 4:2:2, high bit depth, ...).
    Unsupported(&'static str),
}

/// Result alias for this crate.
pub type Result<T> = core::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated => f.write_str("h264: unexpected end of data"),
            Error::Invalid(s) => write!(f, "h264: invalid data: {s}"),
            Error::Unsupported(s) => write!(f, "h264: unsupported: {s}"),
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
