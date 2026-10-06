//! Updates and self-integration for the desktop app.
pub mod apply;
pub mod error;
pub mod fetch;
pub mod kind;
pub mod manifest;
pub mod updater;
pub mod verify;
pub mod version;
pub use apply::Finish;
pub use error::UpdateError;
pub use kind::{Env, InstallKind, Os};
pub use manifest::{FileEntry, Manifest};
pub use updater::{Checked, Config, MANIFEST_URL, Offer, Release, State, Updater};
pub use verify::{RELEASE_FINGERPRINT, Verifier, VerifyError};
pub use version::Version;

#[cfg(test)]
mod tests;
