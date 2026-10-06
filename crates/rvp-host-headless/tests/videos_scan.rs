//! Videos in the library: a folder of generated films is scanned through the real demuxers (titles, lengths, sizes, codecs), a poster
//! frame is decoded for each with the real decoders, a second scan reads nothing, a changed file is read again and gets a new
//! poster, a deleted one is dropped, a file that is not a video is remembered as unreadable and not retried, and the videos and
//! their posters survive a restart. Fixtures: `tools/gen-fixtures.sh`.
use rvp_app::App;
use rvp_core::CodecFactory;
use rvp_host::HostClock;
use rvp_host::ScriptedLibrary;
use rvp_host_headless::{DefaultCodecs, HeadlessHost, UiHost, walk_listing};
use rvp_library::{Library, ScanEvent, ScanReport, Scanner, Video, art_key, encode_thumb};
use rvp_ui::{Action, Enqueue, LibAction, Scope, UiConfig};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Once;

fn dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"))
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

fn fixture(name: &str) -> PathBuf {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let d = dir();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
        for (set, done) in [("core", d.join(".done")), ("m8", d.join("m8/.done"))] {
            if !done.exists() {
                let st = Command::new("bash")
                    .arg(&script)
                    .arg(&d)
                    .env("RVP_FIXTURE_SET", set)
                    .status()
                    .expect("run tools/gen-fixtures.sh");
                assert!(
                    st.success(),
                    "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)"
                );
            }
        }
    });
    dir().join(name)
}

/// The folder `name` under the fixtures, filled with films (overwritten in place, never deleted recursively):
/// `a.mp4` (h264+aac), `b.webm` (av1+opus), `c.webm` (vp9+vorbis), `sub/d.mkv` (h264+flac), `audio_only.mkv` (no picture) and
/// `corrupt.mp4` (not a film at all), plus a song and a text file that are not videos.
fn folder(name: &str) -> PathBuf {
    let d = dir().join("videos_scan").join(name);
    std::fs::create_dir_all(d.join("sub")).unwrap();
    for (from, to) in [
        ("h264_aac.mp4", "a.mp4"),
        ("av1_opus.webm", "b.webm"),
        ("vp9_vorbis.webm", "c.webm"),
        ("h264_flac.mkv", "sub/d.mkv"),
        ("m8/gap_0.mkv", "audio_only.mkv"),
    ] {
        std::fs::copy(fixture(from), d.join(to)).unwrap();
    }
    std::fs::write(d.join("corrupt.mp4"), b"this is not a film, only text pretending to be one".repeat(40))
        .unwrap();
    std::fs::write(d.join("notes.txt"), b"hello").unwrap();
    let _ = std::fs::remove_file(d.join("later.mp4")); // left by an earlier run
    d
}

/// Scan `dir` into `lib`, then make the posters, until the scanner has nothing left to do. Returns what the scan reported and how
/// many `Posters` events there were.
fn run(lib: &mut Library, dir: &Path, codecs: Option<Rc<dyn CodecFactory>>) -> (ScanReport, usize) {
    let mut host = HeadlessHost::new();
    let mut sc = ScriptedLibrary::default();
    sc.listings.push_back(walk_listing("films", "Films", dir));
    host.library = Some(sc);
    let listing = host.library.as_mut().unwrap().listings.pop_front().unwrap();
    let mut scanner = Scanner::new();
    if let Some(c) = codecs {
        scanner.set_codecs(c);
    }
    scanner.push(listing);
    let clock = host.virtual_clock();
    let (mut report, mut posters, mut saw_status) = (ScanReport::default(), 0, false);
    for _ in 0..100_000 {
        match scanner.tick(lib, &mut host) {
            ScanEvent::Finished(r) => {
                report = r;
                scanner.start_posters(lib.pending_posters());
            }
            ScanEvent::Posters => posters += 1,
            _ => {}
        }
        saw_status |= scanner.status().is_some_and(|s| s.posters);
        if !scanner.busy() {
            assert!(!saw_status || posters > 0, "the poster job reports when it has filed something");
            return (report, posters);
        }
        clock.advance(1_000);
    }
    panic!("the scan did not finish");
}

fn codecs() -> Option<Rc<dyn CodecFactory>> {
    Some(Rc::new(DefaultCodecs::default()))
}

