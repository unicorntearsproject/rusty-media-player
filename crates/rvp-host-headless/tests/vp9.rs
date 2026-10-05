//! M7: VP9 through the whole session in virtual time: bit-exact frames, A/V sync with Opus and Vorbis, seeking,
//! a resolution change at a key frame, and playback that survives a damaged stream.
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

/// A fixture path; `vp9/<name>` for the VP9 set, a bare name for the core set.
fn fixture(name: &str) -> String {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let d = dir();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
        for set in ["core", "vp9"] {
            if set == "core" && d.join(".done").exists() {
                continue;
            }
            let st = Command::new("bash")
                .arg(&script)
                .arg(&d)
                .env("RVP_FIXTURE_SET", set)
                .status()
                .expect("run tools/gen-fixtures.sh");
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
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn raw(path: &str) -> Vec<u8> {
    Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            path,
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-noautoscale",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-",
        ])
        .output()
        .unwrap()
        .stdout
}

/// VP9 + Opus (WebM, written by us) and VP9 + Vorbis: every presented frame is what ffmpeg decodes, in order, and
/// the audio runs to the end alongside.
#[test]
fn vp9_frames_are_bit_exact_and_play_in_sync_with_audio() {
    if skip() {
        return;
    }
    for name in ["vp9/av_opus.webm", "vp9_vorbis.webm"] {
        let path = fixture(name);
        let report = play_file(&path, &PlayOptions::default()).unwrap();
        assert_eq!(report.state, SessionState::Ended, "{name}: {:?}", report.error);
        assert!(report.warnings.is_empty(), "{name}: {:?}", report.warnings);
        let want: Vec<u64> = raw(&path).chunks_exact(320 * 240 * 3 / 2).map(fnv).collect();
        let got: Vec<u64> = report.video_frames.iter().map(|f| f.1).collect();
        assert_eq!(report.video_stats.dropped, 0, "{name}: nothing should be late at normal speed");
        assert_eq!(got, want, "{name}: every frame the sink got is bit-exact");
        assert!(report.video_trace.windows(2).all(|w| w[0].pts < w[1].pts), "{name}: frames shown in order");
        assert!(
            report.video_stats.max_drift_us <= FRAME_US,
            "{name}: drift {} us",
            report.video_stats.max_drift_us
        );
        assert!(
            (report.audio.len() as i64 / 2 - 288_000).abs() < 4_000,
            "{name}: {} audio frames",
            report.audio.len() / 2
        );
    }
}

#[test]
fn seeking_shows_the_frame_at_the_target_and_stays_in_sync() {
    if skip() {
        return;
    }
    let path = fixture("vp9/av_opus.webm"); // key frame every 25 frames
    let to = 4_000_000;
    let opts = PlayOptions { seeks: vec![(2_000_000, to)], ..Default::default() };
    let r = play_file(&path, &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    let k = r.video_trace.iter().position(|e| e.pts >= to - FRAME_US).expect("frames after the seek");
    assert!(r.video_trace[..k].iter().all(|e| e.pts < 3_000_000));
    let first = r.video_trace[k];
    assert!(first.pts <= to && to - first.pts < FRAME_US, "first frame after the seek: {} us", first.pts);
    assert!(r.video_trace[k..].iter().all(|e| (e.clock_us - e.pts).abs() <= FRAME_US));
    assert!(r.video_trace.last().unwrap().pts >= 5_900_000);
}

/// A stream whose picture size changes at key frames (320x240, 480x270, 200x120): every frame is bit-exact at its
/// own size.
#[test]
fn a_resolution_change_at_a_key_frame_plays_through() {
    if skip() {
        return;
    }
    let path = fixture("vp9/r_keyframe.webm");
    let report = play_file(&path, &PlayOptions::default()).unwrap();
    assert_eq!(report.state, SessionState::Ended, "{:?}", report.error);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let data = raw(&path);
    let mut want = Vec::new();
    let mut off = 0;
    for (w, h) in [(320, 240), (480, 270), (200, 120)] {
        let sz = w * h * 3 / 2;
        for _ in 0..15 {
            want.push(fnv(&data[off..off + sz]));
            off += sz;
        }
    }
    assert_eq!(off, data.len());
    let got: Vec<u64> = report.video_frames.iter().map(|f| f.1).collect();
    assert_eq!(report.video_stats.dropped, 0, "{:?}", report.video_stats);
    assert_eq!(got, want);
}

/// 720p with tile columns plays to the end (decode cost is measured by the codec benchmark, not in virtual time).
#[test]
fn tiled_720p_plays_to_the_end() {
    if skip() {
        return;
    }
    let path = fixture("vp9/s_720p_typ.webm");
    let report = play_file(&path, &PlayOptions::default()).unwrap();
    assert_eq!(report.state, SessionState::Ended, "{:?}", report.error);
    assert_eq!(report.video_frames.len(), 60);
}

/// A damaged video stream must not end playback: broken packets are dropped with a warning and decoding resumes at
/// the next key frame.
#[test]
fn a_damaged_stream_keeps_playing() {
    if skip() {
        return;
    }
    let src = std::fs::read(fixture("vp9/av_opus.webm")).unwrap();
    // Zero a stretch in the middle of the file (inside a few video packets).
    let mut data = src.clone();
    let mid = data.len() / 2;
    for b in &mut data[mid..mid + 3000] {
        *b = 0;
    }
    let tmp = std::env::temp_dir().join(format!("rvp-vp9-damaged-{}.webm", std::process::id()));
    std::fs::write(&tmp, &data).unwrap();
    let report = play_file(tmp.to_str().unwrap(), &PlayOptions::default());
    let _ = std::fs::remove_file(&tmp);
    let report = report.unwrap();
    assert_eq!(report.state, SessionState::Ended, "{:?}", report.error);
    assert!(report.video_frames.len() > 60, "{} frames", report.video_frames.len());
}
