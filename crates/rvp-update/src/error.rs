//! One error type for the whole update path, with messages written for the person using the app.
use crate::verify::VerifyError;
use std::fmt;

/// Why an update step failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// The network, a file or the system said no.
    Io(String),
    /// A URL that is not HTTPS (or a local file, for tests) was asked for.
    Insecure(String),
    /// The server answered with an error status.
    Http(u16),
    /// More data came than the manifest or a limit allows.
    TooLarge,
    /// The user cancelled.
    Cancelled,
    /// The manifest could not be understood.
    BadManifest(String),
    /// The manifest is for a newer format of this program than we know.
    UnsupportedSchema(u32),
    /// The manifest names another key than the release key we trust.
    UntrustedKey,
    /// The downloaded file is not what the manifest says (size or SHA-256).
    Corrupt(String),
    /// A signature did not verify.
    Signature(VerifyError),
    /// The program cannot replace itself where it is.
    NotWritable(String),
    /// This kind of install is updated another way.
    NotSelfUpdatable,
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::Io(e) => write!(f, "{e}"),
            UpdateError::Insecure(u) => write!(f, "refusing to fetch {u}: only https:// is allowed"),
            UpdateError::Http(c) => write!(f, "the server answered {c}"),
            UpdateError::TooLarge => f.write_str("the download is larger than expected"),
            UpdateError::Cancelled => f.write_str("cancelled"),
            UpdateError::BadManifest(e) => write!(f, "the update information is not valid: {e}"),
            UpdateError::UnsupportedSchema(s) => {
                write!(
                    f,
                    "the update information is in a newer format ({s}); download Rusty Wave again from its site"
                )
            }
            UpdateError::UntrustedKey => {
                f.write_str("the update is signed by a key this version does not trust")
            }
            UpdateError::Corrupt(e) => write!(f, "the download is damaged ({e}); nothing was changed"),
            UpdateError::Signature(e) => write!(f, "{e}; nothing was changed"),
            UpdateError::NotWritable(p) => {
                write!(f, "Rusty Wave cannot replace itself in {p} (no permission)")
            }
            UpdateError::NotSelfUpdatable => {
                f.write_str("this installation is updated through its package manager")
            }
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        UpdateError::Io(e.to_string())
    }
}

impl From<VerifyError> for UpdateError {
    fn from(e: VerifyError) -> Self {
        UpdateError::Signature(e)
    }
}
