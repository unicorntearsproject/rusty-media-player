//! The loudness meter on decoded files against `ffmpeg -af ebur128` (its reading of every fixture was saved when the fixtures
//! were made: `tools/gen-fixtures.sh`, set `levels`): integrated loudness within 0.05 LU, true peak within 0.3 dB, through every
//! codec and at several sample rates, plus the calibration tone of EBU Tech 3341 test case 1.
use rvp_core::task::block_on;
use rvp_host_headless::{DefaultCodecs, FileSource};
use rvp_player::{measure_source, measure_source_with};
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
            .env("RVP_FIXTURE_SET", "levels")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    dir().join("levels").join(name).to_string_lossy().into_owned()
}

/// ffmpeg's `(integrated loudness in LUFS, true peak in dBFS)` for a fixture.
fn ffmpeg_reading(name: &str) -> (f64, f64) {
    let text = std::fs::read_to_string(format!("{}.ebur128", fixture(name))).unwrap();
    let get = |key: &str| {
        text.lines()
            .filter_map(|l| l.strip_prefix(key))
            .next_back()
            .unwrap_or_else(|| panic!("{name}: no {key} in the ffmpeg reading"))
            .parse::<f64>()
            .unwrap()
    };
    (get("lavfi.r128.I="), 20.0 * get("lavfi.r128.true_peak=").log10())
}

#[test]
fn integrated_loudness_agrees_with_ffmpeg_ebur128() {
    if skip() {
        return;
    }
    let codecs = DefaultCodecs::default();
    let names = [
        "tone_m23.wav",     // EBU Tech 3341 case 1: -23.0 LUFS
        "tone_m14.wav",     //
        "music.flac",       // programme-like, stereo, 48 kHz
        "dynamic_44k.flac", // loud and quiet halves at 44.1 kHz: the gates matter
        "mono_22k.flac",    // mono at 22.05 kHz: dual mono, resampled filters
        "music.mp3",        // the same programme through four lossy codecs
        "music.m4a",
        "music.opus",
        "music.ogg",
        "lib/quiet-loud/01.flac",
        "lib/quiet-loud/02.flac",
        "lib/quiet-loud/03.flac",
    ];
    for name in names {
        let (want, _) = ffmpeg_reading(name);
        let got = block_on(measure_source(FileSource::open(&fixture(name)).unwrap(), &codecs))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .unwrap_or_else(|| panic!("{name}: nothing audible"));
        let diff = got.integrated_lufs as f64 - want;
        eprintln!("{name}: {:.3} LUFS, ffmpeg {want:.3} ({diff:+.3})", got.integrated_lufs);
        assert!(diff.abs() <= 0.05, "{name}: {} LUFS against ffmpeg's {want}", got.integrated_lufs);
    }
    // The calibration tone itself.
    let m = block_on(measure_source(FileSource::open(&fixture("tone_m23.wav")).unwrap(), &codecs))
        .unwrap()
        .unwrap();
    assert!((m.integrated_lufs + 23.0).abs() <= 0.1, "{}", m.integrated_lufs);
    assert!((m.duration_us - 20_000_000).abs() < 1000);
}

#[test]
fn true_peak_agrees_with_ffmpeg() {
    if skip() {
        return;
    }
    let codecs = DefaultCodecs::default();
    for name in ["music.flac", "dynamic_44k.flac", "music.opus", "tone_m14.wav"] {
        let (_, want_db) = ffmpeg_reading(name);
        let m = block_on(measure_source_with(FileSource::open(&fixture(name)).unwrap(), &codecs, true))
            .unwrap()
            .unwrap();
        let got_db = 20.0 * (m.true_peak.unwrap() as f64).log10();
        eprintln!("{name}: true peak {got_db:.2} dBFS, ffmpeg {want_db:.2}");
        // ffmpeg prints three decimals of the linear value; both use a 4 times oversampling filter, of different design.
        assert!((got_db - want_db).abs() < 0.3, "{name}: {got_db} dBFS against ffmpeg's {want_db}");
        assert!(m.true_peak.unwrap() >= m.sample_peak);
    }
}

#[test]
fn silence_and_non_audio_have_no_reading() {
    if skip() {
        return;
    }
    let codecs = DefaultCodecs::default();
    // Not a media file at all.
    let r = block_on(measure_source(FileSource::open(&fixture("tone_m23.wav.ebur128")).unwrap(), &codecs));
    assert!(r.is_err());
}
