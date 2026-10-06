//! Constants of the App API: error codes, capability bits, event kinds, keys, limits and the small enums.

/// Error codes (negative return values). Numbering is append-only.
pub mod err {
    /// Bad argument, out-of-bounds pointer, or a handle of the wrong kind.
    pub const INVALID: i32 = -1;
    /// No such key, file, root, or handle.
    pub const NOT_FOUND: i32 = -2;
    /// No permission.
    pub const DENIED: i32 = -3;
    /// Storage or quota full (the old value stays in place).
    pub const NO_SPACE: i32 = -4;
    /// Over a size limit.
    pub const TOO_LARGE: i32 = -5;
    /// Not on this host or in this version, or not applicable.
    pub const UNSUPPORTED: i32 = -6;
    /// Not ready yet: retry after the matching event (for example `IO_READY`).
    pub const BUSY: i32 = -7;
    /// A device or file error.
    pub const IO: i32 = -8;
    /// The handle or stream has ended.
    pub const CLOSED: i32 = -9;
    /// The user cancelled (a picker, a save dialog).
    pub const CANCELLED: i32 = -10;
    /// No audio (or other) device, or it went away.
    pub const NO_DEVICE: i32 = -11;
    /// A wait ran out.
    pub const TIMEOUT: i32 = -12;
}

/// Capability bits of `caps()`.
pub mod caps {
    /// Shared memory and `thread_spawn` (the chosen build has threads).
    pub const THREADS: i64 = 1 << 0;
    /// WebAssembly SIMD (the chosen build uses it).
    pub const SIMD128: i64 = 1 << 1;
    /// Atomics and `memory.atomic.wait`/`notify`.
    pub const ATOMICS: i64 = 1 << 2;
    /// A canvas to draw into.
    pub const CANVAS: i64 = 1 << 3;
    /// The video layer accepts YUV frames.
    pub const VIDEO_YUV: i64 = 1 << 4;
    /// Audio output.
    pub const AUDIO_OUT: i64 = 1 << 5;
    /// Microphone input.
    pub const AUDIO_IN: i64 = 1 << 6;
    /// The shell shows now-playing info and routes transport.
    pub const NOW_PLAYING: i64 = 1 << 7;
    /// The shell wants the visualizer feed right now.
    pub const VISUALIZER: i64 = 1 << 8;
    /// Folder access for a media library.
    pub const LIBRARY: i64 = 1 << 9;
    /// The app can add Bucket Bar commands.
    pub const BAR_COMMANDS: i64 = 1 << 10;
    /// Theme roles and `THEME_CHANGED`.
    pub const THEME: i64 = 1 << 11;
    /// `frame_request` and `FRAME` events (display pacing).
    pub const FRAME_EVENTS: i64 = 1 << 12;
    /// `clipboard_set_text`.
    pub const CLIPBOARD: i64 = 1 << 13;
}

