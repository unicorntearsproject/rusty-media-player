//! The whole application (session + UI + input) on the headless host, in virtual time: open a file, watch the
//! surface change, drive it with scripted keyboard and pointer input.
use rvp_app::{App, Effect};
use rvp_host::{HostClock, InputEvent, Key, Modifiers, PointerButton};
use rvp_host_headless::{DefaultCodecs, FileSource, UiHost};
use rvp_ui::{MediaState, UiConfig};
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
    fn new() -> Self {
        let host = UiHost::new();
        let codecs = DefaultCodecs { stall: None, clock: None };
        // reduce_motion makes fades instant, so the tests do not depend on animation timing.
        let app = App::new(Rc::new(codecs), UiConfig { reduce_motion: true });
        Self { host, app }
    }

    fn open(&mut self, name: &str) {
        let src = FileSource::open(&fixture(name)).unwrap();
        self.app.open(&mut self.host, src);
    }

    /// Run for `ms` of virtual time in 16 ms steps (a 60 Hz display).
    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            if self.app.tick(&mut self.host) {
                self.host.presents += 1;
            }
            clock.advance(16_000);
        }
    }

    fn send(&mut self, ev: InputEvent) {
        self.host.input.0.push_back(ev);
        self.app.pump(&mut self.host);
    }

    fn key(&mut self, k: Key) {
        self.send(InputEvent::KeyDown { key: k, mods: Modifiers::default(), repeat: false });
    }

    fn click(&mut self, x: f32, y: f32) {
        self.send(InputEvent::PointerMove { x, y });
        self.send(InputEvent::PointerDown { x, y, button: PointerButton::Primary });
        self.send(InputEvent::PointerUp { x, y, button: PointerButton::Primary });
    }

    fn state(&self) -> MediaState {
        self.app.model().state
    }

    fn pos_s(&self) -> f64 {
        self.app.model().position_us as f64 / 1e6
    }

    /// RGB of the centre of the picture area.
    fn picture_hash(&self) -> u64 {
        let fb = self.app.framebuffer();
        let (w, h) = (fb.width as usize, fb.height as usize);
        let mut hsh = 0xcbf2_9ce4_8422_2325u64;
        for y in h / 5..h / 2 {
            for x in w / 3..w * 2 / 3 {
                for c in 0..3 {
                    hsh = (hsh ^ fb.pixels[(y * w + x) * 4 + c] as u64).wrapping_mul(0x100_0000_01b3);
                }
            }
        }
        hsh
    }
}

#[test]
fn idle_screen_then_open_plays_and_paints_the_picture() {
    if skip() {
        return;
    }
    let mut rig = Rig::new();
    rig.run(100);
    assert_eq!(rig.state(), MediaState::Idle);
    assert_eq!(rig.host.surface.rgba.len(), 1280 * 720 * 4, "the empty screen was presented");
    rig.open("av1_opus.webm");
    rig.run(1000);
    assert_eq!(rig.state(), MediaState::Playing);
    let a = rig.picture_hash();
    rig.run(500);
    let b = rig.picture_hash();
    assert_ne!(a, b, "the picture moves");
    assert!(rig.host.video.count > 10, "frames reached the sink");
    let pos = rig.pos_s();
    assert!(pos > 1.0 && pos < 2.2, "position {pos}");
    // Plays to the end and stops there.
    rig.run(6000);
    assert_eq!(rig.state(), MediaState::Ended);
    assert!((rig.pos_s() - 6.0).abs() < 0.2);
}

#[test]
fn keyboard_and_pointer_drive_the_player() {
    if skip() {
        return;
    }
    let mut rig = Rig::new();
    rig.open("av1_opus_60s.webm");
    rig.run(800);
    assert_eq!(rig.state(), MediaState::Playing);

    rig.key(Key::Space);
    rig.run(200);
    assert_eq!(rig.state(), MediaState::Paused);
    let p = rig.pos_s();
    let frozen = rig.picture_hash();
    rig.run(500);
    assert_eq!(rig.pos_s(), p, "paused position is frozen");
    assert_eq!(rig.picture_hash(), frozen, "the last frame stays");

    // A click on the seek bar.
    let layout = rig.app.ui().layout(rig.app.model());
    let (sx, sy) = (layout.seek_track.x + layout.seek_track.w * 0.5, layout.seek_track.cy());
    rig.click(sx, sy);
    rig.run(300);
    assert!((rig.pos_s() - 30.0).abs() < 1.0, "landed at {}", rig.pos_s());
    assert_ne!(rig.picture_hash(), frozen, "the preview shows the new position");

    // Arrow keys, then volume and mute.
    let before = rig.pos_s();
    rig.key(Key::Right);
    assert!((rig.pos_s() - (before + 5.0)).abs() < 0.3);
    rig.key(Key::Char('m'));
    assert!(rig.app.model().muted);
    rig.key(Key::Down);
    assert!((rig.app.model().volume - 0.95).abs() < 0.001);

    // Speed.
    rig.key(Key::Char(']'));
    assert_eq!(rig.app.model().rate, 1.25);
    rig.key(Key::Char('\\'));
    assert_eq!(rig.app.model().rate, 1.0);

    // The right-click menu has the same actions; a row click runs one.
    rig.send(InputEvent::PointerDown { x: 600.0, y: 300.0, button: PointerButton::Secondary });
    let json = rig.app.snapshot().json().to_string();
    assert!(json.contains("\"menu_open\":true") && json.contains("Open file"), "{json}");
    let rows = rig.app.ui().menu_rows();
    let open = rows.iter().find(|r| r.0.starts_with("Open file")).unwrap().1;
    rig.click(open.cx(), open.cy());
    assert!(!rig.app.ui().menu_open());
    assert_eq!(rig.app.take_effects(), [Effect::PickFile]);

    // O asks for the picker too.
    rig.key(Key::Char('o'));
    assert_eq!(rig.app.take_effects(), [Effect::PickFile]);
}

#[test]
fn fullscreen_dropped_files_and_the_surface() {
    if skip() {
        return;
    }
    let mut rig = Rig::new();
    rig.send(InputEvent::Drop { id: fixture("av1_opus.webm") });
    assert!(rig.app.model().has_media());
    assert_eq!(rig.app.model().title, "av1_opus.webm");
    rig.run(600);
    assert_eq!(rig.state(), MediaState::Playing);
    rig.key(Key::Char('f'));
    assert!(rig.host.surface.fullscreen);
    assert!(rig.app.model().fullscreen);
    rig.key(Key::Escape); // the UI turns Esc in fullscreen into leaving it
    assert!(!rig.host.surface.fullscreen);
    // A resize is picked up by the next tick.
    rig.host.surface.size = (640, 360, 1.0);
    rig.run(50);
    assert_eq!(rig.host.surface.rgba.len(), 640 * 360 * 4);
    assert_eq!(rig.app.ui().size(), (640, 360));
}

#[test]
fn a_broken_file_shows_an_error_in_the_apps_voice() {
    let dir = std::env::temp_dir().join(format!("rvp-app-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("not-a-video.mp4");
    std::fs::write(&path, vec![0u8; 4096]).unwrap();
    let mut rig = Rig::new();
    rig.app.open(&mut rig.host, FileSource::open(path.to_str().unwrap()).unwrap());
    rig.run(200);
    assert_eq!(rig.state(), MediaState::Failed);
    let err = rig.app.model().error.clone().unwrap();
    assert!(err.contains("guest list") || err.contains("damaged") || err.contains("ends too early"), "{err}");
    std::fs::remove_dir_all(dir).ok();
}
