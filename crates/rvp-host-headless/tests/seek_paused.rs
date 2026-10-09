//! A seek while paused shows the picture at the target promptly, whatever the key frame spacing: counted in ticks of the virtual clock and in
//! pictures presented, never in wall time. (The first version of this test failed: with a later key frame already queued, the player
//! jumped over the target, and no picture ever came.)
use rvp_core::VideoDecoder;
use rvp_host::HostClock;
use rvp_host_headless::{DefaultCodecs, FileSource, HeadlessHost, SessionState};
use rvp_player::Session;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
        || Command::new("ffmpeg").arg("-version").output().is_err()
}

fn make(name: &str, codec: &[&str], gop: u32) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(dir).unwrap();
    let out = dir.join(name);
    if !out.exists() {
        let st = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=30"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=30"])
            .args(codec)
            .args(["-g", &gop.to_string(), "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
            .arg(&out)
            .status()
            .expect("ffmpeg");
        assert!(st.success());
    }
    out
}

/// Seek a paused session to `target` and return (ticks until the first new picture, ticks until the picture at the target, that picture's pts).
fn paused_seek(path: &Path, target: i64, cost_us: i64) -> (usize, usize, i64) {
    let mut host = HeadlessHost::new();
    let clock = host.virtual_clock();
    let codecs =
        DefaultCodecs { clock: Some(clock.clone()), video_cost_us: cost_us, ..DefaultCodecs::default() };
    let mut s = Session::new(FileSource::open(&path.to_string_lossy()).unwrap(), Rc::new(codecs));
    s.enable_video_trace();
    s.play();
    let mut ticks = 0;
    while !(s.state() == SessionState::Playing && s.position_us(clock.now_us()) > 600_000) {
        s.tick(&mut host);
        clock.advance(10_000);
        ticks += 1;
        assert!(ticks < 3000, "the file never started");
    }
    s.pause(clock.now_us());
    for _ in 0..30 {
        s.tick(&mut host);
        clock.advance(10_000);
    }
    let before = s.video_stats().presented;
    let n_trace = s.video_trace().len();
    s.seek(target);
    let mut first = None;
    for t in 1..=1500 {
        s.tick(&mut host);
        clock.advance(10_000);
        if s.video_stats().presented > before {
            first.get_or_insert(t);
            assert!(s.video_trace().len() > n_trace);
            let pts = s.video_trace().last().unwrap().pts;
            if pts <= target && target - pts < 45_000 {
                return (first.unwrap(), t, pts);
            }
        }
    }
    panic!(
        "no picture at the target within 15 s of virtual time after a paused seek to {target} (first picture: {first:?})"
    );
}

#[test]
fn a_paused_seek_shows_the_target_picture_with_a_later_key_frame_queued_and_without() {
    if skip() {
        return;
    }
    let av1 =
        make("seek_paused_av1.mkv", &["-c:v", "libsvtav1", "-preset", "12", "-svtav1-params", "log=0"], 50);
    let h264 = make("seek_paused_h264.mp4", &["-c:v", "libx264", "-preset", "veryfast", "-bf", "2"], 50);
    for (name, file) in [("av1", av1), ("h264", h264)] {
        // Key frames every 2 s. 1.8 s is 1.8 s past the key frame at 0 and the next one (2.0 s) is already queued; 7.9 s is just before one;
        // 12.0 s is on one; 21.3 s is in the middle of a group.
        // A slow machine: every packet takes 30 ms of the (virtual) clock to decode, so the demuxed packets pile up behind the decoder.
        for cost in [0, 30_000] {
            for target in [1_800_000, 7_900_000, 12_000_000, 21_300_000] {
                let (first, ticks, pts) = paused_seek(&file, target, cost);
                assert!(
                    pts <= target && target - pts < 45_000,
                    "{name} {target} cost {cost}: the picture shown is at {pts}"
                );
                // The screen answers the seek at once with the key frame as a stand-in (a few ticks), however long the target takes.
                assert!(first <= 12, "{name} {target} cost {cost}: first picture after {first} ticks");
                // At most 50 pictures from the key frame to decode: they cost at most 1.5 s, plus a little scheduling; a minute of ticks would
                // be the bug.
                assert!(
                    ticks as i64 * 10_000 <= 1_500_000 + 50 * cost + 500_000,
                    "{name} {target} cost {cost}: {ticks} ticks"
                );
            }
        }
    }
    let _ = std::any::type_name::<Box<dyn VideoDecoder>>();
}
