//! Thin, safe helpers over the raw imports: the size-then-fill pattern, logging, limits.
use bucket_v0_sys as sys;

/// Monotonic microseconds from the OS (`time_now_us`).
pub fn now_us() -> i64 {
    // SAFETY: no arguments.
    unsafe { sys::time_now_us() }
}

/// The capability bits right now.
pub fn caps() -> i64 {
    // SAFETY: no arguments.
    unsafe { sys::caps() }
}

/// A limit, or `default` when the host does not know it.
pub fn limit(which: i32, default: i64) -> i64 {
    // SAFETY: no pointers.
    let v = unsafe { sys::limit_get(which) };
    if v > 0 { v } else { default }
}

/// Send a line to the OS log.
pub fn log(level: i32, msg: &str) {
    // SAFETY: the range is the string's.
    unsafe { sys::log(level, msg.as_ptr(), msg.len().min(i32::MAX as usize) as i32) }
}

/// Log an error.
pub fn error(msg: &str) {
    log(sys::log_level::ERROR, msg);
}

/// Log a warning.
pub fn warn(msg: &str) {
    log(sys::log_level::WARN, msg);
}

/// Log information.
pub fn info(msg: &str) {
    log(sys::log_level::INFO, msg);
}

/// The name of an error code, for messages.
pub fn code_name(code: i32) -> &'static str {
    match code {
        sys::err::INVALID => "INVALID",
        sys::err::NOT_FOUND => "NOT_FOUND",
        sys::err::DENIED => "DENIED",
        sys::err::NO_SPACE => "NO_SPACE",
        sys::err::TOO_LARGE => "TOO_LARGE",
        sys::err::UNSUPPORTED => "UNSUPPORTED",
        sys::err::BUSY => "BUSY",
        sys::err::IO => "IO",
        sys::err::CLOSED => "CLOSED",
        sys::err::CANCELLED => "CANCELLED",
        sys::err::NO_DEVICE => "NO_DEVICE",
        sys::err::TIMEOUT => "TIMEOUT",
        _ => "an unknown error",
    }
}

/// Fill a buffer with a host-to-app call that returns the full length: ask for the length (`cap = 0`), then for the bytes; if the
/// value grew in between, ask again. A negative result is the error code.
pub fn bytes_call(mut f: impl FnMut(*mut u8, i32) -> i32) -> Result<Vec<u8>, i32> {
    for _ in 0..4 {
        let len = f(std::ptr::null_mut(), 0);
        if len < 0 {
            return Err(len);
        }
        let mut buf = vec![0u8; len as usize];
        let got = f(buf.as_mut_ptr(), len);
        if got < 0 {
            return Err(got);
        }
        if got <= len {
            buf.truncate(got as usize);
            return Ok(buf);
        }
    }
    Err(sys::err::BUSY)
}

/// [`bytes_call`] for UTF-8 text (invalid bytes become U+FFFD).
pub fn text_call(f: impl FnMut(*mut u8, i32) -> i32) -> Result<String, i32> {
    bytes_call(f).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The text attached to an event (valid until the next `events_wait`).
pub fn event_text(handle: i32) -> Option<String> {
    if handle <= 0 {
        return None;
    }
    // SAFETY: `bytes_call` passes a buffer of the size it announces.
    text_call(|p, cap| unsafe { sys::event_text(handle, p, cap) }).ok()
}

/// The `len` argument of a call for a slice, saturated to what an `i32` holds.
pub fn len32(n: usize) -> i32 {
    n.min(i32::MAX as usize) as i32
}
