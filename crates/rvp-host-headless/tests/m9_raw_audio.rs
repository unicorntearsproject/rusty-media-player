//! M9: raw audio files (MP3, FLAC, Ogg, WAV, ADTS AAC) play through the whole pipeline (demuxer, decoder, gapless trimming,
//! session) and sound like ffmpeg's decode of the same file: same length to the sample or a few, and a small error.
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
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
        let st = Command::new("bash")
            .arg(script)
            .arg(dir())
            .env("RVP_FIXTURE_SET", "audio")
            .status()
            .expect("run gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    dir().join("audio").join(name).to_string_lossy().into_owned()
}

/// ffmpeg's decode as interleaved stereo `f32` at 48 kHz (the rate the player outputs at).
fn reference(name: &str) -> Vec<f32> {
    // A mono file is copied to both channels, at full level (ffmpeg's own upmix lowers it by 3 dB).
    let upmix = if name.contains("mono") { "pan=stereo|c0=c0|c1=c0" } else { "anull" };
    let out = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            &fixture(name),
            "-vn",
            "-af",
            upmix,
            "-f",
            "f32le",
            "-ac",
            "2",
            "-ar",
            "48000",
            "-",
        ])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success());
    out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn rms_db(err: impl Iterator<Item = f32>) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0usize);
    for e in err {
        sum += (e as f64) * (e as f64);
        n += 1;
    }
    10.0 * ((sum / n.max(1) as f64).max(1e-20)).log10()
}

#[test]
fn every_raw_format_plays_like_ffmpeg_decodes_it() {
    if skip() {
        return;
    }
    // (file, frames of slack in the length, allowed error in dBFS; 0 for the length only: the player resamples that one)
    let cases = [
        ("cbr.mp3", 1_200, -100.0),
        ("vbr_v23.mp3", 1_200, -100.0),
        ("mono_v1.mp3", 1_200, 0.0),
        ("plain.mp3", 1_200, -100.0),
        ("tone.flac", 0, -90.0),
        ("tone24_mono.flac", 0, -90.0),
        ("tone.ogg", 600, -100.0),
        ("tone.opus", 600, -70.0),
        ("tone_flac.oga", 0, -90.0),
        ("tone16.wav", 0, -90.0),
        ("tone24.wav", 0, -90.0),
        ("tonef32.wav", 0, -90.0),
        ("tone8_mono.wav", 0, -90.0),
        ("tone.aac", 2_100, -100.0),
    ];
    for (name, slack, max_err) in cases {
        let r = play_file(&fixture(name), &PlayOptions::default()).unwrap();
        assert_eq!(r.state, SessionState::Ended, "{name}: {:?}", r.error);
        let want = reference(name);
        let (got_frames, want_frames) = (r.audio.len() / 2, want.len() / 2);
        eprintln!("{name}: {got_frames} frames (ffmpeg {want_frames})");
        assert!(
            got_frames.abs_diff(want_frames) <= slack,
            "{name}: {got_frames} frames vs ffmpeg's {want_frames}"
        );
        if max_err == 0.0 {
            continue;
        }
        // The waveform: compare at the best alignment within the slack (decoders differ by their delay handling).
        let n = got_frames.min(want_frames) * 2;
        let mut best = f64::MAX;
        for shift in 0..=(slack as i64) {
            for sign in [1i64, -1] {
                let s = shift * sign * 2;
                let (a, b) = if s >= 0 {
                    (&r.audio[s as usize..], &want[..])
                } else {
                    (&r.audio[..], &want[(-s) as usize..])
                };
                let m = a.len().min(b.len()).min(n);
                if m < n / 2 {
                    continue;
                }
                best = best.min(rms_db(a[..m].iter().zip(&b[..m]).map(|(x, y)| x - y)));
                if shift == 0 {
                    break;
                }
            }
            // A coarse search is enough: stop at the first good alignment.
            if best < max_err {
                break;
            }
        }
        eprintln!("{name}: error {best:.1} dBFS");
        assert!(best < max_err, "{name}: error {best:.1} dBFS against ffmpeg");
    }
}
