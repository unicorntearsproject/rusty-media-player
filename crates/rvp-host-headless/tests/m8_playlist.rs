//! M8: the playlist through the whole application: gapless chaining, next/previous, repeat, removal, and the
//! resume position, in virtual time.
use rvp_app::App;
use rvp_host::{Host, HostClock, InputEvent, Key, Modifiers};
use rvp_host_headless::{DefaultCodecs, UiHost};
use rvp_ui::{Action, MediaState, UiConfig};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Once;

fn fixture(name: &str) -> String {
    static ONCE: Once = Once::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir: PathBuf =
        std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"));
    ONCE.call_once(|| {
        for set in ["core", "m8"] {
            if set == "core" && dir.join(".done").exists() {
                continue;
            }
            let st = Command::new("bash")
                .arg(root.join("tools/gen-fixtures.sh"))
                .arg(&dir)
                .env("RVP_FIXTURE_SET", set)
                .status()
                .expect("run gen-fixtures.sh");
            assert!(
                st.success(),
                "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)"
            );
        }
    });
    dir.join(name).to_string_lossy().into_owned()
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

struct Rig {
    host: UiHost,
    app: App,
}

impl Rig {
    fn new() -> Self {
        let mut host = UiHost::new();
        host.audio.capture = Some(Vec::new());
        let app = App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true });
        Self { host, app }
    }

    /// With a host that is kept (storage survives), a fresh app: a page reload.
    fn reload(self) -> Self {
        let app = App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true });
        Self { host: self.host, app }
    }

    fn open(&mut self, names: &[&str]) {
        let items: Vec<(String, String)> = names
            .iter()
            .map(|n| (fixture(n), Path::new(n).file_name().unwrap().to_string_lossy().into_owned()))
            .collect();
        self.app.open_items(&mut self.host, &items, false);
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.app.tick(&mut self.host);
            clock.advance(16_000);
        }
    }

    fn act(&mut self, a: Action) {
        let now = self.host.clock().now_us();
        self.app.apply(&mut self.host, a, now);
        self.run(20);
    }

    fn key(&mut self, c: char) {
        self.host.input.0.push_back(InputEvent::KeyDown {
            key: Key::Char(c),
            mods: Modifiers::default(),
            repeat: false,
        });
        self.app.pump(&mut self.host);
    }

    fn title(&self) -> String {
        self.app.model().title.clone()
    }

    fn pos_s(&self) -> f64 {
        self.app.model().position_us as f64 / 1e6
    }
}

const CHAIN: [&str; 3] = ["m8/gap_0.mkv", "m8/gap_1.mkv", "m8/gap_2.mkv"];

#[test]
fn three_items_play_back_to_back_without_a_gap() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.open(&CHAIN);
    assert_eq!(r.app.playlist().len(), 3);
    let mut titles = Vec::new();
    let clock = r.host.virtual_clock();
    let mut last = String::new();
    while clock.now_us() < 8_000_000 {
        r.app.tick(&mut r.host);
        if r.title() != last {
            last = r.title();
            titles.push((clock.now_us(), last.clone()));
        }
        clock.advance(16_000);
        if r.app.model().state == MediaState::Ended {
            break;
        }
    }
    let names: Vec<&str> = titles.iter().map(|t| t.1.as_str()).collect();
    assert_eq!(names, ["gap_0.mkv", "gap_1.mkv", "gap_2.mkv"], "{titles:?}");
    assert_eq!(r.app.model().state, MediaState::Ended);
    // The audio that reached the sink is the three pieces joined: 6 s, no gap, nothing doubled.
    let frames = r.host.audio.capture.as_ref().unwrap().len() / 2;
    assert_eq!(frames, 288_000, "{frames} frames");
    // And it was heard in about 6 s of virtual time (a gap or a restart would take longer).
    assert!((clock.now_us() - 6_000_000).abs() < 400_000, "{} us", clock.now_us());
}

#[test]
fn next_and_previous_move_through_the_list() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.open(&CHAIN);
    r.run(800);
    assert_eq!(r.title(), "gap_0.mkv");
    r.key('n');
    r.run(600);
    assert_eq!(r.title(), "gap_1.mkv");
    // Soon after the start, previous goes to the previous item...
    r.key('p');
    r.run(600);
    assert_eq!(r.title(), "gap_0.mkv");
    // ...but once an item has played for a while it restarts that item instead.
    r.act(Action::PlayItem(r.app.playlist().items()[1].id));
    r.run(3_800);
    assert!(r.pos_s() > 3.0 || r.app.model().state == MediaState::Ended || r.title() != "gap_1.mkv");
    r.act(Action::SeekFraction(0.8));
    r.run(300);
    r.key('p');
    r.run(500);
    assert_eq!(r.title(), "gap_1.mkv", "restarted, not moved back");
    assert!(r.pos_s() < 1.5, "{}", r.pos_s());
    // Next at the end of the list does nothing but say so.
    r.act(Action::PlayItem(r.app.playlist().items()[2].id));
    r.run(300);
    r.key('n');
    r.run(300);
    assert_eq!(r.title(), "gap_2.mkv");
}

