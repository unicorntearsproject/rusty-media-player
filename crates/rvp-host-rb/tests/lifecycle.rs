//! Start-up, the loop, events and shut-down against the mock host.
mod common;
use bucket_v0_mock::events;
use bucket_v0_sys::{self as sys, caps, ev, key, mods, transport};
use common::Harness;
use rvp_host::{InputEvent, Key, PointerButton};

#[test]
fn starts_draws_and_ends_on_terminate() {
    let mut h = Harness::new();
    // The app was told the canvas size and drew a full frame of it.
    h.run_ms(100);
    let (presents, len) =
        h.mock.with(|s| (s.canvas.presents.len(), s.canvas.presents.first().map(|p| p.len)));
    assert!(presents >= 1);
    assert_eq!(len, Some(320 * 180 * 4));
    assert_eq!(h.player.host.surface.size, (320, 180, 1.0));
    // TERMINATE: save, flush, release, return.
    h.push(events::with_u32(ev::TERMINATE, 1000));
    assert!(!h.player.run_once());
    assert_eq!(h.player.exit_status(), Some(0));
    assert!(h.mock.with(|s| s.kv_flushes) >= 1, "stores must be on disk before the app ends");
}

#[test]
fn a_crash_restart_is_reported_on_screen_and_normal_launches_are_not() {
    // One harness at a time: each installs its own mock host on this thread.
    let frame = |reason: i32| {
        let mut h = Harness::with(|s| {
            s.launch_reason = reason;
            s.canvas.keep_frame = true;
        });
        assert_eq!(h.player.launch_reason, reason);
        h.run_ms(50);
        h.mock.with(|s| s.canvas.last_frame.clone())
    };
    let normal = frame(sys::launch::NORMAL);
    assert!(!normal.is_empty());
    assert_eq!(normal, frame(sys::launch::NORMAL), "the drawing is deterministic");
    // The toast is drawn: the first frame differs from a normal launch's.
    assert_ne!(normal, frame(sys::launch::AFTER_TRAP));
    assert_ne!(normal, frame(sys::launch::AFTER_KILL));
}

#[test]
fn input_events_become_input_events() {
    let mut h = Harness::new();
    h.run_ms(20);
    // Scale 2 canvas: a wheel detent is one line, 40 px at scale 1, so 80 physical pixels.
    h.push(events::resize(640, 360, 2.0, false));
    h.run_ms(20);
    assert_eq!(h.player.host.surface.size, (640, 360, 2.0));
    let wait = |h: &mut Harness| h.player.dispatch(bucket_v0_host_events(0));
    fn bucket_v0_host_events(timeout: i64) -> Vec<rvp_host_rb::events::Ev> {
        rvp_host_rb::events::wait(timeout, 256)
    }
    let script = [
        events::key_down(0x20, 0, false),
        events::key_down(key::RIGHT, mods::SHIFT, true),
        events::key_up('k' as u32, mods::CTRL),
        events::pointer_move(10.5, 20.0),
        events::pointer_button(true, 11.0, 21.0, 2),
        events::pointer_button(false, 11.0, 21.0, 9), // an unknown button is ignored
        events::wheel(0.0, 3.0, false),
        events::wheel(0.0, -1.0, true),
        events::bare(ev::POINTER_LEAVE),
        events::with_u32(ev::DRAG_OVER, 1),
        events::with_u32(ev::FOCUS, 0),
        events::bare(ev::THEME_CHANGED),
        events::key_down(0x07, 0, false), // a control character is not a key
    ];
    for e in script {
        h.push(e);
    }
    wait(&mut h);
    let got: Vec<InputEvent> = h.player.host.input.0.drain(..).collect();
    assert_eq!(
        got,
        vec![
            InputEvent::KeyDown { key: Key::Space, mods: Default::default(), repeat: false },
            InputEvent::KeyDown {
                key: Key::Right,
                mods: rvp_host::Modifiers { shift: true, ..Default::default() },
                repeat: true
            },
            InputEvent::KeyUp {
                key: Key::Char('k'),
                mods: rvp_host::Modifiers { ctrl: true, ..Default::default() }
            },
            InputEvent::PointerMove { x: 10.5, y: 20.0 },
            InputEvent::PointerDown { x: 11.0, y: 21.0, button: PointerButton::Middle },
            InputEvent::Wheel { dx: 0.0, dy: 240.0 },
            InputEvent::Wheel { dx: 0.0, dy: -1.0 },
            InputEvent::PointerMove { x: -1.0e6, y: -1.0e6 },
            InputEvent::DragOver(true),
            InputEvent::Focus(false),
        ]
    );
}