fn by_path<'a>(lib: &'a Library, path: &str) -> &'a Video {
    lib.all_videos().iter().find(|v| v.path == path).unwrap_or_else(|| panic!("no video {path}"))
}

/// Average colour of a poster, 0..255 per channel.
fn average(lib: &Library, v: &Video) -> [f32; 3] {
    let t = lib.thumb(v.poster).unwrap_or_else(|| panic!("{} has no poster picture", v.path));
    let n = (t.rgb.len() / 3) as f32;
    let mut sum = [0f32; 3];
    for p in t.rgb.chunks_exact(3) {
        for c in 0..3 {
            sum[c] += p[c] as f32;
        }
    }
    sum.map(|s| s / n)
}

#[test]
fn a_folder_of_films_is_scanned_with_titles_lengths_codecs_and_posters() {
    if skip() {
        return;
    }
    let d = folder("scan");
    let mut lib = Library::new();
    let (report, posters) = run(&mut lib, &d, codecs());
    assert_eq!(
        report.added, 6,
        "{report:?}: four films, and two files that cannot be read (remembered all the same)"
    );
    assert_eq!(report.failed, 2, "corrupt.mp4 and the file without a picture: {report:?}");
    assert_eq!(lib.track_count(), 0, "videos are not tracks");
    assert_eq!(lib.all_videos().len(), 6);
    assert_eq!(lib.video_count(), 4);
    assert!(posters > 0);

    for (path, vcodec, acodec) in [
        ("a.mp4", "h264", "aac"),
        ("b.webm", "av1", "opus"),
        ("c.webm", "vp9", "vorbis"),
        ("sub/d.mkv", "h264", "flac"),
    ] {
        let v = by_path(&lib, path);
        assert!(!v.unreadable, "{path}");
        assert_eq!((v.vcodec.as_str(), v.acodec.as_str()), (vcodec, acodec), "{path}");
        assert_eq!((v.width, v.height), (320, 240), "{path}");
        assert!((v.duration_us - 6_000_000).abs() < 50_000, "{path}: {} us", v.duration_us);
        assert_eq!(v.size, std::fs::metadata(d.join(path)).unwrap().len(), "{path}");
        assert!(v.mtime_ms > 0 && !v.src.is_empty());
        let stem = path.rsplit('/').next().unwrap().rsplit_once('.').unwrap().0;
        assert_eq!(v.display_title(), stem, "no title tag: the file name stands in");
        assert_eq!(v.size_text(), "320x240");

        // A poster: a real frame (not black, not missing), and nothing left to make.
        assert!(v.poster != 0 && !v.poster_tried, "{path}");
        let avg = average(&lib, v);
        assert!(avg.iter().sum::<f32>() / 3.0 > 12.0, "{path}: poster is black, average {avg:?}");
        let t = lib.thumb(v.poster).unwrap();
        assert!(t.w > 0 && t.h > 0 && t.w.max(t.h) <= 320, "{path}: {}x{}", t.w, t.h);
    }
    assert!(lib.pending_posters().is_empty());

    // The odd ones: remembered, hidden, never given a poster.
    for p in ["corrupt.mp4", "audio_only.mkv"] {
        let v = by_path(&lib, p);
        assert!(v.unreadable && v.poster == 0, "{p}");
    }
    assert!(lib.all_videos().iter().all(|v| v.path != "notes.txt"));
    // Order by title and by length.
    let names: Vec<&str> = lib
        .sorted_videos(rvp_library::VideoSort::Title, true)
        .iter()
        .map(|&i| lib.video(i).unwrap().file_name())
        .collect();
    assert_eq!(names, ["a.mp4", "b.webm", "c.webm", "d.mkv"]);
}

#[test]
fn without_decoders_the_headers_are_read_and_posters_wait() {
    if skip() {
        return;
    }
    let d = folder("nodecoders");
    let mut lib = Library::new();
    let (report, posters) = run(&mut lib, &d, None);
    assert_eq!((report.added, lib.video_count(), posters), (6, 4, 0));
    assert_eq!(lib.pending_posters().len(), 4, "they can be made later");
    assert!(lib.all_videos().iter().all(|v| v.poster == 0));
}

