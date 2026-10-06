//! M8: the now-playing model: metadata and playback state go out to the host's sink when they change, and the
//! transport commands that come back drive the player.
use rvp_app::App;
use rvp_host::{HostClock, InputEvent, Key, Modifiers, PlayState, RecordingNowPlaying, TransportCommand};
use rvp_host_headless::{DefaultCodecs, UiHost};
use rvp_ui::UiConfig;
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
        let st = Command::new("bash")
            .arg(root.join("tools/gen-fixtures.sh"))
            .arg(&dir)
            .env("RVP_FIXTURE_SET", "m8")
            .status()
            .expect("run gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    dir.join("m8").join(name).to_string_lossy().into_owned()
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
        host.now_playing = Some(RecordingNowPlaying::default());
        let app = App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true });
        Self { host, app }
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

    fn np(&mut self) -> &mut RecordingNowPlaying {
        self.host.now_playing.as_mut().unwrap()
    }

    fn key(&mut self, key: Key) {
        self.host.input.0.push_back(InputEvent::KeyDown { key, mods: Modifiers::default(), repeat: false });
        self.run(100);
    }

    fn command(&mut self, c: TransportCommand) {
        self.np().commands.push_back(c);
        self.run(200);
    }
}

#[test]
fn tags_art_and_state_reach_the_sink() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.open(&["tagged.m4a"]);
    r.run(800);
    let np = r.np();
    let m = np.metadata.last().unwrap();
    assert_eq!(m.title, "Sine Song");
    assert_eq!(m.artist, "The Tones");
    assert_eq!(m.album, "Pure");
    assert_eq!(m.art.as_ref().unwrap().mime, "image/jpeg");
    assert!(!m.has_video);
    assert!((m.duration_us.unwrap() - 6_000_000).abs() < 100_000);
    let p = np.playback.last().unwrap();
    assert_eq!(p.state, PlayState::Playing);
    assert!(p.can_seek && !p.can_next && p.can_prev);
    // The metadata went out once, not on every tick.
    assert_eq!(np.metadata.len(), 1);
    // Steady playback does not flood the sink: the host extrapolates the position.
    let before = r.np().playback.len();
    r.run(3_000);
    assert!(r.np().playback.len() - before <= 1, "{} updates in 3 s", r.np().playback.len() - before);
    // A pause and a seek are reported.
    r.command(TransportCommand::Pause);
    assert_eq!(r.np().playback.last().unwrap().state, PlayState::Paused);
    r.command(TransportCommand::SeekTo(1_000_000));
    let p = *r.np().playback.last().unwrap();
    assert!((p.position_us - 1_000_000).abs() < 300_000, "{}", p.position_us);
}

#[test]
fn a_file_without_tags_is_named_by_its_file_name() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.open(&["gap_0.mkv"]);
    r.run(500);
    let m = r.np().metadata.last().unwrap().clone();
    assert_eq!(m.title, "gap_0");
    assert_eq!(m.artist, "");
    assert!(m.art.is_none());
}

#[test]
fn transport_commands_drive_the_player() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.open(&["gap_0.mkv", "gap_1.mkv", "gap_2.mkv"]);
    r.run(500);
    assert!(r.np().playback.last().unwrap().can_next);
    r.command(TransportCommand::Pause);
    assert_eq!(r.app.model().state, rvp_ui::MediaState::Paused);
    r.command(TransportCommand::Play);
    r.run(300);
    assert_eq!(r.app.model().state, rvp_ui::MediaState::Playing);
    r.command(TransportCommand::Toggle);
    assert_eq!(r.app.model().state, rvp_ui::MediaState::Paused);
    r.command(TransportCommand::Toggle);
    r.command(TransportCommand::Next);
    r.run(400);
    assert_eq!(r.np().metadata.last().unwrap().title, "gap_1");
    r.command(TransportCommand::Next);
    r.run(400);
    assert_eq!(r.np().metadata.last().unwrap().title, "gap_2");
    assert!(!r.np().playback.last().unwrap().can_next, "last item");
    r.command(TransportCommand::Prev);
    r.run(400);
    assert_eq!(r.np().metadata.last().unwrap().title, "gap_1");
    r.command(TransportCommand::SeekBy(500_000));
    r.run(300);
    assert!(r.app.model().position_us > 600_000, "{}", r.app.model().position_us);
    r.command(TransportCommand::SetRate(2.0));
    r.run(400);
    assert_eq!(r.np().playback.last().unwrap().rate, 2.0);
    r.command(TransportCommand::SetVolume(0.25));
    assert!((r.app.model().volume - 0.25).abs() < 0.01);
    r.command(TransportCommand::Stop);
    r.run(300);
    assert_eq!(r.app.model().state, rvp_ui::MediaState::Paused);
    assert!(r.app.model().position_us < 300_000, "{}", r.app.model().position_us);
    // The sink saw the item change as a metadata update each time.
    let titles: Vec<&str> = r.np().metadata.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(titles, ["gap_0", "gap_1", "gap_2", "gap_1"]);
    let _ = HostClock::now_us(&*r.host.virtual_clock());
}

#[test]
fn the_volume_reaches_the_sink_from_every_source() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.open(&["gap_0.mkv"]);
    r.run(300);
    // Told once at the start, not on every tick.
    assert_eq!(r.np().volumes, [1.0]);
    // From the media controls (`playerctl volume 0.3`).
    r.command(TransportCommand::SetVolume(0.3));
    assert_eq!(r.np().volumes.last(), Some(&0.3));
    assert_eq!(r.np().volumes.len(), 2);
    // From the keys: Down is five percent.
    r.key(Key::Down);
    assert!((r.np().volumes.last().unwrap() - 0.25).abs() < 1e-4, "{:?}", r.np().volumes);
    // Mute reads as 0, and sound on brings the level back.
    r.key(Key::Char('m'));
    assert_eq!(r.np().volumes.last(), Some(&0.0));
    r.key(Key::Char('m'));
    assert!((r.np().volumes.last().unwrap() - 0.25).abs() < 1e-4, "{:?}", r.np().volumes);
}
