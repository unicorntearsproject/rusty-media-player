//! Builders for the events a test scripts, with the byte layouts of the events page (draft v0.3).
use bucket_v0_sys::{Event, ev};

/// `KEY_DOWN`.
pub fn key_down(key: u32, mods: u32, repeat: bool) -> Event {
    let mut e = Event::new(ev::KEY_DOWN, if repeat { ev::FLAG_REPEAT } else { 0 }, 0);
    e.put_u32(16, key);
    e.put_u32(20, mods);
    e
}

/// `KEY_UP`.
pub fn key_up(key: u32, mods: u32) -> Event {
    let mut e = key_down(key, mods, false);
    e.kind = ev::KEY_UP;
    e
}

/// `POINTER_MOVE`.
pub fn pointer_move(x: f32, y: f32) -> Event {
    let mut e = Event::new(ev::POINTER_MOVE, 0, 0);
    e.put_f32(16, x);
    e.put_f32(20, y);
    e
}

/// `POINTER_DOWN` or `POINTER_UP`.
pub fn pointer_button(down: bool, x: f32, y: f32, button: u32) -> Event {
    let mut e = pointer_move(x, y);
    e.kind = if down { ev::POINTER_DOWN } else { ev::POINTER_UP };
    e.put_u32(24, button);
    e
}

/// `WHEEL`, in lines or (with `pixels`) pixels.
pub fn wheel(dx: f32, dy: f32, pixels: bool) -> Event {
    let mut e = Event::new(ev::WHEEL, if pixels { ev::FLAG_PIXELS } else { 0 }, 0);
    e.put_f32(16, dx);
    e.put_f32(20, dy);
    e
}

/// `RESIZE`.
pub fn resize(w: u32, h: u32, scale: f32, fullscreen: bool) -> Event {
    let mut e = Event::new(ev::RESIZE, if fullscreen { ev::FLAG_FULLSCREEN } else { 0 }, 0);
    e.put_u32(16, w);
    e.put_u32(20, h);
    e.put_f32(24, scale);
    e
}

/// An event with a single `u32` at 16 (`FOCUS`, `DRAG_OVER`, `VISIBILITY`, `MEMORY_PRESSURE`, `TERMINATE`, `CPU_COUNT_CHANGED`).
pub fn with_u32(kind: u16, v: u32) -> Event {
    let mut e = Event::new(kind, 0, 0);
    e.put_u32(16, v);
    e
}

/// An event with no payload (`SUSPEND`, `RELOAD`, `INTERRUPT`, `POINTER_LEAVE`, `WAKE`, `THEME_CHANGED`).
pub fn bare(kind: u16) -> Event {
    Event::new(kind, 0, 0)
}

/// `RESUME`.
pub fn resume(slept_us: i64) -> Event {
    let mut e = Event::new(ev::RESUME, 0, 0);
    e.put_i64(16, slept_us);
    e
}

/// `CAPS_CHANGED`.
pub fn caps_changed(caps: i64) -> Event {
    let mut e = Event::new(ev::CAPS_CHANGED, 0, 0);
    e.put_u64(16, caps as u64);
    e
}

/// `TRANSPORT` with an integer value (microseconds for the seeks).
pub fn transport(command: u32, value: i64) -> Event {
    let mut e = Event::new(ev::TRANSPORT, 0, 0);
    e.put_u32(16, command);
    e.put_i64(24, value);
    e
}

/// `TRANSPORT` with an `f32` value in the low four bytes (rate, volume).
pub fn transport_f32(command: u32, value: f32) -> Event {
    let mut e = Event::new(ev::TRANSPORT, 0, 0);
    e.put_u32(16, command);
    e.put_f32(24, value);
    e
}

/// `DROP`-style file event: a handle at 16 and the `more` follow flag (bit 0). For `OPEN`, whose flags differ, use [`open`].
pub fn file(kind: u16, handle: i32, more: bool) -> Event {
    let mut e = Event::new(kind, if more { ev::FLAG_MORE } else { 0 }, 0);
    e.put_i32(16, handle);
    e
}

/// `OPEN`: a handle at 16; flag bit 0 while the app is running, bit 1 more of the same group follow.
pub fn open(handle: i32, running: bool, more: bool) -> Event {
    let flags = if running { ev::FLAG_WHILE_RUNNING } else { 0 } | if more { ev::FLAG_OPEN_MORE } else { 0 };
    let mut e = Event::new(ev::OPEN, flags, 0);
    e.put_i32(16, handle);
    e
}

/// `FILE_PICKED`: the request and a handle (or `-CANCELLED`).
pub fn file_picked(request: i32, handle: i32, more: bool) -> Event {
    let mut e = Event::new(ev::FILE_PICKED, if more { ev::FLAG_MORE } else { 0 }, 0);
    e.put_i32(16, request);
    e.put_i32(20, handle);
    e
}

/// `FOLDER_ADDED` with a cancelled result (no text).
pub fn folder_cancelled(request: i32) -> Event {
    let mut e = Event::new(ev::FOLDER_ADDED, 0, 0);
    e.put_i32(16, request);
    e.put_i32(20, bucket_v0_sys::err::CANCELLED);
    e
}

/// `FOLDER_ADDED` (the root's text handle is at 20; use `State::push_text(e, 20, root)`).
pub fn folder_added(request: i32) -> Event {
    let mut e = Event::new(ev::FOLDER_ADDED, 0, 0);
    e.put_i32(16, request);
    e
}

/// `LIBRARY_LISTING`, `LIBRARY_CHANGED` or `LIBRARY_PROGRESS` (the root's text handle is at 16; use `State::push_text(e, 16, root)`).
pub fn library(kind: u16, partial: bool) -> Event {
    Event::new(kind, if partial { ev::FLAG_MORE } else { 0 }, 0)
}

/// `IO_READY` for `file` (0 = retry every pending `kv_load` and `file_open_id`).
pub fn io_ready(file: i32) -> Event {
    let mut e = Event::new(ev::IO_READY, 0, 0);
    e.put_i32(16, file);
    e
}

/// `FILE_SAVED`.
pub fn file_saved(request: i32, status: i32) -> Event {
    let mut e = Event::new(ev::FILE_SAVED, 0, 0);
    e.put_i32(16, request);
    e.put_i32(20, status);
    e
}

/// `AUDIO_DEVICE_CHANGED`.
pub fn audio_device_changed(stream: i32, rate: u32, channels: u32) -> Event {
    let mut e = Event::new(ev::AUDIO_DEVICE_CHANGED, 0, 0);
    e.put_i32(16, stream);
    e.put_u32(20, rate);
    e.put_u32(24, channels);
    e
}

/// `AUDIO_ERROR`.
pub fn audio_error(stream: i32, error: i32) -> Event {
    let mut e = Event::new(ev::AUDIO_ERROR, 0, 0);
    e.put_i32(16, stream);
    e.put_i32(20, error);
    e
}

/// `FRAME`: `time_us` is the vblank just passed and `vblank_us` (at 16) the next one, the one to target.
pub fn frame(time_us: i64, vblank_us: i64) -> Event {
    let mut e = Event::new(ev::FRAME, 0, time_us);
    e.put_i64(16, vblank_us);
    e
}
