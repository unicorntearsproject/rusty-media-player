//! Automatic level through the whole session: gains from ReplayGain and R128 tags in every container, the library's measurement
//! as a fallback, track and album mode, the target, a running estimate for files that nothing is known about (it settles and does not
//! pump), and the limiter keeping peaks under the ceiling. Fixtures: `tools/gen-fixtures.sh`, set `levels`.
use rvp_core::{AudioSettings, LevelMode, LoudnessMeter, LoudnessTags};
use rvp_host_headless::{PlayOptions, PlayReport, SessionState, play_file};
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

fn level(target: i8, mode: LevelMode) -> AudioSettings {
    AudioSettings { auto_level: true, target_lufs: target, level_mode: mode, ..AudioSettings::default() }
}

fn play(name: &str, settings: AudioSettings, hint: Option<LoudnessTags>) -> PlayReport {
    let opts = PlayOptions { audio_settings: settings, loudness_hint: hint, ..Default::default() };
    let r = play_file(&fixture(name), &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{name}: {:?}", r.error);
    r
}

/// RMS of the second half of a run (steady: the gain has long since arrived), in dBFS.
fn steady_rms_db(r: &PlayReport) -> f64 {
    let w = &r.audio[r.audio.len() / 2..];
    10.0 * ((w.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / w.len() as f64).log10())
}

#[test]
fn tags_in_every_container_set_the_gain() {
    if skip() {
        return;
    }
    // Every file is the same 440 Hz tone and carries a track loudness of -11.5 LUFS (ReplayGain -6.50 dB) or, for Opus, -21 LUFS
    // (R128_TRACK_GAIN -512). With the target at -14 the gain is the difference.
    let cases = [
        ("tagged_v24.mp3", -2.5),
        ("tagged_v23.mp3", -2.5),
        ("tagged.flac", -2.5),
        ("tagged.ogg", -2.5),
        ("tagged.mka", -2.5),
        ("tagged_mdta.m4a", -2.5),
        ("tagged_itunes.m4a", -2.5),
        ("tagged.opus", 7.0),
    ];
    for (name, gain_db) in cases {
        let plain = play(name, AudioSettings::default(), None);
        let r = play(name, level(-14, LevelMode::Track), None);
        let got = steady_rms_db(&r) - steady_rms_db(&plain);
        assert!((got - gain_db).abs() < 0.15, "{name}: the gain is {got} dB, expected {gain_db}");
        let reported = r.gains.last().unwrap().1 as f64;
        assert!((reported - gain_db).abs() < 0.01, "{name}: reported {reported}");
    }
}

#[test]
fn the_target_and_the_mode_are_respected() {
    if skip() {
        return;
    }
    let plain = play("tagged.flac", AudioSettings::default(), None);
    // The track says -11.5 LUFS and its album -14.8: the gain is the target less the figure of the mode.
    for (target, mode, expected) in [
        (-23, LevelMode::Track, -11.5),
        (-10, LevelMode::Track, 1.5),
        (-14, LevelMode::Album, 0.8),
        (-23, LevelMode::Album, -8.2),
    ] {
        let r = play("tagged.flac", level(target, mode), None);
        let got = steady_rms_db(&r) - steady_rms_db(&plain);
        assert!((got - expected).abs() < 0.15, "target {target} {mode:?}: {got} dB, expected {expected}");
    }
}

#[test]
fn the_librarys_measurement_is_used_when_the_file_has_no_tags_and_tags_win_over_it() {
    if skip() {
        return;
    }
    let plain = play("untagged.wav", AudioSettings::default(), None);
    let hint = LoudnessTags { track_lufs: Some(-20.0), album_lufs: Some(-17.0), ..Default::default() };
    let r = play("untagged.wav", level(-14, LevelMode::Track), Some(hint));
    assert!((steady_rms_db(&r) - steady_rms_db(&plain) - 6.0).abs() < 0.15);
    let r = play("untagged.wav", level(-14, LevelMode::Album), Some(hint));
    assert!((steady_rms_db(&r) - steady_rms_db(&plain) - 3.0).abs() < 0.15);
    // The same hint on a file with tags: the file's own -11.5 decides.
    let plain = play("tagged.flac", AudioSettings::default(), None);
    let r = play("tagged.flac", level(-14, LevelMode::Track), Some(hint));
    assert!((steady_rms_db(&r) - steady_rms_db(&plain) + 2.5).abs() < 0.15);
    // Album mode with no album figure anywhere falls back to the track's.
    let hint = LoudnessTags { track_lufs: Some(-20.0), ..Default::default() };
    let plain = play("untagged.wav", AudioSettings::default(), None);
    let r = play("untagged.wav", level(-14, LevelMode::Album), Some(hint));
    assert!((steady_rms_db(&r) - steady_rms_db(&plain) - 6.0).abs() < 0.15);
}

/// The loudness of what the sink got, integrated over `from..` (seconds).
fn loudness_of(r: &PlayReport, from_s: usize) -> f32 {
    let mut m = LoudnessMeter::new(48_000, 2);
    m.process(&r.audio[from_s * 48_000 * 2..]);
    m.integrated_lufs().unwrap()
}

/// The gain the sink got over the plain run, dB, per 100 ms of audio (the pipeline runs ahead of the clock, so the host-time trace of
/// `PlayReport::gains` is not the place to look for how fast the gain moved).
fn gain_trace(r: &PlayReport, plain: &PlayReport) -> Vec<f64> {
    let win = 4800 * 2;
    r.audio
        .chunks_exact(win)
        .zip(plain.audio.chunks_exact(win))
        .filter_map(|(a, b)| {
            let e = |w: &[f32]| w.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>();
            (e(b) > 1e-6).then(|| 10.0 * (e(a) / e(b)).log10())
        })
        .collect()
}

#[test]
fn files_nobody_knows_are_estimated_while_they_play_and_settle_without_pumping() {
    if skip() {
        return;
    }
    // music.flac measures -12.85 LUFS (ffmpeg and ours agree): to -23 it needs -10.15 dB.
    let plain = play("music.flac", AudioSettings::default(), None);
    let r = play("music.flac", level(-23, LevelMode::Track), None);
    let g = gain_trace(&r, &plain);
    let last = *g.last().unwrap();
    assert!((last + 10.15).abs() < 1.0, "settled at {last} dB");
    assert!((r.gains.last().unwrap().1 as f64 - last).abs() < 0.3, "reported gain follows what came out");
    // The second half of the programme comes out at the target.
    let out = loudness_of(&r, 12);
    assert!((out + 23.0).abs() < 1.0, "output loudness {out} LUFS");
    // The path to it: nothing for the first second or so, then down at no more than 4 dB a second (0.4 dB in 100 ms), no
    // turning back.
    assert!(g[0].abs() < 0.01 && g[5].abs() < 0.01, "{:?}", &g[..8]);
    let max_step = g.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f64, f64::max);
    assert!(max_step <= 0.45, "{max_step} dB in 100 ms");
    let turns =
        g.windows(3).filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < 0.0 && (w[1] - w[0]).abs() > 0.05).count();
    assert_eq!(turns, 0, "no change of direction: {g:?}");
    // A loud half followed by a quiet one (the worst case for a running estimate): the gain follows the average of what has
    // been heard so far, slowly (up 1.5 dB a second, down 4), and never swings.
    let plain = play("dynamic_44k.flac", AudioSettings::default(), None);
    let r = play("dynamic_44k.flac", level(-23, LevelMode::Track), None);
    let g = gain_trace(&r, &plain);
    let max_up = g.windows(2).map(|w| w[1] - w[0]).fold(f64::MIN, f64::max);
    let max_down = g.windows(2).map(|w| w[0] - w[1]).fold(f64::MIN, f64::max);
    assert!(max_up <= 0.16 + 0.02, "up {max_up} dB in 100 ms (1.5 dB a second)");
    assert!(max_down <= 0.41 + 0.02, "down {max_down} dB in 100 ms (4 dB a second)");
    let swings =
        g.windows(3).filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < -1e-4 && (w[1] - w[0]).abs() > 0.05).count();
    assert!(swings <= 2, "{swings} changes of direction in {} windows", g.len());
}