#[test]
fn a_second_scan_reads_nothing_and_the_unreadable_are_not_retried() {
    if skip() {
        return;
    }
    let d = folder("rescan");
    let mut lib = Library::new();
    run(&mut lib, &d, codecs());
    let ids: Vec<u32> = lib.all_videos().iter().map(|v| v.id).collect();
    let posters: Vec<u64> = lib.all_videos().iter().map(|v| v.poster).collect();
    let (report, made) = run(&mut lib, &d, codecs());
    assert_eq!(
        (report.added, report.changed, report.removed, report.failed, report.unchanged),
        (0, 0, 0, 0, 6),
        "{report:?}"
    );
    assert_eq!(made, 0, "no poster is made twice");
    assert_eq!(lib.all_videos().iter().map(|v| v.id).collect::<Vec<_>>(), ids);
    assert_eq!(lib.all_videos().iter().map(|v| v.poster).collect::<Vec<_>>(), posters);
}

#[test]
fn a_changed_film_is_read_again_and_a_deleted_one_is_dropped() {
    if skip() {
        return;
    }
    let d = folder("changes");
    let mut lib = Library::new();
    run(&mut lib, &d, codecs());
    let (id_a, old_poster) = {
        let v = by_path(&lib, "a.mp4");
        (v.id, v.poster)
    };
    let _ = lib.take_dropped_art();

    // a.mp4 becomes another film (different size); b.webm goes; corrupt.mp4 becomes a real film.
    std::thread::sleep(std::time::Duration::from_millis(20)); // a new modification time
    std::fs::copy(fixture("av1_opus_60s.webm"), d.join("a.mp4")).unwrap();
    std::fs::remove_file(d.join("b.webm")).unwrap();
    std::fs::copy(fixture("h264_aac_faststart.mp4"), d.join("corrupt.mp4")).unwrap();
    let (report, _) = run(&mut lib, &d, codecs());
    assert_eq!((report.removed, report.changed, report.added), (1, 2, 0), "{report:?}");
    assert!(lib.all_videos().iter().all(|v| v.path != "b.webm"));
    let a = by_path(&lib, "a.mp4");
    assert_eq!(a.id, id_a, "the same video keeps its id");
    assert_eq!(a.vcodec, "av1");
    assert!(a.duration_us > 50_000_000, "{}", a.duration_us);
    assert!(a.poster != 0 && a.poster != old_poster, "a new poster for the new file");
    assert!(lib.take_dropped_art().contains(&old_poster), "the old picture can go from storage");
    let c = by_path(&lib, "corrupt.mp4");
    assert!(!c.unreadable && c.poster != 0, "a file that was bad is read again once it changes");
    assert_eq!(lib.video_count(), 4);
}

#[test]
fn videos_and_posters_survive_a_restart() {
    if skip() {
        return;
    }
    let d = folder("restart");
    let mut lib = Library::new();
    run(&mut lib, &d, codecs());
    assert!(lib.index_dirty() && lib.videos_dirty());
    let index = lib.save_index();
    let videos = lib.save_videos();
    assert!(!lib.videos_dirty());
    let thumbs: Vec<(u64, Vec<u8>)> =
        lib.take_unsaved_art().into_iter().map(|(id, t)| (id, encode_thumb(&t))).collect();
    assert_eq!(thumbs.len(), 4, "one picture per film");
    assert!(thumbs.iter().all(|(id, _)| !art_key(*id).is_empty()));

    // A new session: the index first (it names the folder), then the videos, then the pictures a view asks for.
    let mut again = Library::load_index(&index).unwrap();
    again.load_videos(&videos).unwrap();
    assert_eq!(again.all_videos().len(), 6);
    assert_eq!(again.video_count(), 4);
    for v in again.all_videos() {
        let was = lib.video(v.id).unwrap();
        assert_eq!(
            (&v.path, v.size, v.mtime_ms, v.duration_us),
            (&was.path, was.size, was.mtime_ms, was.duration_us)
        );
        assert_eq!(
            (&v.vcodec, &v.acodec, v.poster, v.unreadable),
            (&was.vcodec, &was.acodec, was.poster, was.unreadable)
        );
        assert!(v.src.is_empty(), "not openable until the folder is listed again");
    }
    assert!(again.pending_posters().is_empty(), "nothing to make, and nothing to open them with yet");
    for (id, bytes) in &thumbs {
        assert!(again.load_thumb(*id, bytes));
    }
    for v in again.all_videos().iter().filter(|v| v.poster != 0) {
        assert!(average(&again, v).iter().sum::<f32>() > 30.0, "{}", v.path);
    }
    assert!(again.used_art().len() >= 4, "pictures in use are kept when unused ones are pruned");

    // Scanning the folder again reads nothing and gives the films their way to open.
    let (report, made) = run(&mut again, &d, codecs());
    assert_eq!((report.added, report.changed, report.removed, report.unchanged), (0, 0, 0, 6), "{report:?}");
    assert_eq!(made, 0);
    assert!(by_path(&again, "a.mp4").src.ends_with("a.mp4"));
    // The ids a new session hands out continue past the old ones.
    let top = again.all_videos().iter().map(|v| v.id).max().unwrap();
    std::fs::write(d.join("later.mp4"), b"x".repeat(100)).unwrap();
    run(&mut again, &d, codecs());
    assert!(by_path(&again, "later.mp4").id > top);
}