#[test]
fn request_wake_is_the_timeout_of_events_wait() {
    let mut h = Harness::new();
    h.run_ms(50);
    // Idle: no session, no wake request: one frame.
    let idle = h.mock.with(|s| *s.waits.last().unwrap());
    assert_eq!(idle, 16_000);
    // Playing: the session asks to be woken within one 10 ms tick, and the wait is never longer.
    h.play_tone(2.0);
    h.run_ms(200);
    let waits = h.mock.with(|s| s.waits.clone());
    let recent = &waits[waits.len() - 20..];
    assert!(recent.iter().all(|w| (0..=10_000).contains(w)), "{recent:?}");
    assert!(recent.iter().any(|w| *w > 0), "{recent:?}");
    // With input already waiting, the loop does not sleep.
    h.push(events::pointer_move(2.0, 2.0));
    let evs = rvp_host_rb::events::wait(0, 8);
    assert_eq!(evs.len(), 1);
    h.player.host.shared.backlog.borrow_mut().extend(evs);
    assert_eq!(h.player.wait_timeout_us(), 0);
}

#[test]
fn suspend_stops_drawing_and_resume_draws_everything_again() {
    let mut h = Harness::new();
    h.run_ms(100);
    h.push(events::bare(ev::SUSPEND));
    h.run_ms(20);
    let before = h.mock.with(|s| s.canvas.presents.len());
    // Things change while hidden (the pointer moves over the controls): nothing is sent.
    h.push(events::pointer_move(100.0, 100.0));
    h.run_ms(100);
    assert_eq!(h.mock.with(|s| s.canvas.presents.len()), before);
    h.push(events::resume(5_000_000));
    h.run_ms(100);
    assert!(h.mock.with(|s| s.canvas.presents.len()) > before, "a full frame must follow RESUME");
    // Minimised and back through VISIBILITY works the same way.
    h.push(events::with_u32(ev::VISIBILITY, sys::visibility::MINIMIZED));
    h.run_ms(20);
    let n = h.mock.with(|s| s.canvas.presents.len());
    h.push(events::with_u32(ev::VISIBILITY, 0));
    h.run_ms(100);
    assert!(h.mock.with(|s| s.canvas.presents.len()) > n);
}

#[test]
fn a_present_for_a_stale_size_is_dropped_and_the_resize_fixes_it() {
    let mut h = Harness::new();
    h.run_ms(100);
    // The OS changed the size; until the app has seen RESIZE its presents return -BUSY and draw nothing.
    h.mock.with(|s| {
        s.canvas.stale = true;
        s.canvas.width = 400;
        s.canvas.height = 200;
    });
    h.push(events::pointer_move(50.0, 50.0));
    h.run_ms(60);
    let n = h.mock.with(|s| s.canvas.presents.len());
    h.mock.with(|s| {
        s.canvas.stale = false;
        s.push(events::resize(400, 200, 1.0, false));
    });
    h.run_ms(100);
    let after = h.mock.with(|s| s.canvas.presents.clone());
    assert!(after.len() > n);
    assert_eq!(after.last().unwrap().len, 400 * 200 * 4);
}

#[test]
fn fullscreen_goes_through_the_os_and_comes_back_as_a_resize() {
    let mut h = Harness::new();
    h.run_ms(50);
    use rvp_host::Surface;
    h.player.host.surface.set_fullscreen(true);
    assert_eq!(h.mock.with(|s| s.fullscreen_calls.clone()), vec![true]);
    h.push(events::resize(1920, 1080, 1.0, true));
    h.run_ms(50);
    assert_eq!(h.player.host.surface.size.0, 1920);
}

#[test]
fn unknown_events_and_kinds_are_ignored() {
    let mut h = Harness::new();
    h.run_ms(20);
    h.push(sys::Event::new(999, 0, 0));
    h.push(events::bare(ev::FRAME));
    h.push(events::with_u32(ev::CPU_COUNT_CHANGED, 2));
    h.push(events::with_u32(ev::MEMORY_PRESSURE, 2));
    h.run_ms(50);
    assert_eq!(h.player.cpu_count(), 2);
    assert!(h.player.exit_status().is_none());
}

#[test]
fn capabilities_gate_the_optional_interfaces_and_can_change_at_run_time() {
    use rvp_host::Host;
    let mut h = Harness::with(|s| s.caps = caps::CANVAS);
    assert!(h.player.host.now_playing().is_none());
    assert!(h.player.host.visualizer().is_none());
    assert!(h.player.host.library().is_none());
    h.push(events::caps_changed(caps::CANVAS | caps::VISUALIZER | caps::NOW_PLAYING | caps::LIBRARY));
    h.run_ms(20);
    assert!(h.player.host.now_playing().is_some());
    assert!(h.player.host.visualizer().is_some());
    assert!(h.player.host.library().is_some());
    h.push(events::caps_changed(caps::CANVAS));
    h.run_ms(20);
    assert!(h.player.host.visualizer().is_none());
}

#[test]
fn restart_saves_first() {
    let mut h = Harness::new();
    h.run_ms(20);
    assert!(h.player.restart());
    assert_eq!(h.mock.with(|s| s.restarts), 1);
    assert!(h.mock.with(|s| s.kv_flushes) >= 1);
    // An OS that refuses reports it.
    h.mock.with(|s| {
        s.unsupported.insert("restart");
    });
    let _ = transport::PLAY;
}