#[test]
fn repeat_one_loops_the_item_gaplessly_and_repeat_all_wraps() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.open(&["m8/gap_0.mkv"]);
    r.act(Action::CycleRepeat); // all
    r.act(Action::CycleRepeat); // one
    assert_eq!(r.app.model().repeat, 2);
    r.run(5_500); // a 2 s item, played about 2.7 times
    assert_eq!(r.app.model().state, MediaState::Playing);
    assert_eq!(r.title(), "gap_0.mkv");
    let frames = r.host.audio.capture.as_ref().unwrap().len() / 2;
    assert!(frames > 48_000 * 5 - 20_000, "{frames} frames written");
    // Repeat all over three items wraps to the first.
    let mut r = Rig::new();
    r.open(&CHAIN);
    r.act(Action::CycleRepeat);
    assert_eq!(r.app.model().repeat, 1);
    r.run(6_700);
    assert_eq!(r.app.model().state, MediaState::Playing);
    assert_eq!(r.title(), "gap_0.mkv", "back at the first item after the last");
}

#[test]
fn removing_and_clearing_items() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.open(&CHAIN);
    r.run(500);
    let ids: Vec<u32> = r.app.playlist().items().iter().map(|i| i.id).collect();
    r.act(Action::MoveItem(ids[2], -1));
    let order: Vec<u32> = r.app.playlist().items().iter().map(|i| i.id).collect();
    assert_eq!(order, [ids[0], ids[2], ids[1]]);
    // The item that plays next follows the new order.
    r.act(Action::Next);
    r.run(400);
    assert_eq!(r.title(), "gap_2.mkv");
    // Removing the playing item moves on to the one after it.
    r.act(Action::RemoveItem(ids[2]));
    r.run(500);
    assert_eq!(r.title(), "gap_1.mkv");
    assert_eq!(r.app.playlist().len(), 2);
    // Removing the last one stops playback.
    r.act(Action::RemoveItem(ids[1]));
    assert_eq!(r.title(), "");
    r.act(Action::ClearPlaylist);
    assert!(r.app.playlist().is_empty());
}

#[test]
fn a_file_that_cannot_open_is_skipped() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    let items = vec![
        (fixture("m8/gap_0.mkv"), "gap_0.mkv".to_string()),
        ("/nonexistent/file.mkv".to_string(), "missing.mkv".to_string()),
        (fixture("m8/gap_2.mkv"), "gap_2.mkv".to_string()),
    ];
    r.app.open_items(&mut r.host, &items, false);
    r.run(5_000);
    assert_eq!(r.title(), "gap_2.mkv", "the missing file was skipped");
    assert_eq!(r.app.playlist().len(), 2, "and dropped from the list");
}

#[test]
fn an_early_position_and_one_just_before_the_end_resume_and_the_last_stretch_does_not() {
    if skip() {
        return;
    }
    // 60 s file: 3 s in resumes (the old rule skipped anything under 5 s), 6 s from the end resumes, 3 s from the end restarts.
    for (fraction, want) in [(0.05, Some(3.0)), (0.90, Some(54.0)), (0.95, None)] {
        let mut r = Rig::new();
        r.open(&["av1_opus_60s.webm"]);
        r.run(500);
        r.act(Action::SeekFraction(fraction));
        r.run(700);
        r.app.save_state(&mut r.host);
        let mut r = r.reload();
        r.open(&["av1_opus_60s.webm"]);
        r.run(1_500);
        match want {
            Some(t) => assert!((r.pos_s() - t).abs() < 2.5, "{fraction}: resumed at {}, want {t}", r.pos_s()),
            None => assert!(r.pos_s() < 2.5, "{fraction}: restarted at {}", r.pos_s()),
        }
    }
}

#[test]
fn an_audio_file_that_is_opened_directly_resumes_too() {
    if skip() {
        return;
    }
    // Only a song played from the library starts from the top: a file opened or dropped resumes, whatever it is.
    let mut r = Rig::new();
    r.open(&["levels/music.flac"]);
    r.run(500);
    r.act(Action::SeekFraction(0.5));
    r.run(700);
    r.app.save_state(&mut r.host);
    let mut r = r.reload();
    r.open(&["levels/music.flac"]);
    r.run(1_500);
    assert!((r.pos_s() - 15.0).abs() < 2.5, "resumed at {}", r.pos_s());
}

#[test]
fn the_position_is_remembered_across_a_reload() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.open(&["av1_opus_60s.webm"]);
    r.run(500);
    r.act(Action::SeekFraction(0.5));
    r.run(700);
    assert!((r.pos_s() - 30.0).abs() < 1.5, "{}", r.pos_s());
    r.app.save_state(&mut r.host);
    let mut r = r.reload();
    r.open(&["av1_opus_60s.webm"]);
    r.run(1_500);
    assert!((r.pos_s() - 30.0).abs() < 2.5, "resumed at {}", r.pos_s());
    // A different file starts from the beginning.
    r.open(&["av1_opus.webm"]);
    r.run(500);
    assert!(r.pos_s() < 1.0);
    // Watching to the end forgets the position.
    let mut r = Rig::new();
    r.open(&["av1_opus.webm"]);
    r.run(500);
    r.act(Action::SeekFraction(0.99));
    r.run(1_500);
    r.app.save_state(&mut r.host);
    let mut r = r.reload();
    r.open(&["av1_opus.webm"]);
    r.run(500);
    assert!(r.pos_s() < 1.0, "restarted at {}", r.pos_s());
}