/// Event kinds (`Event::kind`).
pub mod ev {
    /// `key u32` at 16, `mods u32` at 20. Flag bit 0: auto-repeat.
    pub const KEY_DOWN: u16 = 1;
    /// `key u32` at 16, `mods u32` at 20.
    pub const KEY_UP: u16 = 2;
    /// `x f32` at 16, `y f32` at 20.
    pub const POINTER_MOVE: u16 = 3;
    /// `x f32` at 16, `y f32` at 20, `button u32` at 24.
    pub const POINTER_DOWN: u16 = 4;
    /// `x f32` at 16, `y f32` at 20, `button u32` at 24.
    pub const POINTER_UP: u16 = 5;
    /// `dx f32` at 16, `dy f32` at 20. Flag bit 0: pixels (else lines).
    pub const WHEEL: u16 = 6;
    /// `w u32` at 16, `h u32` at 20, `scale f32` at 24. Flag bit 0: fullscreen.
    pub const RESIZE: u16 = 7;
    /// `focused u32` at 16.
    pub const FOCUS: u16 = 8;
    /// `file i32` at 16 (an open handle), `name i32` at 20 (text). Flag bit 0: more follow.
    pub const DROP: u16 = 9;
    /// `over u32` at 16.
    pub const DRAG_OVER: u16 = 10;
    /// `vblank_us i64` at 16: the next vblank.
    pub const FRAME: u16 = 11;
    /// `state u32` at 16 (0 visible, 1 occluded, 2 minimized).
    pub const VISIBILITY: u16 = 12;
    /// The pointer left the canvas.
    pub const POINTER_LEAVE: u16 = 13;
    /// `text i32` at 16: composed input (IME) only.
    pub const TEXT: u16 = 14;
    /// `grace_ms u32` at 16.
    pub const TERMINATE: u16 = 20;
    /// Hidden or frozen soon: stop drawing.
    pub const SUSPEND: u16 = 21;
    /// `slept_us i64` at 16.
    pub const RESUME: u16 = 22;
    /// A hot reload is coming.
    pub const RELOAD: u16 = 23;
    /// An interrupt (Ctrl-C style).
    pub const INTERRUPT: u16 = 24;
    /// `level u32` at 16 (1 soon, 2 now).
    pub const MEMORY_PRESSURE: u16 = 25;
    /// `caps u64` at 16: the new bitmask.
    pub const CAPS_CHANGED: u16 = 26;
    /// `count u32` at 16.
    pub const CPU_COUNT_CHANGED: u16 = 27;
    /// Theme roles changed.
    pub const THEME_CHANGED: u16 = 30;
    /// `command i32` at 16, `arg i32` at 20 (text, or 0).
    pub const COMMAND: u16 = 31;
    /// `command u32` at 16, `value` (`i64` microseconds, or `f32` in the low four bytes) at 24.
    pub const TRANSPORT: u16 = 32;
    /// `request i32` at 16, `file i32` at 20 (a handle, or `-CANCELLED`). Flag bit 0: more follow.
    pub const FILE_PICKED: u16 = 33;
    /// `request i32` at 16, `root i32` at 20 (text, or `-CANCELLED`).
    pub const FOLDER_ADDED: u16 = 34;
    /// `root i32` at 16 (text). Flag bit 0: partial, more follows (appends).
    pub const LIBRARY_LISTING: u16 = 35;
    /// `file i32` at 16: a retried read will now succeed; 0 means retry every pending `kv_load` and `file_open_id`.
    pub const IO_READY: u16 = 36;
    /// `file i32` at 16: a file the app was opened with. Flag bit 0: while running.
    pub const OPEN: u16 = 37;
    /// `request i32` at 16, `status i32` at 20 (0 or an error).
    pub const FILE_SAVED: u16 = 38;
    /// `root i32` at 16 (text), `files_seen u64` at 24.
    pub const LIBRARY_PROGRESS: u16 = 39;
    /// Made by `events_wake`.
    pub const WAKE: u16 = 40;
    /// `stream i32` at 16, `rate u32` at 20, `channels u32` at 24.
    pub const AUDIO_DEVICE_CHANGED: u16 = 41;
    /// `stream i32` at 16, `error i32` at 20.
    pub const AUDIO_ERROR: u16 = 42;
    /// `root i32` at 16 (text): files changed on disk.
    pub const LIBRARY_CHANGED: u16 = 43;

    /// Flag bit 0 of `KEY_DOWN`: auto-repeat.
    pub const FLAG_REPEAT: u16 = 1;
    /// Flag bit 0 of `WHEEL`: the deltas are pixels.
    pub const FLAG_PIXELS: u16 = 1;
    /// Flag bit 0 of `RESIZE`: fullscreen.
    pub const FLAG_FULLSCREEN: u16 = 1;
    /// Flag bit 0 of `DROP`, `FILE_PICKED` and `LIBRARY_LISTING`: more follow.
    pub const FLAG_MORE: u16 = 1;
    /// Flag bit 0 of `OPEN`: the app was already running.
    pub const FLAG_WHILE_RUNNING: u16 = 1;
}

