//! M4: AV1 video through the whole session in virtual time: bit-exact frames, A/V sync over a minute,
//! a 200 ms decode stall, and seeking.
use rvp_host_headless::{PlayOptions, SessionState, play_file};
use std::path::{Path, PathBuf};
use std::process::Command;
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

const FRAME_US: i64 = 40_000; // 25 fps

fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[test]
fn presented_frames_are_the_frames_ffmpeg_decodes() {
    if skip() {
        return;
    }
    let path = fixture("av1_opus.webm");
    let report = play_file(&path, &PlayOptions::default()).unwrap();
    assert_eq!(report.state, SessionState::Ended, "{:?}", report.error);
    let raw = Command::new("ffmpeg")
        .args(["-v", "error", "-i", &path, "-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .output()
        .unwrap()
        .stdout;
    let fsz = 320 * 240 * 3 / 2;
    let want: Vec<u64> = raw.chunks_exact(fsz).map(fnv).collect();
    let got: Vec<u64> = report.video_frames.iter().map(|f| f.1).collect();
    assert_eq!(report.video_stats.dropped, 0, "nothing should be late at normal speed");
    assert_eq!(got, want, "every frame the sink got is bit-exact");
}

#[test]
fn a_minute_of_av1_stays_in_sync_with_audio() {
    if skip() {
        return;
    }
    let path = fixture("av1_opus_60s.webm");
    // An awkward tick interval so frames are never presented exactly on their due time.
    let opts = PlayOptions { tick_us: Some(13_000), ..Default::default() };
    let r = play_file(&path, &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let st = &r.video_stats;
    eprintln!("60 s soak at 13 ms ticks: {st:?}");
    assert_eq!(st.presented + st.dropped, 1500, "{st:?}: every frame is shown or counted as dropped");
    assert!(st.dropped <= 15, "{st:?}: at 13 ms ticks only a few frames may be skipped");
    // The headline number: video never strays more than one frame from the audio-locked clock.
    assert!(st.max_drift_us <= FRAME_US, "max |clock - pts| = {} us", st.max_drift_us);
    assert!(
        st.max_staleness_us <= FRAME_US + 13_000,
        "an old frame stayed up for {} us",
        st.max_staleness_us
    );
    assert!(r.video_trace.windows(2).all(|w| w[0].pts < w[1].pts), "frames shown in order");
    assert!(r.video_trace.iter().all(|e| (e.clock_us - e.pts).abs() <= FRAME_US));
    // Audio ran the whole minute without gaps.
    assert!((r.audio.len() as i64 / 2 - 2_880_000).abs() < 2_000, "{} audio frames", r.audio.len() / 2);
    let max_gap = r
        .audio_trace
        .windows(2)
        .map(|w| w[1].pts - (w[0].pts + w[0].frames as i64 * 1_000_000 / 48_000))
        .max()
        .unwrap();
    assert!(max_gap <= 20_000, "audio gap {max_gap} us");
    assert!((r.virtual_us - 60_020_000).abs() < 300_000, "virtual time {}", r.virtual_us);
}

#[test]
fn a_200_ms_decode_stall_drops_frames_and_recovers() {
    if skip() {
        return;
    }
    let path = fixture("av1_opus_60s.webm");
    let baseline = play_file(&path, &PlayOptions::default()).unwrap();
    let opts = PlayOptions { video_stall: Some((500, 200_000)), ..Default::default() };
    let r = play_file(&path, &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    let st = &r.video_stats;
    eprintln!("200 ms decode stall: {st:?}");
    // 200 ms is 5 frames: those are dropped, nothing else is.
    assert!(st.dropped >= 4 && st.dropped <= 7, "{st:?}");
    assert_eq!(st.dropped - baseline.video_stats.dropped, st.dropped);
    assert!(st.max_drift_us <= FRAME_US, "drift after the stall: {} us", st.max_drift_us);
    // The picture froze for about the stall, no longer.
    assert!(
        st.max_staleness_us >= 150_000 && st.max_staleness_us <= 300_000,
        "staleness {}",
        st.max_staleness_us
    );
    // Recovery: from half a second after the stall frames advance one at a time again, to the end.
    let stall_at = 500 * FRAME_US;
    let after: Vec<_> = r.video_trace.iter().filter(|e| e.pts > stall_at + 600_000).collect();
    assert!(after.len() > 900, "{} frames after recovery", after.len());
    assert!(after.windows(2).all(|w| w[1].pts - w[0].pts == FRAME_US), "frames step by 40 ms after recovery");
    assert!(r.video_trace.last().unwrap().pts >= 59_900_000, "played to the end");
    // The stall did not touch audio.
    assert_eq!(r.audio.len(), baseline.audio.len());
    let max_gap = r
        .audio_trace
        .windows(2)
        .map(|w| w[1].pts - (w[0].pts + w[0].frames as i64 * 1_000_000 / 48_000))
        .max()
        .unwrap();
    assert!(max_gap <= 20_000, "audio gap {max_gap} us");
}

#[test]
fn seeking_shows_the_frame_at_the_target_and_stays_in_sync() {
    if skip() {
        return;
    }
    let path = fixture("av1_opus.webm");
    let to = 4_000_000;
    let opts = PlayOptions { seeks: vec![(2_000_000, to)], ..Default::default() };
    let r = play_file(&path, &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    // Frames before the seek are < 2.7 s; the first one after it is the frame at (or just before) the target.
    let k = r.video_trace.iter().position(|e| e.pts >= to - FRAME_US).expect("frames after the seek");
    assert!(r.video_trace[..k].iter().all(|e| e.pts < 3_000_000));
    let first = r.video_trace[k];
    assert!(first.pts <= to && to - first.pts < FRAME_US, "first frame after the seek: {} us", first.pts);
    assert!(r.video_trace[k..].iter().all(|e| (e.clock_us - e.pts).abs() <= FRAME_US));
    assert!(r.video_trace.last().unwrap().pts >= 5_900_000);
}

#[test]
fn speed_changes_how_fast_video_and_audio_play() {
    if skip() {
        return;
    }
    let path = fixture("av1_opus.webm"); // 6 s, 25 fps
    for (rate, secs) in [(2.0, 3.0), (0.5, 12.0), (4.0, 1.5)] {
        let r = play_file(&path, &PlayOptions { rate: Some(rate), ..Default::default() }).unwrap();
        assert_eq!(r.state, SessionState::Ended, "{rate}x: {:?}", r.error);
        let virt = r.virtual_us as f64 / 1e6;
        assert!((virt - secs).abs() < 0.6, "{rate}x took {virt} s, wanted about {secs} s");
        // Every picture is still shown in order, within a frame of the clock.
        assert!(r.video_trace.windows(2).all(|w| w[0].pts < w[1].pts));
        assert_eq!(r.video_stats.presented + r.video_stats.dropped, 150, "{rate}x: {:?}", r.video_stats);
        if rate <= 2.0 {
            assert_eq!(r.video_stats.dropped, 0, "{rate}x: {:?}", r.video_stats);
        }
        // The audio is the same six seconds, resampled to take 1/rate as long (varispeed).
        let audio_s = r.audio.len() as f64 / 2.0 / 48_000.0;
        assert!((audio_s - 6.0 / rate).abs() < 0.3, "{rate}x: {audio_s} s of audio");
    }
}
