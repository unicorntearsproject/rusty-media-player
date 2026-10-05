//! M8: gapless playback. Three 2 s pieces cut from one continuous 440 Hz sine are played as a chain; the output
//! must equal the uncut sine.
use rvp_host_headless::{PlayOptions, SessionEvent, SessionState, play_file};
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
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
        let st = Command::new("bash")
            .arg(script)
            .arg(&d)
            .env("RVP_FIXTURE_SET", "m8")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    dir().join("m8").join(name).to_string_lossy().into_owned()
}

/// The uncut sine as stereo `f32` frames (what the player outputs for a mono 16-bit source).
fn full_sine() -> Vec<f32> {
    let wav = std::fs::read(fixture("sine_full.wav")).unwrap();
    let data = &wav[wav.len() - 6 * 48_000 * 2..];
    data.chunks_exact(2)
        .flat_map(|b| {
            let v = i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0;
            [v, v]
        })
        .collect()
}

fn play_chain(first: &str, rest: &[&str]) -> rvp_host_headless::PlayReport {
    let opts = PlayOptions { chain: rest.iter().map(|p| fixture(p)).collect(), ..Default::default() };
    let r = play_file(&fixture(first), &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    r
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
fn lossless_pieces_join_bit_exactly() {
    if skip() {
        return;
    }
    let r = play_chain("gap_0.mkv", &["gap_1.mkv", "gap_2.mkv"]);
    let want = full_sine();
    assert_eq!(r.audio.len(), want.len(), "no frame added or lost across the joins");
    assert!(r.audio == want, "output differs from the uncut sine");
    let started: Vec<u32> = r
        .events
        .iter()
        .filter_map(|(_, e)| match e {
            SessionEvent::ItemStarted { tag } => Some(*tag),
            _ => None,
        })
        .collect();
    assert_eq!(started, [1, 2]);
    // The second piece is heard 2 s in (plus the device latency), the third 4 s in.
    let times: Vec<i64> =
        r.events.iter().filter(|(_, e)| matches!(e, SessionEvent::ItemStarted { .. })).map(|e| e.0).collect();
    assert!((times[0] - 2_000_000).abs() < 150_000, "{times:?}");
    assert!((times[1] - 4_000_000).abs() < 150_000, "{times:?}");
}

#[test]
fn lossy_pieces_join_without_a_gap() {
    if skip() {
        return;
    }
    for (name, first, rest) in [
        ("opus", "gap_opus_0.webm", ["gap_opus_1.webm", "gap_opus_2.webm"]),
        ("aac", "gap_aac_0.mp4", ["gap_aac_1.mp4", "gap_aac_2.mp4"]),
    ] {
        let r = play_chain(first, &rest);
        let want = full_sine();
        // Lengths agree to well under a millisecond: the encoder delay and padding were trimmed.
        assert!(
            r.audio.len().abs_diff(want.len()) <= 2 * 48,
            "{name}: {} vs {} frames",
            r.audio.len() / 2,
            want.len() / 2
        );
        // And the waveform is the sine, with no click at the joins.
        let n = r.audio.len().min(want.len());
        let err = rms_db(r.audio[..n].iter().zip(&want[..n]).map(|(a, b)| a - b));
        eprintln!("{name}: {} frames (want {}), error {err:.1} dBFS", r.audio.len() / 2, want.len() / 2);
        assert!(
            err < if name == "opus" { -60.0 } else { -45.0 },
            "{name}: error {err:.1} dBFS against the uncut sine"
        );
        for join in [96_000usize, 192_000] {
            let around = &r.audio[(join - 200) * 2..(join + 200) * 2];
            let step = around
                .chunks_exact(2)
                .map(|f| f[0])
                .collect::<Vec<_>>()
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max);
            assert!(step < 0.1, "{name}: a click of {step} at the join {join}");
        }
    }
}
