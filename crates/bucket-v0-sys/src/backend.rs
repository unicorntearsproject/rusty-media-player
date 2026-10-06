//! Native targets only (feature `backend`): the imports call a [`Backend`] installed on the thread, so an app compiled for the
//! host can be run against a mock host (see the `bucket-v0-mock` crate) or a simulator, without WebAssembly.
//!
//! One caveat: structs carry pointers as `u32`, which cannot hold a 64-bit address. [`ptr32`](crate::ptr32) therefore hands out
//! small tokens here and [`deref32`] turns a token back into the address, valid until the call that received the struct returns.
use crate::funcs::Backend;
use std::cell::RefCell;
use std::sync::Arc;
use std::thread_local;
use std::vec::Vec;

thread_local! {
    static CURRENT: RefCell<Option<Arc<dyn Backend>>> = const { RefCell::new(None) };
    static POINTERS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Put `backend` in charge of this thread's `bucket_v0` calls. The previous one (if any) comes back when the guard drops.
pub fn install(backend: Arc<dyn Backend>) -> Guard {
    Guard { previous: CURRENT.with(|c| c.borrow_mut().replace(backend)) }
}

/// The backend of this thread, if one is installed (a mock hands it to the threads it starts).
pub fn current() -> Option<Arc<dyn Backend>> {
    CURRENT.with(|c| c.borrow().clone())
}

pub(crate) fn current_or_panic() -> Arc<dyn Backend> {
    current().expect("no bucket_v0 backend is installed on this thread (see bucket_v0_sys::backend::install)")
}

/// Restores the previous backend when dropped.
pub struct Guard {
    previous: Option<Arc<dyn Backend>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let prev = self.previous.take();
        CURRENT.with(|c| *c.borrow_mut() = prev);
    }
}

pub(crate) fn make_token(p: usize) -> u32 {
    if p == 0 {
        return 0;
    }
    POINTERS.with(|v| {
        let mut v = v.borrow_mut();
        v.push(p);
        v.len() as u32
    })
}

pub(crate) fn end_call() {
    POINTERS.with(|v| v.borrow_mut().clear());
}

/// The address behind a token from [`ptr32`](crate::ptr32) (0 stays null). Call it while handling the call that got the struct.
pub fn deref32(token: u32) -> *const u8 {
    if token == 0 {
        return core::ptr::null();
    }
    POINTERS.with(|v| v.borrow().get(token as usize - 1).copied().unwrap_or(0)) as *const u8
}
