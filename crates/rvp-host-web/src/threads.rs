//! Threads for the shared-memory build: the spawner that starts Web Workers, the pool the kernels use and the worker
//! entry point. Only compiled when the module has atomics (`cargo xtask web --threads` builds that variant); the
//! plain build has none of this and stays single-threaded.
use std::cell::RefCell;
use wasm_bindgen::prelude::*;

thread_local! {
    /// The JavaScript function of this thread's realm that starts a worker for a closure pointer.
    static JS_SPAWN: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
}

fn spawn_on_worker(f: Box<dyn FnOnce() + Send + 'static>) {
    let ptr = Box::into_raw(Box::new(f)) as u32;
    JS_SPAWN.with(|j| {
        if let Some(func) = j.borrow().as_ref() {
            let _ = func.call1(&JsValue::NULL, &JsValue::from(ptr));
        }
    });
}

/// Called by every worker (from `worker.js`): run the closure that `spawn_on_worker` boxed.
#[wasm_bindgen]
pub fn rvp_worker_entry(ptr: u32, spawn: js_sys::Function) {
    JS_SPAWN.with(|j| *j.borrow_mut() = Some(spawn));
    // SAFETY: `ptr` came from `Box::into_raw` in `spawn_on_worker` and is run exactly once.
    let f = unsafe { Box::from_raw(ptr as *mut Box<dyn FnOnce() + Send + 'static>) };
    (*f)();
}

/// Turn threads on: remember the page's spawn function and start the pool of `size` threads (the caller of a
/// parallel kernel counts as one). Returns the pool size, or 0 if threads are unavailable.
#[wasm_bindgen]
pub fn rvp_init_threads(spawn: js_sys::Function, hardware: u32) -> u32 {
    JS_SPAWN.with(|j| *j.borrow_mut() = Some(spawn));
    rvp_par::set_spawner(spawn_on_worker);
    // This is the page's main thread: it draws every frame and must not wait for workers.
    rvp_par::mark_ui_thread();
    if !rvp_par::available() {
        return 0;
    }
    let size = rvp_par::pool_size(hardware as usize);
    rvp_par::Pool::new(size).install();
    size as u32
}

/// True when this module was built with shared memory (so the page may start workers on it).
#[wasm_bindgen]
pub fn rvp_has_threads() -> bool {
    cfg!(target_feature = "atomics")
}

/// An allocator for the shared-memory build that never sleeps: the standard one waits on an atomic when two threads
/// allocate at once, which a browser's main thread may not do. A spin lock around it keeps the standard lock free.
#[cfg(target_feature = "atomics")]
pub struct SpinAlloc;

#[cfg(target_feature = "atomics")]
mod spin_alloc {
    use super::SpinAlloc;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicBool, Ordering};

    static LOCK: AtomicBool = AtomicBool::new(false);

    fn lock() {
        while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            std::hint::spin_loop();
        }
    }

    fn unlock() {
        LOCK.store(false, Ordering::Release);
    }

    // SAFETY: forwards to `System` with every call serialised by the lock.
    unsafe impl GlobalAlloc for SpinAlloc {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            lock();
            // SAFETY: same contract as ours.
            let p = unsafe { System.alloc(l) };
            unlock();
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            lock();
            // SAFETY: same contract as ours.
            unsafe { System.dealloc(p, l) };
            unlock();
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            lock();
            // SAFETY: same contract as ours.
            let p = unsafe { System.alloc_zeroed(l) };
            unlock();
            p
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            lock();
            // SAFETY: same contract as ours.
            let q = unsafe { System.realloc(p, l, n) };
            unlock();
            q
        }
    }
}

#[cfg(target_feature = "atomics")]
#[global_allocator]
static ALLOC: SpinAlloc = SpinAlloc;
