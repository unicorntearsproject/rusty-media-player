//! Files: non-blocking reads (`-BUSY` and `IO_READY`), opening by id, drops and picks.
mod common;
use bucket_v0_mock::{FileSpec, events};
use bucket_v0_sys::{self as sys, ev};
use common::{Harness, tone_wav};
use rvp_core::task::{block_on, poll_once};
use rvp_host::{Host, OpenRequest, Source};
use rvp_host_rb::events::{self as rb_events, Ev};
use rvp_host_rb::source::RbSource;
use std::pin::pin;
use std::task::Poll;

fn data(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7 + i / 251) as u8).collect()
}

#[test]
fn a_cold_read_is_pending_until_io_ready_and_then_correct() {
    let h = Harness::new();
    let bytes = data(200_000);
    let handle = h.mock.with(|s| s.new_handle(FileSpec::new("a.bin", bytes.clone()).slow(5_000)));
    let mut src = RbSource::from_handle(handle, 1 << 20);
    assert_eq!(block_on(src.size()), Some(200_000));
    assert_eq!(src.name(), "a.bin");

    let mut buf = vec![0u8; 1000];
    {
        let mut fut = pin!(src.read_at(0, &mut buf));
        // The host started a fetch and said -BUSY: the future is pending, not blocked.
        assert!(matches!(poll_once(fut.as_mut()), Poll::Pending));
        assert!(
            matches!(poll_once(fut.as_mut()), Poll::Pending),
            "still cold, and no second fetch is started"
        );
        // The wake-up: the sleep in events_wait ends with IO_READY for this file, 5 ms of virtual time later.
        let t0 = h.mock.with(|s| s.now_us);
        let evs = rb_events::wait(100_000, 16);
        assert_eq!(evs, vec![Ev::IoReady { file: handle }]);
        assert_eq!(h.mock.with(|s| s.now_us) - t0, 5_000);
        match poll_once(fut.as_mut()) {
            Poll::Ready(Ok(n)) => assert_eq!(n, 1000),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(buf, bytes[..1000]);

    // A read that crosses the end of a fetched block is short; the next block is cold again.
    let mut big = vec![0u8; 100_000];
    let n = block_on(src.read_at(60_000, &mut big[..10_000])).unwrap();
    assert_eq!(n, 65_536 - 60_000, "a short read at the end of the block");
    assert_eq!(big[..n], bytes[60_000..60_000 + n]);
    {
        let mut fut = pin!(src.read_at(65_536, &mut big[..10]));
        assert!(matches!(poll_once(fut.as_mut()), Poll::Pending));
        rb_events::wait(100_000, 16);
        assert!(matches!(poll_once(fut.as_mut()), Poll::Ready(Ok(10))));
    }

    // End of the file is 0, not -BUSY; a bad handle is an error.
    assert_eq!(block_on(src.read_at(200_000, &mut big)).unwrap(), 0);
    let mut dead = RbSource::from_handle(handle, 1 << 20);
    drop(src);
    assert_eq!(h.mock.with(|s| s.closed.clone()), vec![handle], "dropping the source closes the handle");
    assert!(block_on(dead.read_at(0, &mut big)).is_err());
}

#[test]
fn reads_are_bounded_hinted_and_sequential_reads_prefetch() {
    let h = Harness::new();
    let handle = h.mock.with(|s| s.new_handle(FileSpec::new("b.bin", data(3_000_000))));
    let mut src = RbSource::from_handle(handle, 4096);
    let mut buf = vec![0u8; 100_000];
    assert_eq!(block_on(src.read_at(0, &mut buf)).unwrap(), 4096, "never more than the OS's per-call limit");
    let hints = h.mock.with(|s| s.prefetches.clone());
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].0, handle);
    assert_eq!(hints[0].1, 4096, "the hint starts where the read ended");
    // Another read inside the hinted range adds no hint.
    block_on(src.read_at(4096, &mut buf)).unwrap();
    assert_eq!(h.mock.with(|s| s.prefetches.len()), 1);
}

#[test]
fn an_unknown_size_is_none_and_a_nameless_file_gets_a_name() {
    let h = Harness::new();
    let mut spec = FileSpec::new("", data(10));
    spec.unknown_size = true;
    let handle = h.mock.with(|s| s.new_handle(spec));
    let src = RbSource::from_handle(handle, 4096);
    assert_eq!(block_on(src.size()), None);
    assert_eq!(src.name(), "file");
}

#[test]
fn opening_by_id_waits_for_a_busy_volume_and_keeps_other_events() {
    let mut h = Harness::new();
    h.mock.with(|s| {
        s.add_file(FileSpec::new("c.wav", tone_wav(0.1)));
        s.open_cold.insert("id:c.wav".into());
    });
    // A key press arrives while the open waits for the volume: it must not be lost.
    h.push(events::key_down('x' as u32, 0, false));
    let src =
        block_on(h.player.host.open(OpenRequest::Id("id:c.wav".into()))).expect("opens after IO_READY { 0 }");
    assert_eq!(src.name(), "c.wav");
    let kept = h.player.host.shared.backlog.borrow().clone();
    assert!(kept.iter().any(|e| matches!(e, Ev::IoReady { file: 0 })));
    assert!(kept.iter().any(|e| matches!(e, Ev::Key { key, .. } if *key == 'x' as u32)));
    // The driver handles them first on the next step.
    h.player.step();
    assert!(h.player.host.shared.backlog.borrow().is_empty());
    // Unknown ids fail with a message, not a trap.
    let e = block_on(h.player.host.open(OpenRequest::Id("id:nope".into()))).err().unwrap();
    assert!(e.0.contains("not there"), "{}", e.0);
    assert!(block_on(h.player.host.open(OpenRequest::Pick)).is_err());
}

