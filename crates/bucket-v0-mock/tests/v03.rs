//! The mock reads the pages as the Bucket Simulator does (the v0.3 clarifications): each test is one point of that list.
use bucket_v0_mock::{AppEnded, FileSpec, MockHost, catch_end, events};
use bucket_v0_sys::{self as sys, EVENT_SIZE, Event, err, ev};

fn wait(timeout: i64, max: usize) -> Vec<Event> {
    let mut raw = vec![0u8; max * EVENT_SIZE];
    // SAFETY: the buffer holds `max` records.
    let n = unsafe { sys::events_wait(raw.as_mut_ptr(), max as i32, timeout) };
    raw.chunks_exact(EVENT_SIZE)
        .take(n.max(0) as usize)
        .map(|c| Event::from_bytes(c.try_into().unwrap()))
        .collect()
}

#[test]
fn events_wait_refuses_bad_arguments() {
    let mock = MockHost::new();
    let _g = mock.install();
    let mut buf = [0u8; 64];
    // SAFETY: the buffer holds one record.
    unsafe {
        assert_eq!(sys::events_wait(buf.as_mut_ptr(), 0, 0), err::INVALID);
        assert_eq!(sys::events_wait(buf.as_mut_ptr(), 1, -2), err::INVALID);
        assert_eq!(sys::events_wait(buf.as_mut_ptr(), 1, 0), 0);
    }
}

#[test]
fn restart_and_exit_do_not_return() {
    let mock = MockHost::new();
    let _g = mock.install();
    // SAFETY: no arguments.
    assert_eq!(catch_end(|| unsafe { sys::restart() }), Err(AppEnded::Restart));
    assert_eq!(catch_end(|| unsafe { sys::exit(3) }), Err(AppEnded::Exit(3)));
    assert_eq!(mock.with(|s| (s.restarts, s.exits.clone())), (1, vec![3]));
    // Refused, it returns the error.
    mock.with(|s| s.restart_error = Some(err::UNSUPPORTED));
    // SAFETY: no arguments.
    assert_eq!(catch_end(|| unsafe { sys::restart() }), Ok(err::UNSUPPORTED));
}

#[test]
fn the_thread_limit_counts_the_main_thread() {
    let mock = MockHost::new();
    let _g = mock.install();
    mock.with(|s| s.limits.insert(sys::limit::THREADS, 4));
    // SAFETY: plain calls.
    unsafe {
        assert_eq!((sys::thread_spawn(1), sys::thread_spawn(2), sys::thread_spawn(3)), (1, 2, 3));
        assert_eq!(sys::thread_spawn(4), err::NO_SPACE, "4 threads with the main one");
        assert_eq!(sys::thread_priority(3, 1), 0);
        assert_eq!(sys::thread_priority(0, 0), 0);
        assert_eq!(sys::thread_priority(1, 2), err::INVALID);
        assert_eq!(sys::thread_priority(9, 1), err::NOT_FOUND);
    }
}

#[test]
fn canvas_present_is_strict_and_a_pending_resize_comes_first() {
    let mock = MockHost::new();
    let _g = mock.install();
    mock.with(|s| (s.canvas.width, s.canvas.height) = (4, 2));
    let px = [0u8; 4 * 2 * 4];
    // SAFETY: the buffer is `len` bytes.
    let present = |len: i32, r: (i32, i32, i32, i32)| unsafe {
        sys::canvas_present(px.as_ptr(), len, r.0, r.1, r.2, r.3)
    };
    assert_eq!(present(32, (0, 0, 4, 2)), 0);
    assert_eq!(present(31, (0, 0, 4, 2)), err::INVALID, "len must be exactly width x height x 4");
    assert_eq!(present(32, (-1, 0, 2, 2)), err::INVALID);
    assert_eq!(present(32, (3, 0, 2, 2)), err::INVALID, "leaves the canvas");
    // A resize in flight: -BUSY first, even for a bad argument, until the app has been handed the RESIZE.
    mock.with(|s| s.resize(4, 2, 1.0, false));
    assert_eq!(present(31, (0, 0, 4, 2)), err::BUSY);
    assert_eq!(wait(0, 4)[0].kind, ev::RESIZE);
    assert_eq!(present(32, (0, 0, 4, 2)), 0);
}