/// Keys: a Unicode scalar value for keys that type a character (Space is U+0020), or `NAMED_BASE + n`.
pub mod key {
    /// First named key.
    pub const NAMED_BASE: u32 = 0x0011_0000;
    /// Enter.
    pub const ENTER: u32 = NAMED_BASE;
    /// Escape.
    pub const ESCAPE: u32 = NAMED_BASE + 1;
    /// Tab.
    pub const TAB: u32 = NAMED_BASE + 2;
    /// Backspace.
    pub const BACKSPACE: u32 = NAMED_BASE + 3;
    /// Delete.
    pub const DELETE: u32 = NAMED_BASE + 4;
    /// Insert.
    pub const INSERT: u32 = NAMED_BASE + 5;
    /// Left arrow.
    pub const LEFT: u32 = NAMED_BASE + 6;
    /// Right arrow.
    pub const RIGHT: u32 = NAMED_BASE + 7;
    /// Up arrow.
    pub const UP: u32 = NAMED_BASE + 8;
    /// Down arrow.
    pub const DOWN: u32 = NAMED_BASE + 9;
    /// Home.
    pub const HOME: u32 = NAMED_BASE + 10;
    /// End.
    pub const END: u32 = NAMED_BASE + 11;
    /// Page up.
    pub const PAGE_UP: u32 = NAMED_BASE + 12;
    /// Page down.
    pub const PAGE_DOWN: u32 = NAMED_BASE + 13;
    /// Caps lock.
    pub const CAPS_LOCK: u32 = NAMED_BASE + 15;
    /// Num lock.
    pub const NUM_LOCK: u32 = NAMED_BASE + 16;
    /// F1; F2 is `F1 + 1` and so on up to F24 (`NAMED_BASE + 43`).
    pub const F1: u32 = NAMED_BASE + 20;
    /// F24.
    pub const F24: u32 = NAMED_BASE + 43;
    /// Menu (context menu).
    pub const MENU: u32 = NAMED_BASE + 50;
    /// Media play/pause.
    pub const MEDIA_PLAY_PAUSE: u32 = NAMED_BASE + 60;
    /// Media next.
    pub const MEDIA_NEXT: u32 = NAMED_BASE + 61;
    /// Media previous.
    pub const MEDIA_PREV: u32 = NAMED_BASE + 62;
    /// Media stop.
    pub const MEDIA_STOP: u32 = NAMED_BASE + 63;
    /// Volume up.
    pub const VOLUME_UP: u32 = NAMED_BASE + 64;
    /// Volume down.
    pub const VOLUME_DOWN: u32 = NAMED_BASE + 65;
    /// Volume mute.
    pub const VOLUME_MUTE: u32 = NAMED_BASE + 66;
    /// Browser back.
    pub const BROWSER_BACK: u32 = NAMED_BASE + 67;
    /// Browser forward.
    pub const BROWSER_FORWARD: u32 = NAMED_BASE + 68;
}

/// `mods` bits of key events.
pub mod mods {
    /// Shift.
    pub const SHIFT: u32 = 1;
    /// Control.
    pub const CTRL: u32 = 2;
    /// Alt.
    pub const ALT: u32 = 4;
    /// Super.
    pub const SUPER: u32 = 8;
}

/// Pointer `button` values.
pub mod button {
    /// Primary.
    pub const PRIMARY: u32 = 0;
    /// Secondary.
    pub const SECONDARY: u32 = 1;
    /// Middle.
    pub const MIDDLE: u32 = 2;
    /// Back.
    pub const BACK: u32 = 3;
    /// Forward.
    pub const FORWARD: u32 = 4;
}

/// One wheel line in pixels at scale 1.
pub const WHEEL_LINE_PX: f32 = 40.0;

/// `limit_get(which)` selectors.
pub mod limit {
    /// Max key length (bytes), at least 255.
    pub const KEY_LEN: i32 = 0;
    /// Max value size (bytes), at least 64 MiB.
    pub const VALUE_SIZE: i32 = 1;
    /// Storage quota for this app (bytes), at least 512 MiB.
    pub const QUOTA: i32 = 2;
    /// Max open handles, at least 256.
    pub const HANDLES: i32 = 3;
    /// Max events per `events_wait`, at least 256.
    pub const EVENTS_PER_WAIT: i32 = 4;
    /// Max `file_read_at` per call (bytes), at least 1 MiB.
    pub const READ_PER_CALL: i32 = 5;
    /// Max now-playing art (bytes), at least 8 MiB.
    pub const ART_BYTES: i32 = 6;
    /// Max now-playing string (bytes), at least 4 KiB.
    pub const STRING_BYTES: i32 = 7;
    /// Max threads per app, at least 64.
    pub const THREADS: i32 = 8;
}

/// `TRANSPORT.command` values.
pub mod transport {
    /// Play.
    pub const PLAY: u32 = 0;
    /// Pause.
    pub const PAUSE: u32 = 1;
    /// Toggle.
    pub const TOGGLE: u32 = 2;
    /// Stop.
    pub const STOP: u32 = 3;
    /// Next.
    pub const NEXT: u32 = 4;
    /// Previous.
    pub const PREV: u32 = 5;
    /// Seek to `value` microseconds.
    pub const SEEK_TO: u32 = 6;
    /// Seek by `value` microseconds (negative = back).
    pub const SEEK_BY: u32 = 7;
    /// Set the rate (`f32`).
    pub const SET_RATE: u32 = 8;
    /// Set the volume (`f32`, 0..=1).
    pub const SET_VOLUME: u32 = 9;
}

