//! A random-access [`Source`] over a file.
use rvp_host::{HostError, Source};
use std::io::{Read, Seek, SeekFrom};

/// A file opened for the player.
pub struct FileSource {
    file: std::fs::File,
    len: u64,
    name: String,
    pos: u64,
}

impl FileSource {
    /// Open `path`.
    pub fn open(path: &str) -> Result<Self, HostError> {
        let file = std::fs::File::open(path).map_err(|e| HostError(format!("{path}: {e}")))?;
        let len = file.metadata().map_err(|e| HostError(e.to_string()))?.len();
        let name =
            std::path::Path::new(path).file_name().map_or(path.to_string(), |n| n.to_string_lossy().into());
        Ok(Self { file, len, name, pos: 0 })
    }
}

impl Source for FileSource {
    async fn size(&self) -> Option<u64> {
        Some(self.len)
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, HostError> {
        if self.pos != offset {
            self.file.seek(SeekFrom::Start(offset)).map_err(|e| HostError(e.to_string()))?;
            self.pos = offset;
        }
        let n = self.file.read(buf).map_err(|e| HostError(e.to_string()))?;
        self.pos += n as u64;
        Ok(n)
    }

    fn name(&self) -> &str {
        &self.name
    }
}