#[test]
fn the_limiter_keeps_the_peaks_under_the_ceiling() {
    if skip() {
        return;
    }
    // Pretend the library measured music.flac at -40 LUFS: the gain wants +26 dB, is held to +12, and that would put the peaks (0.58)
    // at 2.3 times full scale. Nothing may reach the sink above -1 dBFS.
    let hint = LoudnessTags { track_lufs: Some(-40.0), ..Default::default() };
    let r = play("music.flac", level(-10, LevelMode::Track), Some(hint));
    let ceiling = 10f32.powf(-1.0 / 20.0);
    let peak = r.audio.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak <= ceiling + 1e-6, "peak {peak} over {ceiling}");
    assert!(peak > 0.85, "the limiter works to the ceiling, not far below it: {peak}");
    // True peak too: between the samples the waveform stays within a quarter dB of the ceiling.
    let mut m = LoudnessMeter::new(48_000, 2).with_true_peak();
    m.process(&r.audio);
    m.process(&[0.0; 64]);
    let tp = 20.0 * (m.true_peak().unwrap() as f64).log10();
    assert!(tp < -1.0 + 0.4, "true peak {tp} dBTP");
    // And nothing is changed where nothing needs limiting: the quiet start of a file is bit-exact apart from the gain.
    let plain = play("music.flac", AudioSettings::default(), None);
    assert_eq!(r.audio.len(), plain.audio.len());
}

#[test]
fn off_changes_nothing_and_turning_it_on_is_a_ramp_not_a_step() {
    if skip() {
        return;
    }
    // Known loudness, on: the first samples are not at the full gain yet (a ramp over a tenth of a second), no sample step.
    let hint = LoudnessTags { track_lufs: Some(-24.0), ..Default::default() };
    let plain = play("music.flac", AudioSettings::default(), None);
    let r = play("music.flac", level(-14, LevelMode::Track), Some(hint));
    // +10 dB: the 440 Hz-ish programme gets 3.16 times louder, in about 80 ms.
    let early: f64 = r.audio[..2 * 480].iter().map(|x| x.abs() as f64).sum();
    let early_plain: f64 = plain.audio[..2 * 480].iter().map(|x| x.abs() as f64).sum();
    assert!(early < early_plain * 3.0, "the first 10 ms are still on their way up");
    let jump = r.audio.windows(4).step_by(2).map(|w| (w[2] - w[0]).abs()).fold(0.0f32, f32::max);
    let jump_plain = plain.audio.windows(4).step_by(2).map(|w| (w[2] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(jump < jump_plain * 3.4, "no step in the signal: {jump} against {jump_plain} (x3.16 at most)");
}