#[test]
fn audio_streams_grant_the_listed_rates_and_every_open_is_a_new_stream() {
    let mock = MockHost::new();
    let _g = mock.install();
    mock.with(|s| s.audio_device_rate = 48_000);
    let mut info = sys::AudioOpenInfo { struct_size: 16, ..Default::default() };
    let open = |rate: i32, info: &mut sys::AudioOpenInfo| {
        // SAFETY: the struct is 16 bytes.
        unsafe { sys::audio_open(rate, 2, (info as *mut sys::AudioOpenInfo).cast()) }
    };
    let a = open(44_100, &mut info);
    assert_eq!((info.rate, info.capacity_frames), (44_100, 44_100), "as asked, a ring of one second");
    let b = open(22_050, &mut info);
    assert_eq!(info.rate, 48_000, "any other rate becomes the device's");
    assert!(a > 0 && b > a, "a new stream each time; the old one stays open");
    assert_eq!(mock.with(|s| s.audio.len()), 2);
    // SAFETY: plain calls.
    unsafe {
        assert_eq!(sys::audio_volume(a, 3.0), 0);
        assert_eq!(sys::audio_volume(a, f32::NAN), err::INVALID);
    }
    assert_eq!(mock.with(|s| s.audio[&a].volume), 1.0, "clamped");
}

#[test]
fn files_cap_zero_folders_siblings_and_handle_limits() {
    let mock = MockHost::new();
    let _g = mock.install();
    let (file, folder) = mock.with(|s| {
        s.add_sibling("d", FileSpec::new("cover.jpg", vec![1, 2, 3]).in_dir("d"));
        let f = s.new_handle(FileSpec::new("a.mp3", vec![9; 10]).in_dir("d").slow(1000));
        let d = s.new_handle(FileSpec::folder("d", "d"));
        (f, d)
    });
    let mut b = [0u8; 8];
    // SAFETY: valid buffers.
    unsafe {
        // `cap` 0 is 0 (not the end of the file), even on a cold block.
        assert_eq!(sys::file_read_at(file, 0, b.as_mut_ptr(), 0), 0);
        assert_eq!(sys::file_read_at(file, 0, b.as_mut_ptr(), 8), err::BUSY);
        assert_eq!(sys::file_size(folder), err::INVALID as i64);
        assert_eq!(sys::file_read_at(folder, 0, b.as_mut_ptr(), 8), err::INVALID);
        let inside = sys::file_open_sibling(folder, b"cover.jpg".as_ptr(), 9);
        assert!(inside > 0);
        assert!(sys::file_open_sibling(file, b"cover.jpg".as_ptr(), 9) > 0);
        assert_eq!(sys::file_open_sibling(folder, b"nope".as_ptr(), 4), err::NOT_FOUND);
        assert!(
            sys::file_close(inside) == 0 && sys::file_open_sibling(folder, b"cover.jpg".as_ptr(), 9) > inside,
            "never reused"
        );
    }
    mock.with(|s| s.limits.insert(sys::limit::HANDLES, 3));
    // SAFETY: valid ranges.
    assert_eq!(
        unsafe { sys::file_open_sibling(folder, b"cover.jpg".as_ptr(), 9) },
        err::NO_SPACE,
        "a count limit"
    );
    // save: the last component only; an empty name is invalid.
    // SAFETY: valid ranges.
    unsafe {
        assert!(sys::file_save(b"x/y/z.m3u".as_ptr(), 9, b"a".as_ptr(), 1, b"d".as_ptr(), 1) > 0);
        assert_eq!(sys::file_save(b"x/".as_ptr(), 2, b"a".as_ptr(), 1, b"d".as_ptr(), 1), err::INVALID);
    }
    assert_eq!(mock.with(|s| s.saves[0].name.clone()), "z.m3u");
}

