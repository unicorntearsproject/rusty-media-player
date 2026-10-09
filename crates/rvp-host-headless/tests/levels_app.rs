//! Crossfade and automatic level through the whole application on the headless host: the settings panel with keyboard and mouse, the
//! settings kept in storage and read back after a restart, a crossfade of queue items with the now-playing information switching in
//! the middle, the library measuring tracks in the background once the automatic level is on, and the gain of what plays.
use rvp_app::App;
use rvp_core::{AudioSettings, LevelMode};
use rvp_host::{HostClock, InputEvent, Key, Modifiers, PointerButton, RecordingNowPlaying, ScriptedLibrary};
use rvp_host_headless::{DefaultCodecs, UiHost, walk_listing};
use rvp_ui::{Action, MediaState, Mode, UiConfig};
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

fn levels() -> PathBuf {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
        let st = Command::new("bash")
            .arg(script)
            .arg(dir())
            .env("RVP_FIXTURE_SET", "levels")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    dir().join("levels")
}

struct Rig {
    host: UiHost,
    app: App,
}

impl Rig {
    fn new() -> Self {
        let mut host = UiHost::new();
        host.library = Some(ScriptedLibrary::default());
        host.now_playing = Some(RecordingNowPlaying::default());
        Self::with(host)
    }

    fn with(host: UiHost) -> Self {
        let app = App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true });
        Rig { host, app }
    }

    fn step(&mut self) {
        self.app.tick(&mut self.host);
        self.app.take_effects();
        self.host.virtual_clock().advance(16_000);
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.step();
        }
    }

    fn act(&mut self, a: Action) {
        let now = self.host.virtual_clock().now_us();
        self.app.apply(&mut self.host, a, now);
        self.app.take_effects();
    }

    fn key(&mut self, k: Key) {
        self.host.input.0.push_back(InputEvent::KeyDown {
            key: k,
            mods: Modifiers::default(),
            repeat: false,
        });
        self.app.pump(&mut self.host);
        self.app.take_effects();
    }

    fn click(&mut self, x: f32, y: f32) {
        for ev in [
            InputEvent::PointerMove { x, y },
            InputEvent::PointerDown { x, y, button: PointerButton::Primary },
            InputEvent::PointerUp { x, y, button: PointerButton::Primary },
        ] {
            self.host.input.0.push_back(ev);
            self.app.pump(&mut self.host);
        }
        self.app.take_effects();
    }

    fn open(&mut self, names: &[&str]) {
        let items: Vec<(String, String)> =
            names.iter().map(|n| (levels().join(n).to_string_lossy().into_owned(), n.to_string())).collect();
        let now = self.host.virtual_clock().now_us();
        let _ = now;
        self.app.open_items(&mut self.host, &items, false);
    }

    fn snapshot(&self) -> serde_json::Value {
        serde_json::from_str(self.app.snapshot().json()).unwrap()
    }

    fn add_folder(&mut self, sub: &str) {
        let listing = walk_listing("levels", "levels", &levels().join(sub));
        self.host.library.as_mut().unwrap().listings.push_back(listing);
    }

    fn metas(&self) -> Vec<String> {
        self.host.now_playing.as_ref().unwrap().metadata.iter().map(|m| m.title.clone()).collect()
    }
}

#[test]
fn the_settings_are_kept_in_storage_and_read_back() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.run(100);
    assert_eq!(r.app.audio_settings(), AudioSettings::default());
    r.act(Action::SetCrossfade(true));
    r.act(Action::SetCrossfadeSecs(7));
    r.act(Action::SetAutoLevel(true));
    r.act(Action::SetTargetLufs(-18));
    r.act(Action::SetLevelMode(LevelMode::Album));
    // Out-of-range values are brought into range.
    r.act(Action::SetCrossfadeSecs(99));
    r.act(Action::SetTargetLufs(5));
    let want = AudioSettings {
        crossfade: true,
        crossfade_secs: 10,
        auto_level: true,
        target_lufs: -10,
        level_mode: LevelMode::Album,
        echo_skip: true,
    };
    assert_eq!(r.app.audio_settings(), want);
    let saved = r.host.storage.0.get("settings/audio").expect("saved under settings/audio");
    assert_eq!(AudioSettings::from_text(std::str::from_utf8(saved).unwrap()), Some(want));
    // The snapshot shows them (for tests and tooling).
    r.run(50);
    let a = &r.snapshot()["audio"];
    assert_eq!(
        (a["crossfade"].as_bool(), a["crossfade_secs"].as_i64(), a["auto_level"].as_bool()),
        (Some(true), Some(10), Some(true))
    );
    assert_eq!((a["target_lufs"].as_i64(), a["level_mode"].as_str()), (Some(-10), Some("album")));
    // A restart on the same storage.
    let storage = std::mem::take(&mut r.host.storage.0);
    let mut host = UiHost::new();
    host.storage.0 = storage;
    let mut r2 = Rig::with(host);
    r2.run(100);
    assert_eq!(r2.app.audio_settings(), want);
    assert_eq!(r2.app.model().audio, want);
    // Damaged or foreign contents are ignored, the defaults stay.
    for junk in [&b"\xff\xfe\x00 not text"[..], b"", b"something else\ncrossfade=1\n"] {
        let mut host = UiHost::new();
        host.storage.0.insert("settings/audio".into(), junk.to_vec());
        let mut r3 = Rig::with(host);
        r3.run(50);
        assert_eq!(r3.app.audio_settings(), AudioSettings::default());
    }
}

