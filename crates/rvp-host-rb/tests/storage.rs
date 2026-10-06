//! The key-value store: round trips, deletion, the cache class, slow volumes and the failures that leave the old value in place.
mod common;
use bucket_v0_sys as sys;
use common::Harness;
use rvp_core::task::block_on;
use rvp_host::Storage;
use rvp_host_rb::storage::RbStorage;

fn storage(h: &Harness) -> RbStorage {
    RbStorage::new(h.player.host.shared.clone())
}

#[test]
fn values_round_trip_and_an_empty_value_deletes() {
    let h = Harness::new();
    let mut st = storage(&h);
    assert_eq!(block_on(st.load("settings/audio")), None);
    block_on(st.store("settings/audio", b"v1\ncrossfade=3"));
    assert_eq!(block_on(st.load("settings/audio")).as_deref(), Some(&b"v1\ncrossfade=3"[..]));
    // Read your writes: nothing needs a flush for the new value to be seen.
    block_on(st.store("settings/audio", b"v1\ncrossfade=5"));
    assert_eq!(block_on(st.load("settings/audio")).as_deref(), Some(&b"v1\ncrossfade=5"[..]));
    // An empty value deletes the key.
    block_on(st.store("settings/audio", b""));
    assert_eq!(block_on(st.load("settings/audio")), None);
    assert!(h.mock.with(|s| s.kv.is_empty()));
    // A big value (the library index of a large collection) is one call.
    let big: Vec<u8> = (0..5_000_000u32).map(|i| (i % 251) as u8).collect();
    block_on(st.store("library/index", &big));
    assert_eq!(block_on(st.load("library/index")), Some(big));
    // Keys with slashes and non-ASCII text are fine.
    block_on(st.store("library/art/00ff é", b"x"));
    assert_eq!(block_on(st.load("library/art/00ff é")).as_deref(), Some(&b"x"[..]));
}

#[test]
fn thumbnails_go_to_the_cache_class_and_nothing_else_does() {
    let h = Harness::new();
    let mut st = storage(&h);
    block_on(st.store("library/art/0123456789abcdef", &[1; 100]));
    block_on(st.store("library/index", &[2; 100]));
    block_on(st.store("library/queue", &[3; 100]));
    block_on(st.store("library/art/0123456789abcdef", b"")); // deleting is a plain store
    let log = h.mock.with(|s| s.kv_stores.clone());
    assert_eq!(
        log,
        vec![
            ("library/art/0123456789abcdef".to_string(), 100, sys::kv::CACHE),
            ("library/index".to_string(), 100, 0),
            ("library/queue".to_string(), 100, 0),
            ("library/art/0123456789abcdef".to_string(), 0, 0),
        ]
    );
}

#[test]
fn a_slow_volume_is_waited_for_with_io_ready_and_other_events_are_kept() {
    let h = Harness::new();
    let mut st = storage(&h);
    h.mock.with(|s| {
        s.kv.insert("session/position".into(), (b"12345".to_vec(), 0));
        s.kv_cold.insert("session/position".into());
    });
    let t0 = h.now();
    assert_eq!(block_on(st.load("session/position")).as_deref(), Some(&b"12345"[..]));
    assert_eq!(h.now() - t0, 2_000, "it waited for IO_READY with file 0, not for a timer");
    assert!(
        h.player
            .host
            .shared
            .backlog
            .borrow()
            .iter()
            .any(|e| matches!(e, rvp_host_rb::events::Ev::IoReady { file: 0 }))
    );
    // A volume that never answers gives up after a few seconds and the key counts as missing.
    h.mock.with(|s| {
        s.kv_cold.insert("session/position".into());
        s.kv_latency_us = 600_000_000;
    });
    assert_eq!(block_on(st.load("session/position")), None);
    assert!(h.now() - t0 >= 5_000_000);
}

#[test]
fn a_full_store_or_a_too_large_value_leaves_the_old_value_and_the_app_carries_on() {
    let h = Harness::new();
    let mut st = storage(&h);
    block_on(st.store("a", &[1; 1000]));
    h.mock.with(|s| s.kv_quota = Some(1500));
    block_on(st.store("b", &[2; 1000])); // -NO_SPACE
    assert_eq!(block_on(st.load("b")), None);
    block_on(st.store("a", &[3; 1200])); // fits: replaces
    assert_eq!(block_on(st.load("a")).as_deref(), Some(&[3u8; 1200][..]));
    block_on(st.store("a", &[4; 1600])); // -NO_SPACE: the old value stays
    assert_eq!(block_on(st.load("a")).as_deref(), Some(&[3u8; 1200][..]));
    h.mock.with(|s| s.limits.insert(sys::limit::VALUE_SIZE, 100));
    block_on(st.store("c", &[5; 200])); // -TOO_LARGE
    assert_eq!(block_on(st.load("c")), None);
    let logs = h.mock.with(|s| s.logs.clone());
    assert_eq!(
        logs.iter().filter(|(lvl, t)| *lvl == sys::log_level::WARN && t.contains("kv_store")).count(),
        3,
        "{logs:?}"
    );
    assert!(
        logs.iter().any(|(_, t)| t.contains("NO_SPACE")) && logs.iter().any(|(_, t)| t.contains("TOO_LARGE"))
    );
}

#[test]
fn flush_asks_the_os_to_commit() {
    let h = Harness::new();
    let st = storage(&h);
    st.flush();
    st.flush();
    assert_eq!(h.mock.with(|s| s.kv_flushes), 2);
}

#[test]
fn the_queue_and_position_survive_a_restart_through_the_store() {
    use bucket_v0_mock::{FileSpec, events};
    use bucket_v0_sys::ev;
    // First run: play a file for a while, then the OS tells the app to end.
    let kv = {
        let mut h = Harness::new();
        // Long enough to be worth resuming: the app restores from 5 s in, with at least 10 s left.
        h.play_tone(30.0);
        h.push(events::transport(bucket_v0_sys::transport::SEEK_TO, 8_000_000));
        h.run_ms(400);
        h.push(events::with_u32(ev::TERMINATE, 1000));
        assert!(!h.player.run_once());
        h.mock.with(|s| s.kv.clone())
    };
    assert!(kv.contains_key("session/position"), "{:?}", kv.keys().collect::<Vec<_>>());
    assert!(kv.contains_key("library/queue"));
    // Second run (a restart): the queue comes back and the item is open, paused, near where it was. Every id from the OS is
    // stable, so the file is reopened by id.
    let mut h = Harness::with(|s| {
        s.kv = kv;
        s.add_file(FileSpec::new("tone.wav", common::tone_wav(30.0)));
    });
    assert!(h.run_until(3000, |h| h.player.app.model().has_media()));
    h.run_ms(300);
    let m = h.player.app.model().clone();
    assert_eq!(m.state, rvp_ui::MediaState::Paused);
    assert!((8_000_000..=8_700_000).contains(&m.position_us), "{}", m.position_us);
    assert_eq!(h.player.app.playlist().len(), 1);
}
