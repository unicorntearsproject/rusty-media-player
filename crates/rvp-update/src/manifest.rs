//! `rusty-wave-latest.json`: what the latest release is and where its files are (written by `cargo xtask dist manifest`).
use crate::error::UpdateError;
use serde::Deserialize;
use std::collections::BTreeMap;

/// The newest manifest format this program reads.
pub const SCHEMA: u32 = 1;

/// One downloadable file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileEntry {
    /// CPU architecture (informational).
    #[serde(default)]
    pub arch: String,
    /// File name.
    pub name: String,
    /// Where to get it (the versioned, immutable file).
    pub url: String,
    /// Exact size in bytes.
    pub size: u64,
    /// SHA-256, hex.
    pub sha256: String,
    /// The detached ASCII-armored signature of the file.
    pub signature_url: String,
    /// The zsync control file (AppImage only; not used by the built-in updater).
    #[serde(default)]
    pub zsync_url: Option<String>,
}

/// The latest release.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    /// Format version.
    pub schema: u32,
    /// The release's version.
    pub version: String,
    /// Release date, `YYYY-MM-DD`.
    #[serde(default)]
    pub released: String,
    /// Fingerprint of the key that signed the files.
    pub key_fingerprint: String,
    /// Files by platform key (`linux-appimage`, `windows-installer`, ...).
    pub files: BTreeMap<String, FileEntry>,
}

impl Manifest {
    /// Parse manifest bytes (unknown fields are ignored, a newer schema is refused).
    pub fn parse(bytes: &[u8]) -> Result<Manifest, UpdateError> {
        // The schema first, on its own, so a future format is reported as such rather than as a parse error.
        #[derive(Deserialize)]
        struct Head {
            schema: u32,
        }
        let head: Head =
            serde_json::from_slice(bytes).map_err(|e| UpdateError::BadManifest(e.to_string()))?;
        if head.schema > SCHEMA || head.schema == 0 {
            return Err(UpdateError::UnsupportedSchema(head.schema));
        }
        serde_json::from_slice(bytes).map_err(|e| UpdateError::BadManifest(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "schema": 1, "version": "0.0.3", "released": "2026-10-06",
      "key_fingerprint": "E13FF843723D54068E45A3FF54BF2FA407093CEE", "future_field": [1,2],
      "files": {
        "linux-appimage": {"arch":"x86_64","name":"a.AppImage","url":"https://x/a.AppImage","size":10,"sha256":"ab",
          "signature_url":"https://x/a.AppImage.asc","zsync_url":"https://x/z.zsync"},
        "linux-deb": {"arch":"amd64","name":"a.deb","url":"https://x/a.deb","size":3,"sha256":"cd","signature_url":"https://x/a.deb.asc"}
      }
    }"#;

    #[test]
    fn reads_the_files_xtask_writes() {
        let m = Manifest::parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(m.version, "0.0.3");
        assert_eq!(m.files["linux-appimage"].zsync_url.as_deref(), Some("https://x/z.zsync"));
        assert_eq!(m.files["linux-deb"].zsync_url, None);
        assert_eq!(m.files["linux-deb"].size, 3);
    }

    #[test]
    fn a_newer_schema_or_garbage_is_refused() {
        let newer = SAMPLE.replace("\"schema\": 1", "\"schema\": 2");
        assert_eq!(Manifest::parse(newer.as_bytes()), Err(UpdateError::UnsupportedSchema(2)));
        let zero = SAMPLE.replace("\"schema\": 1", "\"schema\": 0");
        assert_eq!(Manifest::parse(zero.as_bytes()), Err(UpdateError::UnsupportedSchema(0)));
        assert!(matches!(Manifest::parse(b"{"), Err(UpdateError::BadManifest(_))));
        assert!(matches!(Manifest::parse(br#"{"schema":1}"#), Err(UpdateError::BadManifest(_))));
    }
}