#[test]
fn the_panel_is_reachable_from_the_keyboard_and_the_mouse_on_both_faces() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.run(50);
    let plain = r.snapshot();
    assert!(plain["audio_panel"].is_null());
    // U opens it; it takes the keyboard (N would be "next").
    r.key(Key::Char('u'));
    r.run(50);
    assert!(r.app.ui().audio_settings_open());
    assert!(r.snapshot()["audio_panel"].is_object());
    r.key(Key::Char('n'));
    assert!(r.app.ui().audio_settings_open());
    // Space flips the switch the focus starts on; Tab and the arrows reach the sliders and the mode.
    r.key(Key::Space);
    assert!(r.app.audio_settings().crossfade);
    r.key(Key::Other("Tab".into()));
    r.key(Key::Right);
    assert_eq!(r.app.audio_settings().crossfade_secs, 6);
    r.key(Key::Down); // echo out on skip (on by default)
    r.key(Key::Space);
    assert!(!r.app.audio_settings().echo_skip);
    r.key(Key::Down); // auto-level
    r.key(Key::Space);
    assert!(r.app.audio_settings().auto_level);
    r.key(Key::Down); // target
    r.key(Key::Left);
    assert_eq!(r.app.audio_settings().target_lufs, -15);
    r.key(Key::Down); // mode
    r.key(Key::Right);
    assert_eq!(r.app.audio_settings().level_mode, LevelMode::Album);
    r.key(Key::Escape);
    assert!(!r.app.ui().audio_settings_open());
    // The mouse does the same: the controls are where the snapshot says.
    r.act(Action::ShowAudioSettings);
    r.run(50);
    let s = r.snapshot();
    let rect = |name: &str| {
        let c = &s["audio_panel"]["controls"][name];
        (
            c["x"].as_f64().unwrap() as f32,
            c["y"].as_f64().unwrap() as f32,
            c["w"].as_f64().unwrap() as f32,
            c["h"].as_f64().unwrap() as f32,
        )
    };
    let (x, y, w, h) = rect("crossfade");
    r.click(x + w * 0.5, y + h * 0.5);
    assert!(!r.app.audio_settings().crossfade, "it was on, the click turned it off");
    let (x, y, w, h) = rect("target");
    r.click(x + w - 1.0, y + h * 0.5);
    assert_eq!(r.app.audio_settings().target_lufs, -10);
    r.click(1.0, 1.0); // outside: closes
    assert!(!r.app.ui().audio_settings_open());
    // The Library face has it too.
    r.act(Action::SetMode(Mode::Library));
    r.run(50);
    let before = r.host.surface.rgba.clone();
    r.key(Key::Char('u'));
    r.run(50);
    assert!(r.app.ui().audio_settings_open());
    assert_ne!(before, r.host.surface.rgba, "the panel is drawn over the library");
    r.key(Key::Escape);
    assert!(!r.app.ui().audio_settings_open());
}

