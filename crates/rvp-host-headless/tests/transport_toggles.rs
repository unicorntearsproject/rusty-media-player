//! Shuffle, repeat and the visualizer's way back, through the whole app on the headless host: every control that shows or
//! changes shuffle and repeat (the player bar's and the library bar's buttons, the keys, the playlist menu, the accessibility
//! state in the snapshot) reads the one playlist state, and the visualizer returns to the view it was opened from.
use rvp_app::App;
use rvp_host::{HostClock, InputEvent, Key, Modifiers, PointerButton};
use rvp_host_headless::{DefaultCodecs, FileSource, UiHost};
use rvp_ui::{Mode, UiConfig, View};
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
        if !dir.join(".done").exists() {
            let st = Command::new("bash")
                .arg(root.join("tools/gen-fixtures.sh"))
                .arg(&dir)
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
    /// A rig with a long video playing, on the Player face.
    fn new() -> Self {
        let mut host = UiHost::new();
        let app = App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true });
        let src = FileSource::open(&fixture("av1_opus_60s.webm")).unwrap();
        let mut app = app;
        app.open(&mut host, src);
        let mut r = Rig { host, app };
        r.run(800);
        r
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.app.tick(&mut self.host);
            clock.advance(16_000);
        }
    }

    fn send(&mut self, ev: InputEvent) {
        self.host.input.0.push_back(ev);
        self.app.pump(&mut self.host);
    }

    fn key(&mut self, k: Key) {
        self.send(InputEvent::KeyDown { key: k, mods: Modifiers::default(), repeat: false });
        self.run(50);
    }

    fn click(&mut self, x: f32, y: f32) {
        self.send(InputEvent::PointerMove { x, y });
        self.send(InputEvent::PointerDown { x, y, button: PointerButton::Primary });
        self.send(InputEvent::PointerUp { x, y, button: PointerButton::Primary });
        self.run(50);
    }

    fn snap(&self) -> serde_json::Value {
        serde_json::from_str(self.app.snapshot().json()).unwrap()
    }

    /// Click a button of the current face's bar, found in the snapshot.
    fn click_button(&mut self, name: &str) {
        // Bring the controls up first (they hide while playing) and let them settle.
        self.send(InputEvent::PointerMove { x: 600.0, y: 600.0 });
        self.run(400);
        let s = self.snap();
        let r =
            if self.app.ui().mode() == Mode::Library { &s["lib"]["bar"][name] } else { &s["buttons"][name] };
        let (x, y) = (
            (r["x"].as_f64().unwrap() + r["w"].as_f64().unwrap() / 2.0) as f32,
            (r["y"].as_f64().unwrap() + r["h"].as_f64().unwrap() / 2.0) as f32,
        );
        self.click(x, y);
    }

    fn texts(&self) -> (String, String, bool) {
        let s = self.snap();
        (
            s["transport"]["shuffle"]["label"].as_str().unwrap().to_string(),
            s["transport"]["repeat"]["label"].as_str().unwrap().to_string(),
            s["transport"]["shuffle"]["pressed"].as_bool().unwrap(),
        )
    }

    /// The labels the playlist menu shows now.
    fn menu_labels(&mut self) -> Vec<String> {
        self.key(Key::Char('q'));
        let labels = self.app.ui().menu_rows().into_iter().map(|(l, ..)| l).collect();
        self.key(Key::Escape);
        labels
    }
}

#[test]
fn shuffle_and_repeat_follow_one_state_whichever_control_changes_it() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    assert_eq!(r.texts(), ("Shuffle: off".into(), "Repeat: off".into(), false));
    assert_eq!(r.snap()["transport"]["repeat"]["state"], "off");

    // The player bar's buttons.
    r.click_button("Repeat");
    assert_eq!(r.texts().1, "Repeat: all");
    assert_eq!(r.snap()["repeat"], 1);
    r.click_button("Shuffle");
    assert_eq!(r.texts(), ("Shuffle: on".into(), "Repeat: all".into(), true));
    // The playlist menu says the same.
    let menu = r.menu_labels();
    assert!(
        menu.contains(&"Repeat: all".to_string()) && menu.contains(&"Shuffle: on".to_string()),
        "{menu:?}"
    );

    // The keys.
    r.key(Key::Char('r'));
    assert_eq!(r.texts().1, "Repeat: one");
    assert_eq!(r.snap()["transport"]["repeat"]["state"], "one");
    r.key(Key::Char('z'));
    assert_eq!(r.texts().0, "Shuffle: off");

    // The library bar's buttons act on the same state and show it.
    r.key(Key::Char('b'));
    assert_eq!(r.app.ui().mode(), Mode::Library);
    assert_eq!(r.texts(), ("Shuffle: off".into(), "Repeat: one".into(), false));
    r.click_button("Repeat"); // one -> off
    r.click_button("Shuffle");
    assert_eq!(r.texts(), ("Shuffle: on".into(), "Repeat: off".into(), true));
    // And back on the player face the buttons are the same state.
    r.key(Key::Char('b'));
    let menu = r.menu_labels();
    assert!(
        menu.contains(&"Repeat: off".to_string()) && menu.contains(&"Shuffle: on".to_string()),
        "{menu:?}"
    );
    assert_eq!(r.texts(), ("Shuffle: on".into(), "Repeat: off".into(), true));
}

#[test]
fn the_visualizer_button_and_key_go_back_to_where_it_was_opened_from() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    // From the Player face, by the key and back by the key, Escape and the button.
    for leave in 0..3 {
        r.key(Key::Char('v'));
        assert_eq!((r.app.ui().mode(), r.app.ui().lib_state().view()), (Mode::Library, View::Visualizer));
        let s = r.snap();
        assert_eq!(s["lib"]["viz_return"]["mode"], "player");
        match leave {
            0 => r.key(Key::Char('v')),
            1 => r.key(Key::Escape),
            _ => r.click_button("VizView"),
        }
        assert_eq!(r.app.ui().mode(), Mode::Player, "leave {leave}");
        assert!(r.snap()["lib"]["viz_return"].is_null());
    }
    // From the library's queue; when the queue was emptied meanwhile (the video keeps playing) it is the player.
    r.key(Key::Char('b'));
    r.key(Key::Char('5'));
    assert_eq!(r.app.ui().lib_state().view(), View::Queue);
    r.key(Key::Char('v'));
    r.key(Key::Char('v'));
    assert_eq!(r.app.ui().lib_state().view(), View::Queue);
    r.key(Key::Char('v'));
    r.app.apply(&mut r.host, rvp_ui::Action::ClearPlaylist, 0);
    r.run(100);
    r.key(Key::Escape);
    assert_eq!(r.app.ui().mode(), Mode::Player);
}
