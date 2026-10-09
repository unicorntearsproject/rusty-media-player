//! The audio output failing and coming back, through the whole application on the headless host with a fake output (virtual time, no
//! devices): busy and gone are told apart and named, the position stays where it was, the output is retried quietly with a growing pause,
//! playback resumes by itself if it was playing (and stays paused if it was not), and a note says so.
use rvp_app::App;
use rvp_host::{AudioIssue, HostClock, retry_delay_us};
use rvp_host_headless::{DefaultCodecs, FileSource, UiHost};
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
        if !dir.join(".done").exists() {
            let st = Command::new("bash")
                .arg(root.join("tools/gen-fixtures.sh"))
                .arg(&dir)
                .status()
                .expect("gen-fixtures");
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
    toasts: Vec<String>,
    /// Every time a note appeared (also the same words again after they had gone).
    shown: Vec<String>,
    visible: bool,
}

impl Rig {
    fn new() -> Self {
        let mut r = Rig {
            host: UiHost::new(),
            app: App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true }),
            toasts: Vec::new(),
            shown: Vec::new(),
            visible: false,
        };
        r.app.open(&mut r.host, FileSource::open(&fixture("h264_aac.mp4")).unwrap());
        r
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.app.tick(&mut self.host);
            match self.app.ui().toast_text() {
                Some(t) => {
                    if self.toasts.last().map(String::as_str) != Some(t) {
                        self.toasts.push(t.to_string());
                    }
                    if !self.visible {
                        self.shown.push(t.to_string());
                    }
                    self.visible = true;
                }
                None => self.visible = false,
            }
            clock.advance(16_000);
        }
    }

    fn now(&self) -> i64 {
        self.host.virtual_clock().now_us()
    }

    fn pos(&self) -> i64 {
        self.app.model().position_us
    }
}

fn fail(r: &mut Rig, issue: AudioIssue, after_us: i64) {
    let at = r.now() + after_us;
    r.host.audio.inject_failure(issue, Some(at));
}

#[test]
fn a_busy_output_is_named_waited_for_and_resumed_with_the_position_kept() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.run(1_500);
    assert_eq!(r.app.model().state, MediaState::Playing);
    let before = r.pos();
    assert!(before > 400_000, "{before}");
    fail(
        &mut r,
        AudioIssue::from_error("DDJ-REV1", "start node error -16: Device or resource busy", None),
        4_000_000,
    );
    r.run(2_500);
    // Named, with the way out; the position stood still (give or take what the player had already counted).
    assert!(
        r.toasts.iter().any(|t| t == "Another app is using DDJ-REV1 exclusively. Close it or choose another output. Rusty Wave will resume when it's free."),
        "{:?}",
        r.toasts
    );
    let frozen = r.pos();
    assert!(frozen - before < 1_200_000, "the position ran on: {before} -> {frozen}");
    r.run(1_000);
    assert!((r.pos() - frozen).abs() < 60_000, "still waiting: {frozen} -> {}", r.pos());
    // The output comes back by itself and the picture and the sound go on from where they were.
    r.run(4_000);
    assert!(r.host.audio.issue_is_none(), "recovered");
    assert!(r.toasts.iter().any(|t| t == "Audio back on DDJ-REV1"), "{:?}", r.toasts);
    assert_eq!(r.app.model().state, MediaState::Playing);
    assert!(r.pos() > frozen + 1_000_000, "playing again: {frozen} -> {}", r.pos());
    // Retried quietly: never faster than once a second, easing out, never a spin.
    let t = &r.host.audio.retries;
    assert!(!t.is_empty() && t.len() <= 6, "{t:?}");
    for w in t.windows(2) {
        assert!(w[1] - w[0] >= retry_delay_us(0) - 20_000, "{t:?}");
    }
}

#[test]
fn an_unplugged_output_is_told_apart_and_a_pause_during_the_outage_stays_a_pause() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.run(1_200);
    fail(&mut r, AudioIssue::from_error("USB Speakers", "DeviceNotAvailable", None), 3_000_000);
    r.run(500);
    assert!(
        r.toasts.iter().any(|t| t == "USB Speakers was unplugged or the sound server restarted. Rusty Wave will resume when it's back."),
        "{:?}",
        r.toasts
    );
    // The listener pauses while it is out: when the output returns, it stays paused.
    let now = r.now();
    r.app.apply(&mut r.host, Action::PlayPause, now);
    r.run(5_000);
    assert!(r.toasts.iter().any(|t| t == "Audio back on USB Speakers"), "{:?}", r.toasts);
    assert_eq!(r.app.model().state, MediaState::Paused);
    let p = r.pos();
    r.run(1_000);
    assert_eq!(r.pos(), p, "paused stays paused");
}

#[test]
fn anything_else_keeps_a_clear_message_with_the_reason() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.run(800);
    fail(&mut r, AudioIssue::from_error("Speakers", "unsupported sample format F64", None), 2_000_000);
    r.run(400);
    let t = r.toasts.last().cloned().unwrap();
    assert!(
        t.contains("Speakers") && t.contains("unsupported sample format F64") && t.contains("keep trying"),
        "{t}"
    );
}

#[test]
fn the_message_is_put_up_again_while_the_output_stays_away() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.run(800);
    r.host.audio.inject_failure(AudioIssue::from_error("DDJ-REV1", "busy", None), None);
    r.run(14_000);
    let n = r.shown.iter().filter(|t| t.starts_with("Another app is using")).count();
    assert!(n >= 2, "the note was shown {n} time(s) in 14 s");
}

#[test]
fn pressing_play_asks_for_the_output_again_at_once() {
    if skip() {
        return;
    }
    use rvp_host::AudioSink;
    let mut r = Rig::new();
    r.run(500);
    let now = r.now();
    r.host.audio.inject_failure(AudioIssue::from_error("X", "busy", None), Some(now));
    r.host.audio.maintain();
    assert!(r.host.audio.retries.is_empty(), "the first try waits a second");
    r.host.audio.set_paused(false);
    r.host.audio.maintain();
    assert_eq!(r.host.audio.retries.len(), 1, "play retries at once");
    let _ = HostClock::now_us(&*r.host.virtual_clock());
}
