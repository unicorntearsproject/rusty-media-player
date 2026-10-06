//! The mock host keeps its promises: virtual time, event delivery, text handles, `-UNSUPPORTED` for what it does not do.
use bucket_v0_mock::{MockHost, events};
use bucket_v0_sys::{self as sys, EVENT_SIZE, Event, backend, err, ev};

fn wait(timeout: i64, max: usize) -> Vec<Event> {
    let mut raw = vec![0u8; max * EVENT_SIZE];
    // SAFETY: the buffer holds `max` records.
    let n = unsafe { sys::events_wait(raw.as_mut_ptr(), max as i32, timeout) };
    raw.chunks_exact(EVENT_SIZE).take(n as usize).map(|c| Event::from_bytes(c.try_into().unwrap())).collect()
}

fn now() -> i64 {
    // SAFETY: no arguments.
    unsafe { sys::time_now_us() }
}

#[test]
fn time_only_moves_when_the_app_waits() {
    let mock = MockHost::new();
    let _g = mock.install();
    let t0 = now();
    assert_eq!(now(), t0);
    assert!(wait(0, 4).is_empty());
    assert_eq!(now(), t0, "a poll does not sleep");
    assert!(wait(5_000, 4).is_empty());
    assert_eq!(now(), t0 + 5_000, "a wait that times out sleeps the whole timeout");
    // A scheduled event ends the sleep at its time, and events come out in time order.
    mock.with(|s| {
        s.push_after(9_000, events::bare(ev::SUSPEND));
        s.push_after(2_000, events::bare(ev::RELOAD));
    });
    let got = wait(100_000, 4);
    assert_eq!(got.iter().map(|e| e.kind).collect::<Vec<_>>(), vec![ev::RELOAD]);
    assert_eq!(now(), t0 + 5_000 + 2_000);
    assert_eq!(got[0].time_us, now());
    let got = wait(100_000, 4);
    assert_eq!(got[0].kind, ev::SUSPEND);
    assert_eq!(now(), t0 + 5_000 + 9_000);
    // `max` is honoured and the rest waits for the next call.
    mock.with(|s| (0..5).for_each(|i| s.push(events::with_u32(ev::FOCUS, i))));
    assert_eq!(wait(0, 3).len(), 3);
    assert_eq!(wait(0, 3).len(), 2);
}

#[test]
fn text_handles_live_until_the_next_wait() {
    let mock = MockHost::new();
    let _g = mock.install();
    mock.with(|s| s.push_text(events::folder_added(7), 20, "dir:Músic"));
    let got = wait(0, 4);
    let handle = got[0].i32_at(20);
    assert!(handle > 0);
    let mut buf = [0u8; 32];
    // SAFETY: the buffer is 32 bytes.
    let n = unsafe { sys::event_text(handle, buf.as_mut_ptr(), 32) };
    assert_eq!(&buf[..n as usize], "dir:Músic".as_bytes());
    // `cap = 0` gives the length; a short buffer gets a prefix and the full length.
    assert_eq!(unsafe { sys::event_text(handle, std::ptr::null_mut(), 0) }, n);
    let mut small = [0u8; 3];
    assert_eq!(unsafe { sys::event_text(handle, small.as_mut_ptr(), 3) }, n);
    assert_eq!(&small, b"dir");
    wait(0, 4);
    assert_eq!(
        unsafe { sys::event_text(handle, buf.as_mut_ptr(), 32) },
        err::NOT_FOUND,
        "stale after the next wait"
    );
}

#[test]
fn wakes_coalesce_and_missing_functions_answer_unsupported() {
    let mock = MockHost::new();
    let _g = mock.install();
    // SAFETY: no arguments.
    unsafe {
        sys::events_wake();
        sys::events_wake();
    }
    let got = wait(0, 4);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].kind, ev::WAKE);
    // A function the mock does not implement behaves like a host that predates it.
    // SAFETY: valid pointers and lengths.
    unsafe {
        assert_eq!(sys::bar_command(b"x".as_ptr(), 1, b"y".as_ptr(), 1), err::UNSUPPORTED);
        assert_eq!(sys::theme_get(std::ptr::null_mut(), 0), err::UNSUPPORTED);
        assert_eq!(sys::api_version(), 3);
    }
}

#[test]
#[should_panic(expected = "no bucket_v0 backend")]
fn calls_without_a_host_say_so() {
    // SAFETY: no arguments.
    unsafe { sys::time_now_us() };
}

#[test]
fn the_previous_backend_comes_back_when_a_guard_drops() {
    let a = MockHost::new();
    let b = MockHost::new();
    b.with(|s| s.now_us = 42);
    let _ga = a.install();
    {
        let _gb = b.install();
        assert_eq!(now(), 42);
        assert!(backend::current().is_some());
    }
    assert_ne!(now(), 42);
}

#[test]
fn pointers_in_structs_are_tokens_that_resolve_during_the_call_only() {
    let t = sys::ptr32(b"abc".as_ptr());
    assert!(t != 0);
    assert_eq!(sys::ptr32(std::ptr::null::<u8>()), 0);
    assert_eq!(backend::deref32(0), std::ptr::null());
}