// ---- the application: it keeps the videos, makes their posters, and marks where each one was left --------------------------------

struct Rig {
    host: UiHost,
    app: App,
}

impl Rig {
    fn new(host: UiHost) -> Self {
        let mut host = host;
        host.library.get_or_insert_with(ScriptedLibrary::default);
        Rig { host, app: App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true }) }
    }

    fn step(&mut self) {
        self.app.tick(&mut self.host);
        self.host.virtual_clock().advance(16_000);
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.step();
        }
    }

    /// Scan `dir` and run until the posters are made.
    fn scan(&mut self, dir: &Path) {
        let l = walk_listing("films", "Films", dir);
        self.host.library.as_mut().unwrap().listings.push_back(l);
        for _ in 0..6000 {
            self.step();
            let lib = self.app.library();
            if lib.video_count() >= 4 && self.app.scan_status().is_none() && lib.pending_posters().is_empty()
            {
                break;
            }
        }
        self.run(300);
        assert_eq!(self.app.library().video_count(), 4);
    }

    fn id(&self, path: &str) -> u32 {
        by_path(self.app.library(), path).id
    }

    fn act(&mut self, a: Action) {
        let now = self.host.virtual_clock().now_us();
        self.app.apply(&mut self.host, a, now);
    }
}

#[test]
fn the_app_scans_makes_posters_saves_them_and_loads_them_next_time() {
    if skip() {
        return;
    }
    let d = folder("app_posters");
    let mut r = Rig::new(UiHost::new());
    r.scan(&d);
    let lib = r.app.library();
    assert!(
        lib.all_videos().iter().filter(|v| !v.unreadable).all(|v| v.poster != 0),
        "every film has a poster"
    );
    assert!(!lib.videos_dirty() && !lib.index_dirty(), "everything is saved");
    let st = &r.host.storage.0;
    assert!(st.contains_key(rvp_library::VIDEOS_KEY), "the videos are saved");
    for v in lib.all_videos().iter().filter(|v| v.poster != 0) {
        assert!(
            st.get(&art_key(v.poster)).is_some_and(|b| !b.is_empty()),
            "the poster of {} is saved",
            v.path
        );
    }
    let tried = lib.all_videos().iter().filter(|v| v.unreadable).count();
    assert_eq!(tried, 2);

    // A restart: the videos come back from storage (before any listing), with their posters, and no poster is made again.
    let mut host = UiHost::new();
    host.storage.0 = r.host.storage.0.clone();
    let mut r2 = Rig::new(host);
    r2.run(600);
    let lib2 = r2.app.library();
    assert_eq!(lib2.video_count(), 4);
    assert!(lib2.all_videos().iter().filter(|v| !v.unreadable).all(|v| v.poster != 0));
    assert!(lib2.pending_posters().is_empty());
    for v in lib2.all_videos().iter().filter(|v| v.poster != 0) {
        assert!(lib2.thumb(v.poster).is_some(), "{}: the picture is loaded from storage", v.path);
    }
    // They cannot be played until their folder is listed again.
    assert!(lib2.all_videos().iter().all(|v| v.src.is_empty()));
    r2.scan(&d);
    assert!(r2.app.library().all_videos().iter().filter(|v| !v.unreadable).all(|v| !v.src.is_empty()));
    assert_eq!(by_path(r2.app.library(), "a.mp4").src, d.join("a.mp4").to_string_lossy());
}

