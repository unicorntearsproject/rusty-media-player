//! M8: the visualizer tap through the whole session: blocks of what is heard, and the analysis of it.
use rvp_host_headless::{PlayOptions, SessionState, play_file};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

fn fixture(name: &str) -> String {
    static ONCE: Once = Once::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir: PathBuf =
        std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"));
    ONCE.call_once(|| {
        let st = Command::new("bash")
            .arg(root.join("tools/gen-fixtures.sh"))
            .arg(&dir)
            .env("RVP_FIXTURE_SET", "m8")
            .status()
            .expect("run gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    dir.join("m8").join(name).to_string_lossy().into_owned()
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

#[test]
fn the_tap_gets_what_is_heard_and_a_sine_shows_in_its_band() {
    if skip() {
        return;
    }
    let opts = PlayOptions { visualizer: true, ..Default::default() };
    let r = play_file(&fixture("two_audio.mkv"), &opts).unwrap(); // 6 s of 440 Hz
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    let written = r.audio.len() as u64 / 2;
    // Everything written is eventually heard and passed on (a block's worth may remain at the end).
    assert!(
        r.viz_frames + 4_000 >= written && r.viz_frames <= written,
        "{} of {written} frames",
        r.viz_frames
    );
    assert!(r.viz.len() as u64 >= written / 512 - 10);
    // Time stamps are stream time and never go backwards.
    assert!(r.viz.windows(2).all(|w| w[1].pts_us > w[0].pts_us));
    assert!(r.viz.first().unwrap().pts_us.abs() < 50_000);
    assert!((r.viz.last().unwrap().pts_us - 6_000_000).abs() < 100_000, "{}", r.viz.last().unwrap().pts_us);
    // 440 Hz sits in the same band all through the middle of the run, and level is steady.
    let mid: Vec<_> = r.viz.iter().filter(|s| s.pts_us > 1_000_000 && s.pts_us < 5_000_000).collect();
    let loudest = |s: &rvp_host::VizSummary| {
        (0..32).max_by(|&a, &b| s.bands[a].partial_cmp(&s.bands[b]).unwrap()).unwrap()
    };
    let band = loudest(mid[0]);
    assert!(mid.iter().all(|s| loudest(s) == band), "the band moved");
    assert!(mid.iter().all(|s| s.bands[band] > 0.65), "a 0.125 amplitude sine is about -18 dB");
    assert!((12..=17).contains(&band), "440 Hz in band {band}");
    assert!(mid.iter().all(|s| s.level > 0.05 && s.level < 0.5));
}

#[test]
fn clicks_at_120_bpm_are_onsets_and_a_tempo() {
    if skip() {
        return;
    }
    let opts = PlayOptions { visualizer: true, ..Default::default() };
    let r = play_file(&fixture("clicks_120.mkv"), &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    let hop_us = 512 * 1_000_000 / 48_000;
    let onsets: Vec<i64> = r.viz.iter().filter(|s| s.onset).map(|s| s.pts_us).collect();
    // A click every 500 ms from 0 to 7.5 s (the one at 0 has nothing before it to be an onset from).
    let want: Vec<i64> = (1..16).map(|k| k * 500_000).collect();
    assert_eq!(onsets.len(), want.len(), "{onsets:?}");
    for (o, w) in onsets.iter().zip(&want) {
        assert!((o - w).abs() <= hop_us + 1_000, "onset at {o}, click at {w}");
    }
    let tempo = r.viz.last().unwrap().tempo_bpm;
    assert!((tempo - 120.0).abs() < 2.0, "tempo {tempo}");
    assert!(r.viz.iter().filter(|s| s.onset).all(|s| s.onset_strength > 1.0));
}

#[test]
fn without_a_tap_nothing_is_analysed() {
    if skip() {
        return;
    }
    let r = play_file(&fixture("two_audio.mkv"), &PlayOptions::default()).unwrap();
    assert_eq!(r.viz_frames, 0);
    assert!(r.viz.is_empty());
}
