//! M3: audio playback through the headless host against ffmpeg's decode of the same files.
//!
//! Needs ffmpeg/ffprobe (fixtures are generated on first use); `RVP_SKIP_FIXTURES=1` skips.
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

/// ffmpeg's decode of the first audio stream: interleaved stereo f32 at 48 kHz.
fn reference(path: &str) -> Vec<f32> {
    let out = Command::new("ffmpeg")
        .args([
            "-v", "error", "-i", path, "-map", "0:a:0", "-vn", "-f", "f32le", "-ac", "2", "-ar", "48000", "-",
        ])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg failed");
    out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn rms_dbfs(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    let e: f64 = (0..n).map(|i| ((a[i] - b[i]) as f64).powi(2)).sum::<f64>() / n as f64;
    10.0 * (e + 1e-24).log10()
}

/// (file, codec, frame length in samples: the allowed length difference, bit exact?)
const CASES: &[(&str, &str, usize, bool)] = &[
    ("h264_aac.mp4", "aac", 1024, false),
    ("av1_opus.webm", "opus", 960, false),
    ("vp9_vorbis.webm", "vorbis", 2048, false),
    ("h264_flac.mkv", "flac", 0, true),
    ("mp3.mkv", "mp3", 1152, false),
];

#[test]
fn every_audio_codec_matches_ffmpeg() {
    if skip() {
        return;
    }
    for &(file, codec, frame, exact) in CASES {
        let path = fixture(file);
        let report = play_file(&path, &PlayOptions::default()).unwrap();
        assert_eq!(report.state, SessionState::Ended, "{file}: {:?}", report.error);
        let want = reference(&path);
        let (mine, theirs) = (report.audio.len() / 2, want.len() / 2);
        assert!(mine.abs_diff(theirs) <= frame, "{file} ({codec}): {mine} frames vs ffmpeg {theirs}");
        let err = rms_dbfs(&report.audio, &want);
        if exact {
            let n = report.audio.len().min(want.len());
            assert_eq!(report.audio[..n], want[..n], "{file} ({codec}) must be bit-exact");
            assert_eq!(mine, theirs, "{file}: length");
        }
        assert!(err < -60.0, "{file} ({codec}): RMS error {err:.1} dBFS");
        // Playback runs in virtual time at 1x: it must take about as long as the media.
        let media = mine as f64 / 48_000.0;
        let took = report.virtual_us as f64 / 1e6;
        assert!((took - media).abs() < 0.25, "{file}: {took:.3}s of virtual time for {media:.3}s of audio");
    }
}

#[test]
fn the_rvp_headless_binary_writes_a_matching_wav() {
    if skip() {
        return;
    }
    let path = fixture("h264_flac.mkv");
    let wav = std::env::temp_dir().join(format!("rvp-m3-{}.wav", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_rvp-headless"))
        .args(["play", &path, "--audio-wav"])
        .arg(&wav)
        .output()
        .expect("run rvp-headless");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("state=Ended"));
    let bytes = std::fs::read(&wav).unwrap();
    std::fs::remove_file(&wav).ok();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 3, "IEEE float WAV");
    let mine: Vec<f32> =
        bytes[44..].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    assert_eq!(mine, reference(&path), "FLAC through the CLI is bit-exact");
}

#[test]
fn seeking_resumes_at_the_target_without_gaps() {
    if skip() {
        return;
    }
    let (at, to) = (2_000_000, 4_000_000);
    for &(file, codec, _, _) in CASES {
        let path = fixture(file);
        let opts = PlayOptions { seeks: vec![(at, to)], ..Default::default() };
        let report = play_file(&path, &opts).unwrap();
        assert_eq!(report.state, SessionState::Ended, "{file}: {:?}", report.error);
        let t = &report.audio_trace;
        // Chunks written before the seek stay below ~3.7 s (position 2 s plus up to 1.7 s of buffering).
        let k = t.iter().position(|e| e.pts >= to - 100_000).expect("audio after the seek");
        let first = t[k];
        // "Within one packet of the target": the player discards up to the target, so the gap is tiny.
        assert!(
            (first.pts - to).abs() <= 24_000,
            "{file} ({codec}): audio resumed at {} us for a target of {to} us",
            first.pts
        );
        let gaps = |seg: &[rvp_host_headless::TraceEntryRef]| {
            seg.windows(2)
                .map(|w| w[1].pts - (w[0].pts + w[0].frames as i64 * 1_000_000 / 48_000))
                .max()
                .unwrap_or(0)
        };
        assert!(gaps(&t[..k]) <= 20_000, "{file}: gap before the seek");
        assert!(gaps(&t[k..]) <= 20_000, "{file}: gap after the seek: {} us", gaps(&t[k..]));
        // Total audio actually delivered: ~2 s before the seek (+ buffered) and 2 s after it.
        let after: i64 = t[k..].iter().map(|e| e.frames as i64).sum();
        assert!(
            (after - 96_000).abs() < 4_000,
            "{file}: {after} frames after the seek (expected about 96000)"
        );
    }
}
