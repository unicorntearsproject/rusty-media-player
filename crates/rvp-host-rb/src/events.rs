//! Events: the 64-byte records of `events_wait` decoded into an owned enum, so text handles (which die at the next wait) are
//! read at once and an event can wait in a queue.
use crate::api;
use bucket_v0_sys::{self as sys, Event, ev};

/// One event, decoded.
#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    /// A key went down or up.
    Key {
        /// Down (else up).
        down: bool,
        /// Key code (`bucket_v0_sys::key`).
        key: u32,
        /// Modifier bits.
        mods: u32,
        /// Auto-repeat.
        repeat: bool,
    },
    /// The pointer moved.
    PointerMove {
        /// X, physical pixels.
        x: f32,
        /// Y.
        y: f32,
    },
    /// A button went down or up.
    Pointer {
        /// Down (else up).
        down: bool,
        /// X.
        x: f32,
        /// Y.
        y: f32,
        /// Button number.
        button: u32,
    },
    /// The pointer left the canvas.
    PointerLeave,
    /// Wheel.
    Wheel {
        /// Horizontal, positive right.
        dx: f32,
        /// Vertical, positive down.
        dy: f32,
        /// Pixels (else lines).
        pixels: bool,
    },
    /// The canvas changed size, scale or fullscreen state.
    Resize {
        /// Width.
        w: u32,
        /// Height.
        h: u32,
        /// Scale.
        scale: f32,
        /// Fullscreen.
        fullscreen: bool,
    },
    /// Focus changed.
    Focus(bool),
    /// A file was dropped (an open handle the app owns).
    Drop {
        /// The handle.
        file: i32,
        /// More files follow in this drop.
        more: bool,
    },
    /// Something is dragged over the window (or left).
    DragOver(bool),
    /// A display refresh (not used: `frame_request` is never turned on).
    Frame {
        /// The next vblank.
        vblank_us: i64,
    },
    /// Visibility state (0 visible, 1 occluded, 2 minimized).
    Visibility(u32),
    /// Composed text (IME).
    Text(String),
    /// Save and return.
    Terminate {
        /// Time the OS allows.
        grace_ms: u32,
    },
    /// Hidden or frozen soon.
    Suspend,
    /// Back; how long the app slept.
    Resume {
        /// Slept microseconds.
        slept_us: i64,
    },
    /// A hot reload is coming.
    Reload,
    /// An interrupt.
    Interrupt,
    /// Memory is short (1 soon, 2 now).
    MemoryPressure(u32),
    /// The capability bits changed.
    CapsChanged(i64),
    /// The cores available changed.
    CpuCount(u32),
    /// Theme roles changed.
    ThemeChanged,
    /// A Bucket Bar command ran.
    Command {
        /// Command id.
        command: i32,
        /// Argument text handle (already read: `arg`).
        arg: Option<String>,
    },
    /// A transport command from the shell.
    Transport {
        /// `bucket_v0_sys::transport` value.
        command: u32,
        /// The value as microseconds.
        value_us: i64,
        /// The value as `f32` (low four bytes).
        value_f32: f32,
    },
    /// A picked file (or a cancel: a negative `file`).
    FilePicked {
        /// Request id.
        request: i32,
        /// Handle, or `-CANCELLED`.
        file: i32,
        /// More files follow.
        more: bool,
    },
    /// A folder joined the library (or a cancel).
    FolderAdded {
        /// Request id.
        request: i32,
        /// The root id, or the error code.
        root: Result<String, i32>,
    },
    /// A listing (or part of one) is ready.
    LibraryListing {
        /// Root id.
        root: String,
        /// More follows (appends).
        partial: bool,
    },
    /// A retried read will succeed (0: retry every pending `kv_load` and `file_open_id`).
    IoReady {
        /// File handle, or 0.
        file: i32,
    },
    /// A file the app was opened with.
    Open {
        /// The handle.
        file: i32,
        /// The app was already running.
        while_running: bool,
        /// More files of the same group follow (flag bit 1; older hosts never set it).
        more: bool,
    },
    /// A save finished.
    FileSaved {
        /// Request id.
        request: i32,
        /// 0 or an error code.
        status: i32,
    },
    /// Walk progress.
    LibraryProgress {
        /// Root id.
        root: String,
        /// Files seen so far.
        files_seen: u64,
    },
    /// Made by `events_wake`.
    Wake,
    /// The default output changed (the stream handle survives).
    AudioDeviceChanged {
        /// Stream.
        stream: i32,
        /// New native rate.
        rate: u32,
        /// New native channel count.
        channels: u32,
    },
    /// An audio stream failed.
    AudioError {
        /// Stream.
        stream: i32,
        /// Error code.
        error: i32,
    },
    /// Files changed on disk under a root.
    LibraryChanged {
        /// Root id.
        root: String,
    },
    /// A kind this version does not know (ignored).
    Unknown(u16),
}

