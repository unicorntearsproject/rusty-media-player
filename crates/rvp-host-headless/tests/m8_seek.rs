//! M8: exact seek, frame stepping and the A-B loop, driven against a session in virtual time.
use rvp_host_headless::{FileSource, HeadlessHost, PlayOptions, SessionState, play_file};
use rvp_player::Session;
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

fn fixture(name: &str) -> String {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let d = dir();
        if !d.join(".done").exists() {
            let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
            let st = Command::new("bash").arg(script).arg(&d).status().expect("run tools/gen-fixtures.sh");
            assert!(
                st.success(),
                "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)"
            );
        }
    });
    dir().join(name).to_string_lossy().into_owned()
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// ffmpeg's frames of the first video stream, hashed one by one (320x240 4:2:0).
fn reference_hashes(path: &str) -> Vec<u64> {
    let raw = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            path,
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-",
        ])
        .output()
        .unwrap()
        .stdout;
    raw.chunks_exact(320 * 240 * 3 / 2).map(fnv).collect()
}

const FRAME_US: i64 = 40_000;

struct Rig {
    host: HeadlessHost,
    session: Session,
}

impl Rig {
    fn new(path: &str) -> Self {
        let host = HeadlessHost::new();
        let codecs = rvp_host_headless::DefaultCodecs::default();
        let mut session = Session::new(FileSource::open(path).unwrap(), Rc::new(codecs));
        session.enable_video_trace();
        let mut r = Rig { host, session };
        // Open and show the first frame, paused.
        r.run_until(|r| !r.host.video.frames.is_empty(), 3_000_000);
        r
    }

    fn clock(&self) -> Rc<rvp_host_headless::VirtualClock> {
        self.host.virtual_clock()
    }

    /// Tick in 5 ms steps of virtual time until `done` holds (or `limit_us` pass); panics on timeout.
    fn run_until(&mut self, mut done: impl FnMut(&Rig) -> bool, limit_us: i64) {
        let clock = self.clock();
        let start = rvp_host::HostClock::now_us(&*clock);
        loop {
            self.session.tick(&mut self.host);
            if done(self) {
                return;
            }
            assert!(
                rvp_host::HostClock::now_us(&*clock) - start < limit_us,
                "timed out (state {:?}, {} frames shown)",
                self.session.state(),
                self.host.video.frames.len()
            );
            clock.advance(5_000);
        }
    }

    /// Run a little longer so nothing pending is left unprocessed.
    fn settle(&mut self, us: i64) {
        let clock = self.clock();
        let end = rvp_host::HostClock::now_us(&*clock) + us;
        while rvp_host::HostClock::now_us(&*clock) < end {
            self.session.tick(&mut self.host);
            clock.advance(5_000);
        }
    }

    fn last(&self) -> (i64, u64) {
        *self.host.video.frames.last().unwrap()
    }
}

/// Seeking while paused shows exactly the frame at the target, bit-identical to a sequential decode, for many
/// random targets, forward and backward, on a stream with B-frames and a 12 frame GOP.
#[test]
fn exact_seek_shows_the_same_frame_as_sequential_decode() {
    if skip() {
        return;
    }
    let path = fixture("h264_aac.mp4");
    let want = reference_hashes(&path);
    assert_eq!(want.len(), 150);
    let mut rig = Rig::new(&path);
    let mut x = 0x2545_f491_4f6c_dd1du64;
    for i in 0..50 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let n = (x % 150) as usize;
        // Half the targets land exactly on a frame time, half a little after it (the frame shown is still n).
        let target = n as i64 * FRAME_US + if i % 2 == 0 { 0 } else { 17_000 };
        let before = rig.host.video.frames.len();
        rig.session.seek(target);
        rig.run_until(|r| r.host.video.frames.len() > before, 3_000_000);
        let (pts, hash) = rig.last();
        assert_eq!(pts, n as i64 * FRAME_US, "seek to {target} us shows the frame at {pts} us");
        assert_eq!(hash, want[n], "frame {n} (seek {i}) differs from the sequential decode");
    }
}

#[test]
fn frame_stepping_walks_frame_by_frame_in_both_directions() {
    if skip() {
        return;
    }
    let path = fixture("h264_aac.mp4");
    let want = reference_hashes(&path);
    let mut rig = Rig::new(&path);
    rig.session.seek(10 * FRAME_US);
    rig.run_until(|r| r.last().0 == 10 * FRAME_US, 3_000_000);
    for n in 11..=14 {
        let before = rig.host.video.frames.len();
        rig.session.step_frame(true);
        rig.run_until(|r| r.host.video.frames.len() > before, 3_000_000);
        assert_eq!(rig.last(), (n * FRAME_US, want[n as usize]), "step forward to {n}");
        assert_eq!(rig.session.position_us(0), n * FRAME_US);
    }
    // Step across a key frame boundary backwards: 14 -> 13 -> 12 -> 11 -> 10 -> 9 (9 is in the previous GOP... gop is 12).
    for n in (8..14).rev() {
        let before = rig.host.video.frames.len();
        rig.session.step_frame(false);
        rig.run_until(|r| r.host.video.frames.len() > before, 3_000_000);
        assert_eq!(rig.last(), (n * FRAME_US, want[n as usize]), "step back to {n}");
    }
    // Several quick presses are all honoured.
    let before = rig.host.video.frames.len();
    for _ in 0..3 {
        rig.session.step_frame(true);
    }
    rig.run_until(|r| r.host.video.frames.len() >= before + 3, 3_000_000);
    assert_eq!(rig.last().0, 11 * FRAME_US);
    // Resuming from a stepped position plays on from there, with audio.
    rig.session.play();
    rig.run_until(|r| r.session.state() == SessionState::Playing, 2_000_000);
    rig.settle(300_000);
    let pos = rig.session.position_us(rvp_host::HostClock::now_us(&*rig.clock()));
    assert!(pos > 11 * FRAME_US && pos < 11 * FRAME_US + 1_000_000, "resumed at {pos}");
    assert!(rig.last().0 >= 11 * FRAME_US);
}

#[test]
fn the_a_b_loop_keeps_playback_between_the_marks() {
    if skip() {
        return;
    }
    let path = fixture("h264_aac.mp4");
    let opts = PlayOptions {
        ab_loop: Some((1_000_000, 2_000_000)),
        max_virtual_us: Some(7_000_000),
        ..Default::default()
    };
    let r = play_file(&path, &opts).unwrap();
    // It never ends by itself, so the run is cut off at the limit while still playing.
    assert_ne!(r.state, SessionState::Ended);
    assert!(
        r.video_trace.iter().all(|e| e.pts <= 2_000_000 + FRAME_US),
        "frames past B: {:?}",
        r.video_trace.iter().map(|e| e.pts).max()
    );
    let jumps = r.video_trace.windows(2).filter(|w| w[1].pts < w[0].pts).count();
    assert!(jumps >= 3, "{jumps} loops in 7 s of a 1 s loop");
    // Audio keeps coming through the loops: roughly the whole run is filled.
    assert!(r.audio.len() / 2 > 48_000 * 5, "{} audio frames", r.audio.len() / 2);
}
