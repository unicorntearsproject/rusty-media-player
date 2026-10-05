//! Helpers over the host `Source`: exact reads that survive short reads, and a size guard.
use alloc::vec::Vec;
use rvp_core::{Error, Result};
use rvp_host::Source;

/// Largest single allocation a demuxer makes for container metadata or one packet.
pub(crate) const MAX_READ: u64 = 256 << 20;

pub(crate) struct Reader<S: Source> {
    src: S,
    size: Option<u64>,
}

impl<S: Source> Reader<S> {
    pub(crate) async fn new(src: S) -> Self {
        let size = src.size().await;
        Self { src, size }
    }

    pub(crate) fn size(&self) -> Option<u64> {
        self.size
    }

    /// Read until `buf` is full or the data ends; returns the number of bytes read.
    pub(crate) async fn read_upto(&mut self, off: u64, buf: &mut [u8]) -> Result<usize> {
        let mut done = 0;
        while done < buf.len() {
            let n = self.src.read_at(off + done as u64, &mut buf[done..]).await?;
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }

    /// Read exactly `buf.len()` bytes or fail with `Truncated`.
    pub(crate) async fn read_exact(&mut self, off: u64, buf: &mut [u8]) -> Result<()> {
        if self.read_upto(off, buf).await? == buf.len() { Ok(()) } else { Err(Error::Truncated) }
    }

    /// Read `n` bytes into a new vector (bounded by the source size and [`MAX_READ`]).
    pub(crate) async fn read_vec(&mut self, off: u64, n: u64) -> Result<Vec<u8>> {
        if n > MAX_READ || self.size.is_some_and(|s| off.saturating_add(n) > s) {
            return Err(Error::Truncated);
        }
        let mut v = alloc::vec![0u8; n as usize];
        self.read_exact(off, &mut v).await?;
        Ok(v)
    }
}

/// A forward cursor over an in-memory byte slice. Every read is bounds-checked.
pub(crate) struct Cur<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    pub(crate) fn new(d: &'a [u8]) -> Self {
        Self { d, p: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.d.len() - self.p
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(Error::Truncated);
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }

    pub(crate) fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    pub(crate) fn rest(&mut self) -> &'a [u8] {
        let s = &self.d[self.p..];
        self.p = self.d.len();
        s
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub(crate) fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub(crate) fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
}

pub(crate) fn invalid<T>(what: &str) -> Result<T> {
    Err(Error::Invalid(alloc::string::String::from(what)))
}