/// Decode one record. Text is read now.
pub fn decode(e: &Event) -> Ev {
    let text = |at| api::event_text(e.i32_at(at));
    let root = |at| text(at).unwrap_or_default();
    match e.kind {
        ev::KEY_DOWN | ev::KEY_UP => Ev::Key {
            down: e.kind == ev::KEY_DOWN,
            key: e.u32_at(16),
            mods: e.u32_at(20),
            repeat: e.flags & ev::FLAG_REPEAT != 0,
        },
        ev::POINTER_MOVE => Ev::PointerMove { x: e.f32_at(16), y: e.f32_at(20) },
        ev::POINTER_DOWN | ev::POINTER_UP => Ev::Pointer {
            down: e.kind == ev::POINTER_DOWN,
            x: e.f32_at(16),
            y: e.f32_at(20),
            button: e.u32_at(24),
        },
        ev::POINTER_LEAVE => Ev::PointerLeave,
        ev::WHEEL => Ev::Wheel { dx: e.f32_at(16), dy: e.f32_at(20), pixels: e.flags & ev::FLAG_PIXELS != 0 },
        ev::RESIZE => Ev::Resize {
            w: e.u32_at(16),
            h: e.u32_at(20),
            scale: e.f32_at(24),
            fullscreen: e.flags & ev::FLAG_FULLSCREEN != 0,
        },
        ev::FOCUS => Ev::Focus(e.u32_at(16) != 0),
        ev::DROP => Ev::Drop { file: e.i32_at(16), more: e.flags & ev::FLAG_MORE != 0 },
        ev::DRAG_OVER => Ev::DragOver(e.u32_at(16) != 0),
        ev::FRAME => Ev::Frame { vblank_us: e.i64_at(16) },
        ev::VISIBILITY => Ev::Visibility(e.u32_at(16)),
        ev::TEXT => Ev::Text(text(16).unwrap_or_default()),
        ev::TERMINATE => Ev::Terminate { grace_ms: e.u32_at(16) },
        ev::SUSPEND => Ev::Suspend,
        ev::RESUME => Ev::Resume { slept_us: e.i64_at(16) },
        ev::RELOAD => Ev::Reload,
        ev::INTERRUPT => Ev::Interrupt,
        ev::MEMORY_PRESSURE => Ev::MemoryPressure(e.u32_at(16)),
        ev::CAPS_CHANGED => Ev::CapsChanged(e.u64_at(16) as i64),
        ev::CPU_COUNT_CHANGED => Ev::CpuCount(e.u32_at(16)),
        ev::THEME_CHANGED => Ev::ThemeChanged,
        ev::COMMAND => Ev::Command { command: e.i32_at(16), arg: text(20) },
        ev::TRANSPORT => {
            Ev::Transport { command: e.u32_at(16), value_us: e.i64_at(24), value_f32: e.f32_at(24) }
        }
        ev::FILE_PICKED => {
            Ev::FilePicked { request: e.i32_at(16), file: e.i32_at(20), more: e.flags & ev::FLAG_MORE != 0 }
        }
        ev::FOLDER_ADDED => {
            let h = e.i32_at(20);
            let root = if h > 0 {
                text(20).ok_or(sys::err::NOT_FOUND)
            } else {
                Err(if h == 0 { sys::err::INVALID } else { h })
            };
            Ev::FolderAdded { request: e.i32_at(16), root }
        }
        ev::LIBRARY_LISTING => Ev::LibraryListing { root: root(16), partial: e.flags & ev::FLAG_MORE != 0 },
        ev::IO_READY => Ev::IoReady { file: e.i32_at(16) },
        ev::OPEN => Ev::Open {
            file: e.i32_at(16),
            while_running: e.flags & ev::FLAG_WHILE_RUNNING != 0,
            more: e.flags & ev::FLAG_OPEN_MORE != 0,
        },
        ev::FILE_SAVED => Ev::FileSaved { request: e.i32_at(16), status: e.i32_at(20) },
        ev::LIBRARY_PROGRESS => Ev::LibraryProgress { root: root(16), files_seen: e.u64_at(24) },
        ev::WAKE => Ev::Wake,
        ev::AUDIO_DEVICE_CHANGED => {
            Ev::AudioDeviceChanged { stream: e.i32_at(16), rate: e.u32_at(20), channels: e.u32_at(24) }
        }
        ev::AUDIO_ERROR => Ev::AudioError { stream: e.i32_at(16), error: e.i32_at(20) },
        ev::LIBRARY_CHANGED => Ev::LibraryChanged { root: root(16) },
        other => Ev::Unknown(other),
    }
}

/// Wait for events (see `events_wait`): up to `timeout_us` (0 = poll, -1 = no timeout) and at most `max` records.
pub fn wait(timeout_us: i64, max: usize) -> Vec<Ev> {
    let max = max.clamp(1, 1024);
    let mut raw = vec![0u8; max * sys::EVENT_SIZE];
    // SAFETY: `raw` holds `max` records.
    let n = unsafe { sys::events_wait(raw.as_mut_ptr(), max as i32, timeout_us) };
    if n <= 0 {
        return Vec::new();
    }
    raw.chunks_exact(sys::EVENT_SIZE)
        .take(n as usize)
        .map(|c| {
            let mut b = [0u8; 64];
            b.copy_from_slice(c);
            decode(&Event::from_bytes(&b))
        })
        .collect()
}
