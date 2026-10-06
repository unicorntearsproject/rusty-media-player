//! The imports of module `bucket_v0`, declared once.
//!
//! The same list produces the `extern "C"` block (wasm32), the [`FUNCTIONS`] table (names and WebAssembly signatures) and, on
//! native targets with the `backend` feature, the [`Backend`](crate::backend::Backend) trait and the free functions that call it.

/// A WebAssembly value type, as the import section writes it.
pub trait WasmType {
    /// `"i32"`, `"i64"`, `"f32"`, `"f64"`, or `""` for no value.
    const WASM: &'static str;
}
impl WasmType for i32 {
    const WASM: &'static str = "i32";
}
impl WasmType for i64 {
    const WASM: &'static str = "i64";
}
impl WasmType for f32 {
    const WASM: &'static str = "f32";
}
impl WasmType for () {
    const WASM: &'static str = "";
}
impl<T> WasmType for *const T {
    const WASM: &'static str = "i32";
}
impl<T> WasmType for *mut T {
    const WASM: &'static str = "i32";
}

/// What an unimplemented function of a [`Backend`](crate::backend::Backend) returns: `-UNSUPPORTED`, as an older host does.
pub trait Unsupported {
    /// The value.
    const VALUE: Self;
}
impl Unsupported for i32 {
    const VALUE: i32 = crate::err::UNSUPPORTED;
}
impl Unsupported for i64 {
    const VALUE: i64 = crate::err::UNSUPPORTED as i64;
}
impl Unsupported for () {
    const VALUE: () = ();
}

/// One imported function: its name and WebAssembly signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnSig {
    /// Function name in module `bucket_v0`.
    pub name: &'static str,
    /// Parameter types (`"i32"`, `"i64"`, `"f32"`).
    pub params: &'static [&'static str],
    /// Result type, or `""` for none.
    pub result: &'static str,
}