#[test]
fn key_value_limits_eviction_and_failing_fetches() {
    let mock = MockHost::new();
    let _g = mock.install();
    let key = "k".repeat(256);
    // SAFETY: valid ranges.
    unsafe {
        assert_eq!(
            sys::kv_store(key.as_ptr(), 256, b"v".as_ptr(), 1, 0),
            err::TOO_LARGE,
            "a size, not a count"
        );
        assert_eq!(sys::kv_load(key.as_ptr(), 256, std::ptr::null_mut(), 0), err::TOO_LARGE);
        assert_eq!(sys::kv_store(b"".as_ptr(), 0, b"v".as_ptr(), 1, 0), err::INVALID);
    }
    mock.with(|s| s.kv_quota = Some(12));
    let put = |k: &str, n: usize, flags: i32| {
        // SAFETY: valid ranges.
        unsafe { sys::kv_store(k.as_ptr(), k.len() as i32, vec![1u8; n].as_ptr(), n as i32, flags) }
    };
    assert_eq!(put("old", 4, sys::kv::CACHE), 0);
    assert_eq!(put("new", 4, sys::kv::CACHE), 0);
    assert_eq!(put("keep", 2, 0), 0);
    assert_eq!(put("big", 6, 0), 0, "the oldest cache entry made room");
    assert!(mock.with(|s| !s.kv.contains_key("old") && s.kv.contains_key("new")));
    // Still not enough: it fails and what it evicted stays evicted.
    assert_eq!(put("huge", 9, 0), err::NO_SPACE);
    assert!(mock.with(|s| !s.kv.contains_key("new")));
    // A fetch that fails: BUSY, IO_READY, then -IO.
    mock.with(|s| {
        s.kv_cold.insert("x".into());
        s.kv_fail.insert("x".into());
    });
    // SAFETY: valid ranges.
    unsafe {
        assert_eq!(sys::kv_load(b"x".as_ptr(), 1, std::ptr::null_mut(), 0), err::BUSY);
        assert_eq!(wait(1_000_000, 4)[0].kind, ev::IO_READY);
        assert_eq!(sys::kv_load(b"x".as_ptr(), 1, std::ptr::null_mut(), 0), err::IO);
    }
}

#[test]
fn now_playing_validates_what_it_is_given() {
    let mock = MockHost::new();
    let _g = mock.install();
    let pb = |state: u32, rate: f32, pos: i64, t: i64| {
        let raw = sys::NowPlayingPlaybackRaw {
            struct_size: 40,
            state,
            rate,
            reserved0: 0,
            position_us: pos,
            host_time_us: t,
            flags: 4,
            reserved1: 0,
        };
        // SAFETY: the 40-byte struct.
        unsafe { sys::now_playing_playback((&raw as *const sys::NowPlayingPlaybackRaw).cast()) }
    };
    assert_eq!(pb(3, 1.0, 0, 0), err::INVALID);
    assert_eq!(pb(1, f32::INFINITY, 0, 0), err::INVALID);
    assert_eq!(pb(1, 1.0, -1, 0), err::INVALID);
    assert_eq!(pb(1, 1.0, 0, 0), 0);
    // The same report with the position where the shell would put it: needless; a jump of more than 0.5 s is not.
    assert_eq!(pb(1, 1.0, 1_000_000, 1_000_000), 0);
    assert_eq!(mock.with(|s| s.needless_playback), 1);
    assert_eq!(pb(1, 1.0, 9_000_000, 2_000_000), 0);
    assert_eq!(mock.with(|s| s.needless_playback), 1);
}

