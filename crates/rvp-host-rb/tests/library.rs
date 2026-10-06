//! The media library: roots, the listing cursor with partial listings, the requests the app makes, and a library built from what
//! the shell lists.
mod common;
use bucket_v0_mock::{FileSpec, events};
use bucket_v0_sys::{self as sys, ev};
use common::{Harness, tone_wav};
use rvp_host::{Host, Library};

fn listing_event(h: &Harness, root: &str, partial: bool) {
    h.mock.with(|s| s.push_text(events::library(ev::LIBRARY_LISTING, partial), 16, root));
}

/// Hand the events that are waiting to the driver, without ticking the app (which would take the listing itself).
fn deliver(h: &mut Harness) {
    let evs = rvp_host_rb::events::wait(0, 256);
    h.player.dispatch(evs);
}

fn take(h: &mut Harness) -> Option<rvp_host::Listing> {
    h.player.host.library.take_listing()
}

#[test]
fn roots_are_read_with_their_names_and_whether_they_can_be_read_now() {
    let mut h = Harness::new();
    h.mock.with(|s| {
        s.add_root("dir:Music", "Music", true);
        s.add_root("dir:Tab\there", "Name with\ttab and\nnewline and \\ slash", true);
        s.add_root("usb:Backup", "Backup disk", false); // remembered, but the disk is not plugged in
    });
    let lib = h.player.host.library().unwrap();
    assert_eq!(lib.connected_roots(), vec!["dir:Music".to_string(), "dir:Tab\there".to_string()]);
    let all = rvp_host_rb::library::RbLibrary::roots();
    assert_eq!(all.len(), 3);
    assert_eq!(all[1].name, "Name with\ttab and\nnewline and \\ slash");
    assert!(!all[2].readable);
}

#[test]
fn a_listing_is_read_through_the_cursor_and_released() {
    let mut h = Harness::new();
    h.mock.with(|s| {
        s.add_root("dir:Music", "My Music", true);
        s.set_listing(
            "dir:Music",
            &[
                ("id-1", "Artist/Album/01 Song.mp3", 4_000_000, 1_700_000_000_000),
                ("id-2", "Artist/Album/cover.jpg", 90_000, 0),
            ],
        );
    });
    listing_event(&h, "dir:Music", false);
    deliver(&mut h);
    let l = take(&mut h).expect("a listing");
    assert_eq!((l.root.as_str(), l.name.as_str()), ("dir:Music", "My Music"));
    assert_eq!(l.files.len(), 2);
    assert_eq!(l.files[0].id, "id-1");
    assert_eq!(l.files[0].path, "Artist/Album/01 Song.mp3");
    assert_eq!((l.files[0].size, l.files[0].mtime_ms), (4_000_000, 1_700_000_000_000));
    assert_eq!(l.files[1].mtime_ms, 0, "0 means unknown");
    assert!(take(&mut h).is_none());
    assert_eq!(
        h.mock.with(|s| s.releases.clone()),
        vec!["dir:Music".to_string()],
        "the kept listing is freed"
    );
}

#[test]
fn a_large_listing_arrives_in_parts_that_append() {
    let mut h = Harness::new();
    let files: Vec<(String, String)> = (0..2000)
        .map(|i| {
            (
                format!("handle-{i:05}"),
                format!("Artist {}/Album {}/{:02} Track number {i}.flac", i / 100, i / 10, i % 100),
            )
        })
        .collect();
    let refs: Vec<(&str, &str, u64, i64)> = files
        .iter()
        .enumerate()
        .map(|(i, (a, b))| (a.as_str(), b.as_str(), i as u64 * 10, i as i64))
        .collect();
    h.mock.with(|s| {
        s.add_root("dir:Big", "Big", true);
        s.set_listing("dir:Big", &refs);
    });
    let total = h.mock.with(|s| s.listings["dir:Big"].blob.len());
    assert!(total > 150_000, "more than two 64 KiB reads: {total}");
    // The walk reports a third of it, then two thirds (both partial: more follows), then everything. A cut can fall inside a
    // record; the next read continues where the last ended.
    for upto in [total / 3 + 5, total * 2 / 3 + 1] {
        h.mock.with(|s| {
            s.reveal("dir:Big", upto);
        });
        listing_event(&h, "dir:Big", true);
        deliver(&mut h);
        assert!(take(&mut h).is_none(), "a partial listing is not handed over");
    }
    h.mock.with(|s| {
        s.reveal("dir:Big", total);
    });
    listing_event(&h, "dir:Big", false);
    deliver(&mut h);
    let l = take(&mut h).expect("the whole listing");
    assert_eq!(l.files.len(), 2000);
    for (i, f) in l.files.iter().enumerate() {
        assert_eq!(f.id, files[i].0);
        assert_eq!(f.path, files[i].1);
        assert_eq!((f.size, f.mtime_ms), (i as u64 * 10, i as i64));
    }
    assert_eq!(h.mock.with(|s| s.releases.len()), 1, "freed once, at the end");
}