/// `launch_reason()` values.
pub mod launch {
    /// A normal start.
    pub const NORMAL: i32 = 0;
    /// After a hot reload.
    pub const HOT_RELOAD: i32 = 1;
    /// Restarted after a trap.
    pub const AFTER_TRAP: i32 = 2;
    /// Restarted after a kill or freeze timeout.
    pub const AFTER_KILL: i32 = 3;
}

/// `power_inhibit` kinds.
pub mod power {
    /// Keep the display on.
    pub const DISPLAY: i32 = 1;
    /// Keep the system awake.
    pub const SYSTEM: i32 = 2;
}

/// `cursor_set` kinds.
pub mod cursor {
    /// Default arrow.
    pub const DEFAULT: i32 = 0;
    /// Pointer (hand).
    pub const POINTER: i32 = 1;
    /// Text.
    pub const TEXT: i32 = 2;
    /// Grab.
    pub const GRAB: i32 = 3;
    /// Grabbing.
    pub const GRABBING: i32 = 4;
    /// East-west resize.
    pub const EW_RESIZE: i32 = 5;
    /// North-south resize.
    pub const NS_RESIZE: i32 = 6;
    /// Hidden.
    pub const NONE: i32 = 7;
}

/// `now_playing_playback.state` values.
pub mod play_state {
    /// Stopped.
    pub const STOPPED: u32 = 0;
    /// Playing.
    pub const PLAYING: u32 = 1;
    /// Paused.
    pub const PAUSED: u32 = 2;
}

/// `now_playing_playback.flags` bits.
pub mod playback_flags {
    /// There is a next item.
    pub const CAN_NEXT: u32 = 1;
    /// There is a previous item.
    pub const CAN_PREV: u32 = 2;
    /// Seeking works.
    pub const CAN_SEEK: u32 = 4;
}

/// `kv_store` flags.
pub mod kv {
    /// Cache class: the OS may evict the value under space pressure.
    pub const CACHE: i32 = 1;
}

/// `file_pick` flags.
pub mod pick {
    /// Multi-select.
    pub const MULTI: i32 = 1;
    /// Pick a folder.
    pub const FOLDER: i32 = 2;
}

/// `log` levels.
pub mod log_level {
    /// Error.
    pub const ERROR: i32 = 0;
    /// Warning.
    pub const WARN: i32 = 1;
    /// Information.
    pub const INFO: i32 = 2;
    /// Debug.
    pub const DEBUG: i32 = 3;
    /// Trace.
    pub const TRACE: i32 = 4;
}

/// `video_present` field values.
pub mod video {
    /// YUV 4:2:0, 8 bit.
    pub const FORMAT_YUV420_8: u32 = 0;
    /// YUV 4:2:0, 10 bit (little-endian `u16`, value in the low 10 bits).
    pub const FORMAT_YUV420_10: u32 = 1;
    /// BT.601.
    pub const MATRIX_BT601: u32 = 0;
    /// BT.709.
    pub const MATRIX_BT709: u32 = 1;
    /// BT.2020.
    pub const MATRIX_BT2020: u32 = 2;
    /// Limited range.
    pub const RANGE_LIMITED: u32 = 0;
    /// Full range.
    pub const RANGE_FULL: u32 = 1;
    /// `flags` bit 8: scale with the nearest-neighbour filter (default bilinear).
    pub const FLAG_NEAREST: u32 = 1 << 8;
}

/// Canvas info `flags` bits.
pub mod canvas_flags {
    /// The canvas is fullscreen.
    pub const FULLSCREEN: u32 = 1;
    /// The canvas is visible.
    pub const VISIBLE: u32 = 2;
    /// The display is HDR.
    pub const HDR: u32 = 4;
}

/// `VISIBILITY.state` values.
pub mod visibility {
    /// Visible.
    pub const VISIBLE: u32 = 0;
    /// Occluded.
    pub const OCCLUDED: u32 = 1;
    /// Minimized.
    pub const MINIMIZED: u32 = 2;
}
