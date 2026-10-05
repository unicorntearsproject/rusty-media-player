//! Settings, resume positions, the library index and the saved queue as files under the user's data directory.
//!
//! One file per key, named by escaping the key; a write goes to a temporary file first and is renamed over the old one, so a
//! crash never leaves half a file. An empty value deletes the file.
use rvp_host::Storage;
use std::path::{Path, PathBuf};

/// A [`Storage`] over a directory.
pub struct FileStorage {
    dir: PathBuf,
}

impl FileStorage {
    /// Store under `dir` (made on first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file a key lives in.
    pub fn path_for(&self, key: &str) -> PathBuf {
        self.dir.join(file_name(key))
    }

    /// Read a key synchronously.
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path_for(key)).ok()
    }

    /// Write a key synchronously (an empty value deletes it).
    pub fn put(&self, key: &str, value: &[u8]) {
        let path = self.path_for(key);
        if value.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, value).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// A key as a file name: letters, digits and `-_.` stay, everything else becomes `%xx`; long names are cut and given a hash.
pub fn file_name(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 8);
    for b in key.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02x}"));
        }
    }
    if out.len() > 120 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in key.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
        out.truncate(100);
        out.push_str(&format!("-{h:016x}"));
    }
    out.push_str(".bin");
    out
}

impl Storage for FileStorage {
    async fn load(&mut self, key: &str) -> Option<Vec<u8>> {
        self.get(key)
    }

    async fn store(&mut self, key: &str, value: &[u8]) {
        self.put(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvp_core::task::block_on;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rvp-desktop-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn keys_round_trip_and_empty_deletes() {
        let dir = scratch("storage");
        let mut s = FileStorage::new(&dir);
        block_on(async {
            s.store("library/art/00ff", b"abc").await;
            s.store("resume:Some File (1).mkv", b"123").await;
            assert_eq!(s.load("library/art/00ff").await.as_deref(), Some(&b"abc"[..]));
            assert_eq!(s.load("resume:Some File (1).mkv").await.as_deref(), Some(&b"123"[..]));
            assert_eq!(s.load("nope").await, None);
            s.store("library/art/00ff", b"").await;
            assert_eq!(s.load("library/art/00ff").await, None);
        });
        assert!(!s.path_for("x").to_string_lossy().contains("//"));
        std::fs::remove_file(s.path_for("resume:Some File (1).mkv")).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn long_and_odd_keys_make_safe_distinct_names() {
        let a = file_name(&format!("resume:{}", "a".repeat(300)));
        let b = file_name(&format!("resume:{}b", "a".repeat(300)));
        assert!(a.len() < 140 && a != b);
        assert!(!file_name("../../etc/passwd").contains('/'));
        assert!(!file_name("C:\\x").contains('\\') && !file_name("C:\\x").contains(':'));
    }
}