#[test]
fn a_busy_open_that_never_finishes_times_out() {
    let mut h = Harness::new();
    h.mock.with(|s| {
        s.add_file(FileSpec::new("d.wav", tone_wav(0.1)));
        // The same id is cold again after every retry: re-arm it from the test by never delivering IO_READY.
        s.open_cold.insert("id:d.wav".into());
        s.kv_latency_us = 60_000_000;
    });
    let e = block_on(h.player.host.open(OpenRequest::Id("id:d.wav".into()))).err().unwrap();
    assert!(e.0.contains("too long"), "{}", e.0);
    assert!(h.mock.with(|s| s.now_us) >= 1_000_000 + 5_000_000);
}

#[test]
fn a_dropped_group_opens_as_one_queue_and_handles_are_not_leaked() {
    let mut h = Harness::new();
    h.run_ms(50);
    // Three files in one drop (one event each, "more follows" on all but the last); the third has no stable id.
    let handles: Vec<i32> = h.mock.with(|s| {
        let mut v = Vec::new();
        for n in ["one.wav", "two.wav"] {
            let spec = FileSpec::new(n, tone_wav(0.3));
            s.add_file(spec.clone());
            v.push(s.new_handle(spec));
        }
        v.push(s.new_handle(FileSpec::new("three.wav", tone_wav(0.3)).without_id()));
        v
    });
    h.push(events::file(ev::DROP, handles[0], true));
    h.push(events::file(ev::DROP, handles[1], true));
    h.push(events::file(ev::DROP, handles[2], false));
    h.run_ms(300);
    assert_eq!(h.player.app.playlist().len(), 3);
    let names: Vec<_> = h.player.app.playlist().items().iter().map(|i| i.name.clone()).collect();
    assert_eq!(names, ["one.wav", "two.wav", "three.wav"]);
    // Files with an id were closed at once (the app may hold a limited number); the one without keeps its handle until used.
    let closed = h.mock.with(|s| s.closed.clone());
    assert!(closed.contains(&handles[0]) && closed.contains(&handles[1]));
    assert!(!closed.contains(&handles[2]), "the handle of a file without an id stays open");
    assert_eq!(h.player.host.shared.stash.borrow().len(), 1);
}

#[test]
fn a_group_that_straddles_two_waits_is_still_one_group() {
    let mut h = Harness::new();
    h.run_ms(50);
    let handles: Vec<i32> = h.mock.with(|s| {
        (0..2)
            .map(|i| {
                let spec = FileSpec::new(&format!("t{i}.wav"), tone_wav(0.2));
                s.add_file(spec.clone());
                s.new_handle(spec)
            })
            .collect()
    });
    h.push(events::file(ev::DROP, handles[0], true));
    h.run_ms(40);
    assert_eq!(h.player.app.playlist().len(), 0, "waiting for the rest of the drop");
    h.push(events::file(ev::DROP, handles[1], false));
    h.run_ms(100);
    assert_eq!(h.player.app.playlist().len(), 2);
}

#[test]
fn a_group_whose_last_event_never_comes_opens_after_a_second() {
    let mut h = Harness::new();
    let handle = h.mock.with(|s| {
        let spec = FileSpec::new("only.wav", tone_wav(0.2));
        s.add_file(spec.clone());
        s.new_handle(spec)
    });
    h.push(events::file(ev::DROP, handle, true));
    h.run_ms(500);
    assert_eq!(h.player.app.playlist().len(), 0);
    h.run_ms(1200);
    assert_eq!(h.player.app.playlist().len(), 1);
}

#[test]
fn picks_ask_with_the_open_types_and_a_cancel_is_quiet() {
    use rvp_app::Effect;
    let mut h = Harness::new();
    h.run_ms(20);
    // The app's own actions create the requests (the Open button and the add-files action).
    h.player.request_pick(false);
    h.player.request_pick(true);
    let picks = h.mock.with(|s| s.picks.clone());
    assert_eq!(picks.len(), 2);
    assert_eq!(picks[0].flags, sys::pick::MULTI);
    for ext in rvp_host_rb::OPEN_EXTENSIONS {
        assert!(picks[0].kinds.split(',').any(|k| k == *ext), "{ext}");
    }
    // Replace: the picked files become the queue. Add: they join it.
    let (a, b) = h.mock.with(|s| {
        let mk = |s: &mut bucket_v0_mock::State, n: &str| {
            let spec = FileSpec::new(n, tone_wav(0.2));
            s.add_file(spec.clone());
            s.new_handle(spec)
        };
        (mk(s, "p1.wav"), mk(s, "p2.wav"))
    });
    h.push(events::file_picked(picks[0].request, a, false));
    h.run_ms(100);
    assert_eq!(h.player.app.playlist().len(), 1);
    h.push(events::file_picked(picks[1].request, b, false));
    h.run_ms(100);
    assert_eq!(h.player.app.playlist().len(), 2, "an add-files pick appends");
    // A cancelled pick opens nothing and forgets the request.
    h.player.request_pick(false);
    let req = h.mock.with(|s| s.picks.last().unwrap().request);
    h.push(events::file_picked(req, sys::err::CANCELLED, false));
    h.run_ms(50);
    assert_eq!(h.player.app.playlist().len(), 2);
    let _ = Effect::PickFile;
}
