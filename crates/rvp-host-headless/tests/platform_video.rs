//! The platform decoder handoff on the headless host: 10-bit H.264 moves to the platform; HEVC is decoded by ours first, with or without
//! a platform decoder behind it.
use rvp_app::App;
use rvp_core::PlatformVideo;
use rvp_host::{HostClock, ScriptedPlatform};
use rvp_host_headless::{DefaultCodecs, FileSource, UiHost};
use rvp_ui::{MediaState, UiConfig};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Once;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn hevc_dir() -> Option<PathBuf> {
    static ONCE: Once = Once::new();
    let dir =
        std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root().join("target/fixtures"));
    if std::env::var_os("RVP_SKIP_FIXTURES").is_some() {
        return None;
    }
    ONCE.call_once(|| {
        let st = Command::new("bash")
            .arg(root().join("tools/gen-fixtures.sh"))
            .arg(&dir)
            .env("RVP_FIXTURE_SET", "hevc")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (ffmpeg needed; RVP_SKIP_FIXTURES=1 skips)");
    });
    let d = dir.join("hevc");
    d.join("hevc_aac.mp4").exists().then_some(d)
}

struct Rig {
    host: UiHost,
    app: App,
    /// Every toast shown so far.
    seen: Vec<String>,
}

impl Rig {
    fn new(platform: Option<ScriptedPlatform>) -> Self {
        let platform: Option<Rc<dyn PlatformVideo>> = platform.map(|p| Rc::new(p) as Rc<dyn PlatformVideo>);
        let codecs = DefaultCodecs { platform, ..DefaultCodecs::default() };
        Rig {
            host: UiHost::new(),
            app: App::new(Rc::new(codecs), UiConfig { reduce_motion: true }),
            seen: Vec::new(),
        }
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.app.tick(&mut self.host);
            self.host.virtual_clock().advance(16_000);
            self.app.take_effects();
            if let Some(t) = self.app.ui().toast_text() {
                if self.seen.last().map(String::as_str) != Some(t) {
                    self.seen.push(t.to_string());
                }
            }
        }
    }

    fn open(&mut self, path: &Path) {
        self.app.open(&mut self.host, FileSource::open(&path.to_string_lossy()).unwrap());
    }

    fn toast(&self) -> String {
        self.seen.join(" | ")
    }
}

#[test]
fn hevc_plays_with_our_own_decoder_when_the_host_has_no_platform_decoder() {
    let Some(d) = hevc_dir() else { return };
    for file in ["hevc_aac.mp4", "hevc10_aac.mp4", "hevc_flac.mkv"] {
        let mut r = Rig::new(None);
        r.open(&d.join(file));
        r.run(2000);
        assert_eq!(r.app.model().state, MediaState::Playing, "{file}");
        assert!(r.host.video.count > 10, "{file}: {} frames", r.host.video.count);
        assert_eq!((r.host.video.width, r.host.video.height), (320, 240), "{file}");
        assert!(r.toast().is_empty(), "{file}: nothing to say: {}", r.toast());
    }
}

#[test]
fn hevc_with_a_platform_decoder_present_is_still_decoded_by_ours_and_the_clock_follows_the_audio() {
    let Some(d) = hevc_dir() else { return };
    for file in ["hevc_aac.mp4", "hevc10_aac.mp4", "hevc_flac.mkv"] {
        let mut r = Rig::new(Some(ScriptedPlatform::decoding(&["hevc"])));
        r.open(&d.join(file));
        r.run(2000);
        assert_eq!(r.app.model().state, MediaState::Playing, "{file}");
        assert!(
            r.host.video.count > 10,
            "{file}: {} frames at {} us",
            r.host.video.count,
            r.app.model().position_us
        );
        assert_eq!((r.host.video.width, r.host.video.height), (320, 240), "{file}");
        let pos = r.app.model().position_us;
        assert!((1_500_000..2_500_000).contains(&pos), "{file}: position {pos}");
        assert!(
            (r.host.video.pts - pos).abs() < 200_000,
            "{file}: picture {} vs clock {pos}",
            r.host.video.pts
        );
    }
}

#[test]
fn ten_bit_h264_moves_to_the_platform_on_its_first_packet_and_says_so_without_one() {
    let Some(d) = hevc_dir() else { return };
    let mut r = Rig::new(Some(ScriptedPlatform::decoding(&["h264"])));
    r.open(&d.join("h264_10bit.mp4"));
    r.run(1500);
    assert!(r.host.video.count > 10, "{} frames", r.host.video.count);
    let mut r = Rig::new(None);
    r.open(&d.join("h264_10bit.mp4"));
    r.run(1500);
    assert_eq!(r.host.video.width, 0);
    assert_eq!(r.app.model().state, MediaState::Playing);
    let t = r.toast();
    assert!(t.contains("10-bit") && t.contains("H.264"), "{t}");
}