#[test]
fn a_walk_replaces_then_appends_and_the_os_starts_the_first_one() {
    let mock = MockHost::new();
    let _g = mock.install();
    let files: Vec<(String, String)> = (0..450).map(|i| (format!("id{i}"), format!("a/{i}.mp3"))).collect();
    let refs: Vec<(&str, &str, u64, i64)> =
        files.iter().map(|(a, b)| (a.as_str(), b.as_str(), 1, 1)).collect();
    mock.with(|s| s.answer_folder(7, "dir:M", "M", &refs));
    let size = || {
        // SAFETY: a null buffer of 0 bytes asks for the size.
        unsafe { sys::library_listing(b"dir:M".as_ptr(), 5, 0, std::ptr::null_mut(), 0) }
    };
    assert_eq!(size(), 0, "nothing is visible before the first event");
    let got = wait(1_000_000, 8);
    assert_eq!(got[0].kind, ev::FOLDER_ADDED);
    // 450 files: 200 + 200 + 50, the last without the partial flag; the total grows as events are generated.
    let mut totals = Vec::new();
    let mut flags = Vec::new();
    for _ in 0..3 {
        for e in wait(1_000_000, 8) {
            if e.kind == ev::LIBRARY_LISTING {
                flags.push(e.flags & ev::FLAG_MORE != 0);
                totals.push(size());
            }
        }
    }
    assert_eq!(flags, vec![true, true, false]);
    assert!(totals[0] > 0 && totals[0] < totals[1] && totals[1] < totals[2], "{totals:?}");
    assert!(mock.with(|s| s.rescans.is_empty() && s.walking.is_empty()));
    // A second walk replaces the first one's listing at its first event.
    mock.with(|s| s.walk("dir:M", &refs[..10], 1_000));
    assert_eq!(wait(1_000_000, 8).iter().filter(|e| e.kind == ev::LIBRARY_LISTING).count(), 1);
    assert!(size() < totals[0]);
}

#[test]
fn a_rescan_of_an_unreadable_root_is_io_and_a_running_walk_is_not_restarted() {
    let mock = MockHost::new();
    let _g = mock.install();
    mock.with(|s| {
        s.add_root("usb", "U", false);
        s.add_root("dir", "D", true);
        s.walking.insert("dir".into());
    });
    // SAFETY: valid ranges.
    unsafe {
        assert_eq!(sys::library_rescan(b"usb".as_ptr(), 3), err::IO);
        assert_eq!(sys::library_rescan(b"dir".as_ptr(), 3), 0);
    }
    assert!(mock.with(|s| s.rescans.is_empty()));
}

#[test]
fn frame_events_carry_the_vblank_just_passed_and_the_next() {
    let e = events::frame(16_667, 33_333);
    assert_eq!((e.time_us, e.i64_at(16)), (16_667, 33_333));
}

#[test]
fn an_underrun_is_a_gap_the_app_then_fills_not_a_drained_end() {
    let mock = MockHost::new();
    let _g = mock.install();
    let mut info = sys::AudioOpenInfo { struct_size: 16, ..Default::default() };
    // SAFETY: the struct is 16 bytes; the write range is 100 frames of silence.
    let h = unsafe { sys::audio_open(44_100, 2, (&mut info as *mut sys::AudioOpenInfo).cast()) };
    let pcm = vec![0f32; 200];
    // SAFETY: as above.
    let write = || unsafe { sys::audio_write(h, pcm.as_ptr(), 100) };
    assert_eq!(write(), 100);
    // It drains at the end and stays dry: no underrun.
    mock.with(|s| s.advance(1_000_000));
    // SAFETY: plain call.
    unsafe { sys::audio_queued(h) };
    assert_eq!(mock.with(|s| s.audio[&h].underruns), 0);
    // The app writes more after the dry spell: that was a gap.
    assert_eq!(write(), 100);
    assert_eq!(mock.with(|s| s.audio[&h].underruns), 1);
}

#[test]
fn the_first_playback_report_after_metadata_counts_as_a_change() {
    let mock = MockHost::new();
    let _g = mock.install();
    let pb = |t: i64| {
        let raw = sys::NowPlayingPlaybackRaw {
            struct_size: 40,
            state: 1,
            rate: 1.0,
            reserved0: 0,
            position_us: t,
            host_time_us: t,
            flags: 4,
            reserved1: 0,
        };
        // SAFETY: the 40-byte struct.
        unsafe { sys::now_playing_playback((&raw as *const sys::NowPlayingPlaybackRaw).cast()) }
    };
    assert_eq!(pb(0), 0);
    let mut meta = [0u8; 64];
    meta[..4].copy_from_slice(&64u32.to_le_bytes());
    // SAFETY: the 64-byte struct.
    assert_eq!(unsafe { sys::now_playing_metadata(meta.as_ptr()) }, 0);
    assert_eq!(pb(1_000_000), 0, "unchanged, but the first after metadata");
    assert_eq!(mock.with(|s| s.needless_playback), 0);
    assert_eq!(pb(2_000_000), 0);
    assert_eq!(mock.with(|s| s.needless_playback), 1);
}