#[test]
fn a_crossfade_of_queue_items_through_the_app_switches_the_now_playing_in_the_middle() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.act(Action::SetCrossfadeSecs(2));
    r.act(Action::SetCrossfade(true));
    r.host.audio.capture = Some(Vec::new());
    r.open(&["xf_a.flac", "xf_b.flac"]);
    let mut saw_fade = false;
    let mut switch_at = None;
    for _ in 0..2000 {
        r.step();
        saw_fade |= r.snapshot()["audio"]["crossfading"].as_bool() == Some(true);
        if switch_at.is_none() && r.metas().len() == 2 {
            switch_at = Some(r.host.virtual_clock().now_us());
        }
        if r.app.model().state == MediaState::Ended {
            break;
        }
    }
    assert_eq!(r.app.model().state, MediaState::Ended);
    assert!(saw_fade, "the snapshot says when a crossfade is under way");
    let got = r.host.audio.capture.take().unwrap();
    // Two 8 s tracks with a fade of just under 2 s overlap: 14 s and a bit, not 16.
    let secs = got.len() as f64 / 2.0 / 48_000.0;
    assert!((13.9..14.2).contains(&secs), "{secs} s");
    assert_eq!(r.metas(), ["xf_a", "xf_b"], "one now-playing update per item");
    // The title changed about a second before the first track's own end (the middle of the fade), when 7 s of it have been heard.
    let t = switch_at.unwrap() as f64 / 1e6;
    assert!(t > 6.0 && t < 8.5, "the second item became current at {t} s");
}

#[test]
fn turning_the_automatic_level_on_measures_the_library_in_the_background() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.add_folder("lib");
    r.run(2000);
    assert_eq!(r.app.library().track_count(), 4);
    // Off: only the tags were read (the tagged track), the other three wait.
    assert_eq!(r.app.library().all_tracks().iter().filter(|t| t.loudness.lufs.is_some()).count(), 1);
    r.act(Action::SetAutoLevel(true));
    let mut seen_progress = false;
    for _ in 0..4000 {
        r.step();
        seen_progress |= r.app.scan_status().is_some_and(|s| s.analysing);
        if r.app.library().all_tracks().iter().all(|t| t.loudness.lufs.is_some())
            && r.app.scan_status().is_none()
        {
            break;
        }
    }
    assert!(seen_progress, "the status shows the measurement under way");
    assert!(r.app.library().all_tracks().iter().all(|t| t.loudness.lufs.is_some()));
    // It was saved: a new app on the same storage has the figures without reading anything.
    let storage = std::mem::take(&mut r.host.storage.0);
    let mut host = UiHost::new();
    host.library = Some(ScriptedLibrary::default());
    host.storage.0 = storage;
    let mut r2 = Rig::with(host);
    r2.run(300);
    assert_eq!(r2.app.library().track_count(), 4);
    assert!(r2.app.library().all_tracks().iter().all(|t| t.loudness.lufs.is_some()));
    // Turning it off again stops the measuring (nothing is pending anyway) and keeps the figures.
    r.act(Action::SetAutoLevel(false));
    r.run(100);
    assert!(r.app.scan_status().is_none());
}

#[test]
fn what_plays_is_leveled_by_the_librarys_figures_in_track_and_album_mode() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.act(Action::SetAutoLevel(true));
    r.act(Action::SetTargetLufs(-23));
    r.add_folder("lib");
    for _ in 0..4000 {
        r.step();
        if r.app.library().track_count() == 4
            && r.app.library().all_tracks().iter().all(|t| t.loudness.lufs.is_some())
            && r.app.scan_status().is_none()
        {
            break;
        }
    }
    let track = r.app.library().all_tracks().iter().find(|t| t.path == "quiet-loud/02.flac").unwrap().clone();
    let lufs = track.loudness.lufs.unwrap();
    let album = r.app.library().loudness_hint(track.id).unwrap().album_lufs.unwrap();
    assert!((lufs + 29.35).abs() < 0.1);
    // Play it from the library and let it settle.
    r.act(Action::Lib(rvp_ui::LibAction::Play(rvp_ui::Scope::Track(track.id), rvp_ui::Enqueue::Now)));
    r.run(1500);
    let gain = r.app.model().level_gain_db.expect("auto-level is on and something plays");
    assert!((gain as f64 - (-23.0 - lufs as f64)).abs() < 0.1, "track mode: {gain} dB for {lufs} LUFS");
    // Album mode: the album's figure instead (the three tracks are 8 LU apart, so this track is quieter than its album).
    r.act(Action::SetLevelMode(LevelMode::Album));
    r.run(1500);
    let gain = r.app.model().level_gain_db.unwrap();
    assert!(
        (gain as f64 - (-23.0 - album as f64)).abs() < 0.1,
        "album mode: {gain} dB for the album at {album} LUFS"
    );
    assert!(gain < 6.5 && gain > -6.5);
    // Turning it off: no gain.
    r.act(Action::SetAutoLevel(false));
    r.run(200);
    assert_eq!(r.app.model().level_gain_db, None);
}
