//! Threads for the player: a worker pool behind [`rvp_core::par::Parallel`] and [`ThreadedVideoDecoder`], which runs
//! a video decoder on a thread of its own so decoding never competes with the UI thread.
//!
//! Works natively with `std::thread`, and on `wasm32-unknown-unknown` when the module is built with shared memory
//! (`+atomics`); there the host (the page's JavaScript) provides the worker spawner through [`set_spawner`]. Without
//! threads ([`available`] is false) nothing here is used and the player stays single-threaded.
//!
//! No code on the caller's side ever sleeps in the OS: hand-over between threads uses spin locks and `unpark`, so the
//! functions are safe to call from a browser's main thread. Only the worker threads wait (with `park`).
#![forbid(unsafe_op_in_unsafe_fn)]

mod decoder;
pub mod h264;
mod pool;

pub use decoder::ThreadedVideoDecoder;
pub use pool::{Pool, mark_ui_thread};

use std::sync::OnceLock;

/// Starts a thread running the given closure. The host provides one where `std::thread::spawn` does not work
/// (browsers: a Web Worker that runs the closure).
pub type SpawnFn = fn(Box<dyn FnOnce() + Send + 'static>);

static SPAWNER: OnceLock<SpawnFn> = OnceLock::new();

/// Provide the thread spawner of the host (once, before any thread is needed).
pub fn set_spawner(f: SpawnFn) {
    let _ = SPAWNER.set(f);
}

/// True when threads can be started: natively always, on wasm32 once the host has set a spawner and the module has
/// shared memory.
pub fn available() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        cfg!(target_feature = "atomics") && SPAWNER.get().is_some()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        true
    }
}

/// Start a thread running `f`.
///
/// # Panics
/// If [`available`] is false.
pub fn spawn(f: impl FnOnce() + Send + 'static) {
    match SPAWNER.get() {
        Some(s) => s(Box::new(f)),
        None => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                std::thread::Builder::new().name("rvp-worker".into()).spawn(f).expect("spawn a thread");
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = f;
                panic!("no thread spawner on this host");
            }
        }
    }
}

/// [`spawn`] for callers that need a plain function pointer (a hook taking `Box<dyn FnOnce() + Send>`).
pub fn spawn_boxed(f: Box<dyn FnOnce() + Send + 'static>) {
    spawn(f);
}

/// A good number of threads for this machine: `hardware` clamped to 2..=8 (the caller supplies the hardware count, as
/// the browser and `std` learn it differently).
pub fn pool_size(hardware: usize) -> usize {
    hardware.clamp(2, 8)
}