macro_rules! bucket_functions {
    ($( $(#[$meta:meta])* fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty; )*) => {
        #[cfg(target_arch = "wasm32")]
        #[link(wasm_import_module = "bucket_v0")]
        unsafe extern "C" {
            $( $(#[$meta])* pub fn $name($($arg: $ty),*) -> $ret; )*
        }

        /// The address of every import, so that a module that calls this imports the whole documented set (the module's link
        /// requirements are then exactly the API, whatever the optimiser drops). Call it behind a condition the compiler cannot
        /// fold, never in a hot path.
        #[cfg(target_arch = "wasm32")]
        pub fn link_all() -> usize {
            let mut x = 0usize;
            $( x ^= $name as *const () as usize; )*
            x
        }

        /// Every function of the App API (draft v0.3), with its WebAssembly signature, in the order of the API pages.
        pub const FUNCTIONS: &[FnSig] = &[
            $( FnSig { name: stringify!($name), params: &[$(<$ty as WasmType>::WASM),*], result: <$ret as WasmType>::WASM } ),*
        ];

        #[cfg(all(not(target_arch = "wasm32"), feature = "backend"))]
        pub use self::dispatch::*;

        /// The functions as a trait, for native targets: a mock host or a simulator implements the ones it supports; the rest
        /// answer `-UNSUPPORTED` like a host that predates them.
        #[cfg(all(not(target_arch = "wasm32"), feature = "backend"))]
        pub trait Backend: Send + Sync {
            $(
                $(#[$meta])*
                #[allow(unused_variables, clippy::missing_safety_doc)]
                unsafe fn $name(&self, $($arg: $ty),*) -> $ret {
                    <$ret as Unsupported>::VALUE
                }
            )*
        }

        #[cfg(all(not(target_arch = "wasm32"), feature = "backend"))]
        pub(crate) mod dispatch {

            $(
                $(#[$meta])*
                #[allow(clippy::missing_safety_doc)]
                pub unsafe fn $name($($arg: $ty),*) -> $ret {
                    let backend = crate::backend::current_or_panic();
                    // SAFETY: the caller upholds the function's contract, as for the real import.
                    let r = unsafe { backend.$name($($arg),*) };
                    crate::backend::end_call();
                    r
                }
            )*
        }
    };
}

bucket_functions! {
    // ---- System ----
    /// Minor revision of `bucket_v0`.
    fn api_version() -> i32;
    /// Capability bits.
    fn caps() -> i64;
    /// Why the app started (see `launch`).
    fn launch_reason() -> i32;
    /// A clean relaunch, without a trap.
    fn restart() -> i32;
    /// Ends every thread of the app at once.
    fn exit(status: i32) -> ();
    /// Log a line (any thread).
    fn log(level: i32, ptr: *const u8, len: i32) -> ();
    /// Cryptographically strong random bytes.
    fn random_fill(ptr: *mut u8, len: i32) -> i32;
    /// Monotonic microseconds (any thread).
    fn time_now_us() -> i64;
    /// Cores the app may use right now.
    fn cpu_count() -> i32;
    /// A limit (see `limit`).
    fn limit_get(which: i32) -> i64;
    /// 1 = keep the display on, 2 = keep the system awake.
    fn power_inhibit(kind: i32, on: i32) -> i32;
    /// Put text on the clipboard (needs `CLIPBOARD`).
    fn clipboard_set_text(ptr: *const u8, len: i32) -> i32;
    // ---- Threads ----
    /// Start a thread that runs the export `bucket_thread_start(tid, arg)`; returns the thread id.
    fn thread_spawn(arg: i32) -> i32;
    /// Give up the rest of the time slice.
    fn thread_yield() -> ();
    /// A scheduling hint for a thread: 0 normal, 1 background.
    fn thread_priority(tid: i32, level: i32) -> i32;
    // ---- Events ----
    /// Sleep until an event or the timeout (microseconds, 0 = poll, -1 = forever); returns the number of 64-byte records.
    fn events_wait(buf: *mut u8, max: i32, timeout_us: i64) -> i32;
    /// Make the current or next `events_wait` return (any thread).
    fn events_wake() -> i32;
    /// Copy the text attached to an event (valid until the next `events_wait`).
    fn event_text(handle: i32, buf: *mut u8, cap: i32) -> i32;
    // ---- Canvas ----
    /// Write the canvas info struct.
    fn canvas_info(out: *mut u8) -> i32;
    /// Show a full-size RGBA8 buffer; only the dirty rectangle changed. Never waits for the display.
    fn canvas_present(rgba: *const u8, len: i32, x: i32, y: i32, w: i32, h: i32) -> i32;
    /// Ask for fullscreen.
    fn canvas_fullscreen(on: i32) -> i32;
    /// Start or stop `FRAME` events (needs `FRAME_EVENTS`).
    fn frame_request(on: i32) -> i32;
    /// Set the mouse cursor.
    fn cursor_set(kind: i32) -> i32;
    /// Keep pointer events coming from outside the canvas.
    fn pointer_capture(on: i32) -> i32;
    // ---- Theme and Bucket Bar ----
    /// Theme roles (needs `THEME`).
    fn theme_get(buf: *mut u8, cap: i32) -> i32;
    /// Add a Bucket Bar command (needs `BAR_COMMANDS`).
    fn bar_command(name: *const u8, name_len: i32, help: *const u8, help_len: i32) -> i32;
    // ---- Video ----
    /// Show a YUV frame in the video layer under the canvas (needs `VIDEO_YUV`).
    fn video_present(frame: *const u8) -> i32;
    // ---- Audio ----
    /// Open a stream (needs `AUDIO_OUT`); writes the audio-open struct.
    fn audio_open(rate: i32, channels: i32, out: *mut u8) -> i32;
    /// Queue interleaved `f32` frames; never blocks; returns the frames accepted.
    fn audio_write(h: i32, ptr: *const f32, frames: i32) -> i32;
    /// A coherent clock snapshot.
    fn audio_clock(h: i32, out: *mut u8) -> i32;
    /// Frames not yet handed to the device.
    fn audio_queued(h: i32) -> i32;
    /// Delay from hand-off to audible, microseconds.
    fn audio_latency_us(h: i32) -> i64;
    /// Drop queued audio; resets the clock counters.
    fn audio_flush(h: i32) -> i32;
    /// Pause or resume.
    fn audio_pause(h: i32, paused: i32) -> i32;
    /// Per-stream volume, 0..=1.
    fn audio_volume(h: i32, volume: f32) -> i32;
    /// Close the stream.
    fn audio_close(h: i32) -> i32;
    // ---- Now playing ----
    /// Report what is playing.
    fn now_playing_metadata(meta: *const u8) -> i32;
    /// Report the transport state.
    fn now_playing_playback(pb: *const u8) -> i32;
    /// Give up the active session.
    fn now_playing_clear() -> i32;
    // ---- Visualizer ----
    /// PCM just heard.
    fn viz_block(pts_us: i64, rate: i32, channels: i32, samples: *const f32, frames: i32) -> i32;
    /// One analysis summary.
    fn viz_summary(s: *const u8) -> i32;
    /// Several summaries back to back (the stride is element 0's `struct_size`).
    fn viz_summary_n(s: *const u8, count: i32) -> i32;
    // ---- Library ----
    /// Show a folder picker; returns a request id.
    fn library_add_folder() -> i32;
    /// One line per known root: `root_id TAB display_name TAB readable`.
    fn library_roots(buf: *mut u8, cap: i32) -> i32;
    /// Ask the user to make an unreadable root readable again.
    fn library_reconnect(root: *const u8, len: i32) -> i32;
    /// Start a new walk of a root.
    fn library_rescan(root: *const u8, len: i32) -> i32;
    /// Forget a root and revoke access.
    fn library_forget(root: *const u8, len: i32) -> i32;
    /// Copy listing bytes from `offset`; returns the total size so far.
    fn library_listing(root: *const u8, len: i32, offset: i64, buf: *mut u8, cap: i32) -> i64;
    /// Free a kept listing.
    fn library_listing_release(root: *const u8, len: i32) -> i32;
    // ---- Key-value store ----
    /// Load a value; returns its length.
    fn kv_load(key: *const u8, key_len: i32, buf: *mut u8, cap: i32) -> i32;
    /// Queue a value (empty deletes); flags bit 0 = cache class.
    fn kv_store(key: *const u8, key_len: i32, val: *const u8, val_len: i32, flags: i32) -> i32;
    /// Wait until every queued store is on disk.
    fn kv_flush() -> i32;
    // ---- Files ----
    /// Show the picker; returns a request id.
    fn file_pick(kinds: *const u8, kinds_len: i32, flags: i32) -> i32;
    /// A save-as dialog; returns a request id.
    fn file_save(name: *const u8, name_len: i32, mime: *const u8, mime_len: i32, data: *const u8, len: i32) -> i32;
    /// Reopen a file by its stable id.
    fn file_open_id(id: *const u8, len: i32) -> i32;
    /// Open a file in the same folder as `h`.
    fn file_open_sibling(h: i32, name: *const u8, name_len: i32) -> i32;
    /// Open a file inside the app's own `.bucket`.
    fn file_open_bundle(path: *const u8, len: i32) -> i32;
    /// Size in bytes, or `-UNSUPPORTED`.
    fn file_size(h: i32) -> i64;
    /// Non-blocking read: bytes now, 0 at the end, or `-BUSY`.
    fn file_read_at(h: i32, offset: i64, buf: *mut u8, cap: i32) -> i32;
    /// Start reading ahead.
    fn file_prefetch(h: i32, offset: i64, len: i32) -> i32;
    /// Display name.
    fn file_name(h: i32, buf: *mut u8, cap: i32) -> i32;
    /// A stable id that reopens the file after a restart.
    fn file_id(h: i32, buf: *mut u8, cap: i32) -> i32;
    /// Close a handle.
    fn file_close(h: i32) -> i32;
}
