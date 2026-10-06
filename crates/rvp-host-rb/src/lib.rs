//! Rusty Bucket host adapter: Rusty Wave's host traits (`rvp-host`) on top of the App API (`bucket_v0`, draft v0.3), through the
//! raw bindings of `bucket-v0-sys`.
//!
//! | `rvp-host` | App API |
//! | --- | --- |
//! | `HostClock` | `time_now_us`; `request_wake` becomes the timeout of `events_wait` |
//! | `Source` | `file_read_at` (non-blocking: `-BUSY` is `Pending`, `IO_READY` wakes the loop), `file_prefetch`, `file_size`, `file_name` |
//! | `AudioSink` | `audio_open`, `audio_write`, `audio_queued`, `audio_latency_us` (or `audio_clock`), `audio_flush`, `audio_pause`, `audio_volume` |
//! | `Surface` | `canvas_info`, `canvas_present` (plan A), `canvas_fullscreen` |
//! | `VideoSink` | plan A: the app composes the picture; `video` has the optional `video_present` accelerator |
//! | `InputEvents` | `events_wait` |
//! | `Storage` | `kv_load`, `kv_store` (cache class for thumbnails), `kv_flush` |
//! | `NowPlaying`, `VisualizerTap` | `now_playing_*`, `TRANSPORT` events, `viz_block`, `viz_summary_n` |
//! | `Library` | `library_*` with the listing cursor |
//!
//! Nothing here depends on the app's UI or core beyond the public host traits, and the application crates (`rvp-core`, `rvp-ui`,
//! `rvp-app`) know nothing about Rusty Bucket.
#![cfg_attr(all(target_arch = "wasm32", target_feature = "atomics"), feature(asm_experimental_arch))]
#![forbid(unsafe_op_in_unsafe_fn)]

// Only the threads build uses it; a build without atomics still links the crate so the dependency list is the same.
#[cfg(target_arch = "wasm32")]
use rvp_par as _;

pub mod api;
pub mod audio;
pub mod clock;
pub mod events;
pub mod host;
pub mod input;
pub mod library;
pub mod media;
pub mod player;
pub mod shared;
pub mod source;
pub mod storage;
pub mod surface;
pub mod threads;
pub mod video;

pub use host::RbHost;
pub use player::{OPEN_EXTENSIONS, RbPlayer};

/// Name of the host, shown in diagnostics.
pub const HOST_NAME: &str = "rusty-bucket";

/// What `bucket_save_state` does: nothing is kept in the module's memory (the queue, the position and the library are in the
/// key-value store), so it only makes sure the stores already queued are on disk, and reports no state.
pub fn save_state_hook() -> i32 {
    // SAFETY: no arguments.
    unsafe { bucket_v0_sys::kv_flush() };
    0
}

/// Make a panic leave a line in the OS log before the module traps (a panic is a trap on `wasm32-unknown-unknown`).
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| api::error(&format!("panic: {info}"))));
}
