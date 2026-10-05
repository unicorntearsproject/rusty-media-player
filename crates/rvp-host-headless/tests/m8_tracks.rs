//! M8: subtitles (embedded in Matroska and MP4, and sidecar files) and audio track switching, through the session.
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

/// A fixture path in the M8 set (`target/fixtures/m8/<name>`).
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

const FRAME_US: i64 = 40_000; // 25 fps

/// What the sidecar and embedded fixtures all say: (start, end, text).
const CUES: [(i64, i64, &str); 3] = [
    (1_000_000, 2_000_000, "Hello"),
    (3_000_000, 4_500_000, "World\ntwo lines"),
    (5_000_000, 5_500_000, "Last one"),
];

/// Events as `(position, text)` flattened: each change of the on-screen text, with the position it was noticed at.
fn changes(r: &rvp_host_headless::PlayReport) -> Vec<(i64, Option<String>)> {
    r.events
        .iter()
        .filter_map(|(_, e)| match e {
            SessionEvent::Subtitle { at_us, text } => Some((*at_us, text.clone())),
            _ => None,
        })
        .collect()
}

/// Cues appear and disappear at the right virtual time (within one frame of 25 fps).
fn check_cues(r: &rvp_host_headless::PlayReport, name: &str) {
    let ch = changes(r);
    // Drop the initial "nothing" if the first change is a clear.
    let mut want: Vec<(i64, Option<String>)> = Vec::new();
    for (s, e, t) in CUES {
        want.push((s, Some(t.to_string())));
        want.push((e, None));
    }
    let got: Vec<&(i64, Option<String>)> = ch.iter().filter(|c| c.1.is_some() || c.0 > 0).collect();
    assert_eq!(got.len(), want.len(), "{name}: {ch:?}");
    for (g, w) in got.iter().zip(&want) {
        assert_eq!(g.1, w.1, "{name}: {ch:?}");
        assert!((g.0 - w.0).abs() <= FRAME_US, "{name}: change at {} us, wanted {} us ({ch:?})", g.0, w.0);
    }
}

#[test]
fn embedded_subrip_in_matroska() {
    if skip() {
        return;
    }
    let path = fixture("subs_srt.mkv");
    let r0 = play_file(&path, &PlayOptions::default()).unwrap();
    assert_eq!(r0.state, SessionState::Ended, "{:?}", r0.error);
    assert!(changes(&r0).is_empty(), "subtitles are off until selected");
    let labels: Vec<_> = r0.subtitle_tracks.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(labels, ["English", "Spanish"]);
    let id = r0.subtitle_tracks[0].id;
    let r = play_file(&path, &PlayOptions { subtitle_track: Some(id), ..Default::default() }).unwrap();
    check_cues(&r, "subs_srt.mkv eng");
    // The second track says something else.
    let r = play_file(
        &path,
        &PlayOptions { subtitle_track: Some(r0.subtitle_tracks[1].id), ..Default::default() },
    )
    .unwrap();
    let texts: Vec<_> = changes(&r).into_iter().filter_map(|c| c.1).collect();
    assert_eq!(texts, ["Hola", "Mundo"]);
}

#[test]
fn embedded_webvtt_in_matroska_and_mov_text_in_mp4() {
    if skip() {
        return;
    }
    for name in ["subs_vtt.mkv", "subs_movtext.mp4"] {
        let path = fixture(name);
        let r0 = play_file(&path, &PlayOptions::default()).unwrap();
        assert_eq!(r0.subtitle_tracks.len(), 1, "{name}");
        let id = r0.subtitle_tracks[0].id;
        let r = play_file(&path, &PlayOptions { subtitle_track: Some(id), ..Default::default() }).unwrap();
        assert_eq!(r.state, SessionState::Ended, "{name}: {:?}", r.error);
        check_cues(&r, name);
    }
}

#[test]
fn sidecar_srt_and_vtt_files_play_against_any_video() {
    if skip() {
        return;
    }
    for sub in ["sub.srt", "sub.vtt"] {
        let r = play_file(
            &fixture("subs_movtext.mp4"),
            &PlayOptions { subtitle_file: Some(fixture(sub)), ..Default::default() },
        )
        .unwrap();
        assert!(r.subtitle_tracks.iter().any(|t| t.external && t.label == sub), "{sub}");
        check_cues(&r, sub);
    }
}

#[test]
fn seeking_back_shows_the_cue_again() {
    if skip() {
        return;
    }
    let opts = PlayOptions {
        subtitle_file: Some(fixture("sub.srt")),
        seeks: vec![(4_800_000, 3_500_000)],
        ..Default::default()
    };
    let r = play_file(&fixture("subs_movtext.mp4"), &opts).unwrap();
    let texts: Vec<_> = changes(&r).into_iter().filter_map(|c| c.1).collect();
    // Hello, World, then after the seek back World again, then Last one.
    assert_eq!(texts, ["Hello", "World\ntwo lines", "World\ntwo lines", "Last one"]);
}

/// The two audio tracks carry 440 Hz and 880 Hz; selecting the second changes what is heard.
#[test]
fn switching_the_audio_track_changes_the_sound() {
    if skip() {
        return;
    }
    let path = fixture("two_audio.mkv");
    let freq = |r: &rvp_host_headless::PlayReport| -> f64 {
        // Frequency from zero crossings of the left channel over the middle of the run.
        let l: Vec<f32> = r.audio.chunks_exact(2).map(|f| f[0]).collect();
        let seg = &l[48_000..l.len() - 48_000];
        let crossings = seg.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        crossings as f64 / (seg.len() as f64 / 48_000.0)
    };
    let r = play_file(&path, &PlayOptions::default()).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    assert!((freq(&r) - 440.0).abs() < 5.0, "first track: {} Hz", freq(&r));
    let r = play_file(&path, &PlayOptions { audio_track: Some(3), ..Default::default() }).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    assert!((freq(&r) - 880.0).abs() < 8.0, "second track: {} Hz", freq(&r));
}

/// Speed changes the duration but not the pitch (WSOLA time stretch): the 440 Hz sine stays at 440 Hz.
#[test]
fn speed_keeps_the_pitch() {
    if skip() {
        return;
    }
    let path = fixture("two_audio.mkv"); // 6 s of 440 Hz in the first track
    for (rate, secs) in [(0.5, 12.0), (0.75, 8.0), (1.5, 4.0), (2.0, 3.0), (3.0, 2.0)] {
        let r = play_file(&path, &PlayOptions { rate: Some(rate), ..Default::default() }).unwrap();
        assert_eq!(r.state, SessionState::Ended, "{rate}x: {:?}", r.error);
        let audio_s = r.audio.len() as f64 / 2.0 / 48_000.0;
        assert!((audio_s - secs).abs() < 0.4, "{rate}x: {audio_s} s of audio, wanted {secs}");
        let l: Vec<f32> = r.audio.chunks_exact(2).map(|f| f[0]).collect();
        let seg = &l[l.len() / 4..l.len() * 3 / 4];
        let crossings = seg.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        let f = crossings as f64 / (seg.len() as f64 / 48_000.0);
        assert!((f - 440.0).abs() < 8.0, "{rate}x: pitch {f} Hz");
    }
}
