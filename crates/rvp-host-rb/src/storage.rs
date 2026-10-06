//! Storage on the key-value store. Thumbnails go to the cache class (the OS may evict them; they are rebuilt from tags).
use crate::api;
use crate::shared::Shared;
use bucket_v0_sys::{self as sys, err};
use rvp_host::Storage;
use std::rc::Rc;

/// How long a `kv_load` that is `-BUSY` is waited for before it counts as missing, microseconds.
const LOAD_TIMEOUT_US: i64 = 5_000_000;

/// Keys under these prefixes are recomputable.
fn is_cache(key: &str) -> bool {
    key.starts_with("library/art/")
}

/// `Storage` over `kv_load` and `kv_store`.
pub struct RbStorage {
    shared: Rc<Shared>,
}

impl RbStorage {
    /// A store.
    pub fn new(shared: Rc<Shared>) -> Self {
        Self { shared }
    }

    /// Wait until every queued store is on disk (`kv_flush`): before a restart and when told to terminate.
    pub fn flush(&self) {
        // SAFETY: no pointers.
        let r = unsafe { sys::kv_flush() };
        if r < 0 {
            api::warn(&format!("kv_flush failed: {}", api::code_name(r)));
        }
    }

    fn load_now(&self, key: &str) -> Option<Vec<u8>> {
        let deadline = api::now_us() + LOAD_TIMEOUT_US;
        loop {
            // A value that changes between the two calls makes the second answer longer than the buffer; ask again.
            // SAFETY: the buffer holds the announced size; the key is the string's range.
            let got = api::bytes_call(|p, cap| unsafe {
                sys::kv_load(key.as_ptr(), api::len32(key.len()), p, cap)
            });
            match got {
                Ok(v) if v.is_empty() => return None,
                Ok(v) => return Some(v),
                Err(err::NOT_FOUND) => return None,
                Err(err::BUSY) => {
                    let left = deadline - api::now_us();
                    if left <= 0 || !self.shared.wait_io(left) {
                        api::warn(&format!("kv_load `{key}` timed out"));
                        return None;
                    }
                }
                Err(e) => {
                    api::warn(&format!("kv_load `{key}` failed: {}", api::code_name(e)));
                    return None;
                }
            }
        }
    }
}

impl Storage for RbStorage {
    async fn load(&mut self, key: &str) -> Option<Vec<u8>> {
        self.load_now(key)
    }

    async fn store(&mut self, key: &str, value: &[u8]) {
        let flags = if is_cache(key) && !value.is_empty() { sys::kv::CACHE } else { 0 };
        // SAFETY: the ranges are the slices'.
        let r = unsafe {
            sys::kv_store(key.as_ptr(), api::len32(key.len()), value.as_ptr(), api::len32(value.len()), flags)
        };
        if r < 0 {
            // `-NO_SPACE` and `-TOO_LARGE` leave the old value in place; the app carries on with what it has.
            api::warn(&format!("kv_store `{key}` ({} bytes) failed: {}", value.len(), api::code_name(r)));
        }
    }
}
