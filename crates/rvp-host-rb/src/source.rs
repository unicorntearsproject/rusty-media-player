//! A `Source` over an App API file handle. `file_read_at` never blocks: `-BUSY` becomes `Pending` (the player's executor polls
//! the task again on its next tick, and `IO_READY` wakes `events_wait` so that tick comes at once).
use crate::api;
use bucket_v0_sys::{self as sys, err};
use rvp_core::task::yield_now;
use rvp_host::{HostError, Source};

/// How far ahead the next read is hinted to the OS, bytes.
const PREFETCH: u64 = 512 * 1024;

/// An open file. Closing the handle is up to this value: it closes it when dropped.
pub struct RbSource {
    handle: i32,
    name: String,
    size: Option<u64>,
    max_read: usize,
    /// Reads hinted up to here already.
    prefetched: u64,
}

impl RbSource {
    /// Take over an open handle (the source closes it).
    pub fn from_handle(handle: i32, max_read: usize) -> Self {
        // SAFETY: `file_size` takes no pointers.
        let size = match unsafe { sys::file_size(handle) } {
            n if n >= 0 => Some(n as u64),
            _ => None,
        };
        // SAFETY: `text_call` passes a buffer of the size it announces.
        let name = api::text_call(|p, cap| unsafe { sys::file_name(handle, p, cap) })
            .ok()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "file".into());
        Self { handle, name, size, max_read: max_read.max(4096), prefetched: 0 }
    }

    /// The handle.
    pub fn handle(&self) -> i32 {
        self.handle
    }
}

impl Drop for RbSource {
    fn drop(&mut self) {
        // SAFETY: no pointers.
        unsafe { sys::file_close(self.handle) };
    }
}

impl Source for RbSource {
    async fn size(&self) -> Option<u64> {
        self.size
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, HostError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min(self.max_read);
        loop {
            // SAFETY: `buf` holds at least `want` bytes.
            let n = unsafe { sys::file_read_at(self.handle, offset as i64, buf.as_mut_ptr(), want as i32) };
            match n {
                n if n >= 0 => {
                    let end = offset + n as u64;
                    if n > 0 && end + PREFETCH / 2 > self.prefetched {
                        self.prefetched = end + PREFETCH;
                        // SAFETY: no pointers.
                        unsafe { sys::file_prefetch(self.handle, end as i64, PREFETCH as i32) };
                    }
                    return Ok(n as usize);
                }
                err::BUSY => yield_now().await,
                e => return Err(HostError(format!("{}: read failed ({})", self.name, api::code_name(e)))),
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}