#[test]
fn a_video_remembers_where_it_was_left_and_the_marker_follows() {
    if skip() {
        return;
    }
    let d = folder("app_resume");
    // A long film: a position within five seconds of the end counts as finished, so the six-second fixtures never resume.
    std::fs::copy(fixture("av1_opus_60s.webm"), d.join("a.mp4")).unwrap();
    let mut r = Rig::new(UiHost::new());
    r.scan(&d);
    assert!(r.app.video_resume_fractions().is_empty());
    let id = r.id("a.mp4");

    r.act(Action::Lib(LibAction::Play(Scope::Video(id), Enqueue::Now)));
    r.run(3_000);
    r.act(Action::PlayPause);
    r.run(200);
    // Saved under the file name (not the title shown in the queue), and the marker is there.
    r.app.save_state(&mut r.host);
    let key = "resume:a.mp4";
    let raw = r.host.storage.0.get(key).expect("the position is saved under the file name");
    let ms = i64::from_le_bytes(raw.as_slice().try_into().unwrap());
    assert!((2_000..4_500).contains(&ms), "{ms}");
    assert!(!r.host.storage.0.contains_key("resume:a"));
    let f = r.app.video_resume_fractions()[&id];
    assert!((f - ms as f32 / 60_000.0).abs() < 0.01, "marker {f} for {ms} ms of 60 s");
    assert!(!r.app.video_resume_fractions().contains_key(&r.id("b.webm")));

    // A restart reads the markers from storage once the folder is listed.
    let mut host = UiHost::new();
    host.storage.0 = r.host.storage.0.clone();
    let mut r2 = Rig::new(host);
    r2.run(300);
    let f2 = r2.app.video_resume_fractions()[&id];
    assert!((f2 - f).abs() < 0.001, "{f2} vs {f}");

    // Playing it from the library resumes there; so does opening the same file by hand (one rule, one key).
    r2.scan(&d);
    r2.act(Action::Lib(LibAction::Play(Scope::Video(id), Enqueue::Now)));
    r2.run(600);
    assert!(
        r2.app.model().position_us / 1000 > ms - 300,
        "resumed near {ms} ms, at {} us",
        r2.app.model().position_us
    );
    let path = d.join("a.mp4").to_string_lossy().into_owned();
    let mut host = UiHost::new();
    host.storage.0 = r.host.storage.0.clone();
    let mut r3 = Rig::new(host);
    r3.app.open_items(&mut r3.host, &[(path, "a.mp4".to_string())], false);
    r3.run(600);
    assert!(
        r3.app.model().position_us / 1000 > ms - 300,
        "opened by hand: {} us",
        r3.app.model().position_us
    );

    // Watching to the end clears the marker.
    r2.act(Action::SeekFraction(0.99));
    r2.run(2_000);
    r2.app.save_state(&mut r2.host);
    assert!(!r2.app.video_resume_fractions().contains_key(&id), "a finished film has no marker");
}

#[test]
fn a_queued_video_comes_back_after_a_restart() {
    if skip() {
        return;
    }
    let d = folder("app_queue");
    let mut r = Rig::new(UiHost::new());
    r.scan(&d);
    let (a, c) = (r.id("a.mp4"), r.id("c.webm"));
    r.act(Action::Lib(LibAction::Play(Scope::Video(a), Enqueue::Now)));
    r.act(Action::Lib(LibAction::Play(Scope::Video(c), Enqueue::Append)));
    r.run(500);
    assert_eq!(r.app.playlist().len(), 2);
    r.app.save_state(&mut r.host);

    // The host's ids do not last (a browser): the library's own source for each video is used.
    let mut host = UiHost::new();
    host.stable = false;
    host.storage.0 = r.host.storage.0.clone();
    let mut r2 = Rig::new(host);
    r2.scan(&d);
    r2.run(600);
    let items = r2.app.playlist().items();
    assert_eq!(items.len(), 2, "both videos are restored");
    assert_eq!(items.iter().map(|i| i.track).collect::<Vec<_>>(), [Some(a), Some(c)]);
    assert!(items.iter().all(|i| !i.source.is_empty()), "their source comes from the library");
}