#[test]
fn a_new_walk_that_replaces_a_partial_listing_starts_over() {
    let mut h = Harness::new();
    let big: Vec<(String, String)> = (0..400).map(|i| (format!("old-{i}"), format!("old/{i}.mp3"))).collect();
    let refs: Vec<(&str, &str, u64, i64)> = big.iter().map(|(a, b)| (a.as_str(), b.as_str(), 1, 1)).collect();
    h.mock.with(|s| {
        s.add_root("dir:R", "R", true);
        s.set_listing("dir:R", &refs);
        let half = s.listings["dir:R"].blob.len() / 2;
        s.reveal("dir:R", half);
    });
    listing_event(&h, "dir:R", true);
    deliver(&mut h);
    // A rescan began: the kept listing is a different, shorter one.
    h.mock.with(|s| s.set_listing("dir:R", &[("new-1", "new/a.mp3", 5, 6)]));
    listing_event(&h, "dir:R", false);
    deliver(&mut h);
    let l = take(&mut h).unwrap();
    assert_eq!(l.files.len(), 1);
    assert_eq!(l.files[0].id, "new-1");
}

#[test]
fn a_damaged_listing_is_dropped_not_trusted() {
    let mut h = Harness::new();
    h.mock.with(|s| {
        s.add_root("dir:Bad", "Bad", true);
        let mut blob = vec![0u8; 48];
        blob[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        s.listings.insert("dir:Bad".into(), bucket_v0_mock::KeptListing { visible: blob.len(), blob });
    });
    listing_event(&h, "dir:Bad", false);
    deliver(&mut h);
    assert!(take(&mut h).is_none());
    assert_eq!(h.mock.with(|s| s.releases.clone()), vec!["dir:Bad".to_string()]);
    // A listing the OS no longer has is an error that is logged, not a crash.
    listing_event(&h, "dir:Gone", false);
    deliver(&mut h);
    assert!(take(&mut h).is_none());
    assert!(h.mock.with(|s| s.logs.iter().any(|(_, t)| t.contains("dir:Gone") && t.contains("NOT_FOUND"))));
}

#[test]
fn folders_rescans_and_forgetting_reach_the_os() {
    let mut h = Harness::new();
    h.mock.with(|s| s.add_root("dir:Music", "Music", true));
    h.run_ms(20);
    // The user adds a folder: a picker request; the answer names the root and the app asks for its walk.
    h.player.request_folder();
    let req = h.mock.with(|s| s.folder_requests[0]);
    h.mock.with(|s| s.push_text(events::folder_added(req), 20, "dir:Music"));
    h.run_ms(20);
    assert_eq!(h.mock.with(|s| s.rescans.clone()), vec!["dir:Music".to_string()]);
    // Cancelled: nothing. An answer to a request we did not make: nothing.
    h.player.request_folder();
    let req2 = h.mock.with(|s| *s.folder_requests.last().unwrap());
    h.push(events::folder_cancelled(req2));
    h.mock.with(|s| s.push_text(events::folder_added(4242), 20, "dir:Music"));
    h.run_ms(20);
    assert_eq!(h.mock.with(|s| s.rescans.len()), 1);
    // Files changed on disk: the walk is asked for again (it is incremental).
    h.mock.with(|s| s.push_text(events::library(ev::LIBRARY_CHANGED, false), 16, "dir:Music"));
    h.mock.with(|s| s.push_text(events::library(ev::LIBRARY_PROGRESS, false), 16, "dir:Music"));
    h.run_ms(20);
    assert_eq!(h.mock.with(|s| s.rescans.len()), 2);
    // Forgetting a root revokes it; a rescan of a root the OS does not know fails quietly.
    h.player.host.library.forget("dir:Music");
    assert_eq!(h.mock.with(|s| (s.forgets.clone(), s.roots.len())), (vec!["dir:Music".to_string()], 0));
    assert!(h.player.host.library.rescan("dir:Music") < 0);
    h.player.host.library.reconnect("usb:x");
    assert_eq!(h.mock.with(|s| s.reconnects.clone()), vec!["usb:x".to_string()]);
}

#[test]
fn without_the_capability_folders_say_so() {
    let mut h = Harness::with(|s| s.caps = sys::caps::CANVAS);
    h.player.request_folder();
    assert!(h.mock.with(|s| s.folder_requests.is_empty()));
    assert!(h.player.host.library().is_none());
}

#[test]
fn the_app_builds_its_library_from_what_the_shell_lists() {
    let mut h = Harness::new();
    h.run_ms(50);
    let names = ["a.wav", "b.wav", "c.wav"];
    h.mock.with(|s| {
        s.add_root("dir:Music", "Music", true);
        let mut entries = Vec::new();
        for n in names {
            let spec = FileSpec::new(n, tone_wav(0.2));
            entries.push((spec.id.clone().unwrap(), format!("Album/{n}"), spec.data.len() as u64, 5));
            s.add_file(spec);
        }
        let refs: Vec<(&str, &str, u64, i64)> =
            entries.iter().map(|(a, b, c, d)| (a.as_str(), b.as_str(), *c, *d)).collect();
        s.set_listing("dir:Music", &refs);
    });
    listing_event(&h, "dir:Music", false);
    assert!(
        h.run_until(5000, |h| h.player.app.library().track_count() == 3),
        "tracks: {}",
        h.player.app.library().track_count()
    );
    // The index is saved through the store (and survives a restart; see the storage tests).
    assert!(h.run_until(3000, |h| h.mock.with(|s| s.kv.contains_key("library/index"))));
    assert!(h.mock.with(|s| s.logs.iter().all(|(lvl, _)| *lvl > 1)), "no warnings or errors");
}
