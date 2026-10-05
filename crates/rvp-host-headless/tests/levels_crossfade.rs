//! Crossfade between queue items, through the whole session: what the sink gets (sums on generated tones, timing, the clock and
//! now-playing switch in the middle of the fade), the cases that must stay gapless (the setting off, tracks of a gapless
//! album, video, tracks too short), and the visualizer tap seeing the mixture. Fixtures: `tools/gen-fixtures.sh`, set `levels`.
use rvp_core::AudioSettings;
use rvp_host_headless::{PlayOptions, PlayReport, SessionEvent, SessionState, play_file};
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

fn settings(fade_secs: Option<u8>) -> AudioSettings {
    AudioSettings {
        crossfade: fade_secs.is_some(),
        crossfade_secs: fade_secs.unwrap_or(5),
        ..AudioSettings::default()
    }
}

fn play(first: &str, rest: &[&str], settings: AudioSettings) -> PlayReport {
    let opts = PlayOptions {
        chain: rest.iter().map(|p| fixture(p)).collect(),
        audio_settings: settings,
        ..Default::default()
    };
    let r = play_file(&fixture(first), &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    r
}

fn alone(name: &str) -> Vec<f32> {
    play(name, &[], AudioSettings::default()).audio
}

fn item_started_at(r: &PlayReport) -> Vec<(i64, u32)> {
    r.events
        .iter()
        .filter_map(|(t, e)| match e {
            SessionEvent::ItemStarted { tag } => Some((*t, *tag)),
            _ => None,
        })
        .collect()
}

/// First frame at which `out` and `reference` differ.
fn first_difference(out: &[f32], reference: &[f32]) -> usize {
    out.iter().zip(reference).position(|(a, b)| a != b).map_or(out.len().min(reference.len()), |i| i / 2)
}

#[test]
fn crossfade_mixes_the_end_of_one_track_into_the_start_of_the_next() {
    if skip() {
        return;
    }
    let (a, b) = (alone("xf_a.flac"), alone("xf_b.flac"));
    let (a_len, b_len) = (a.len() / 2, b.len() / 2);
    assert_eq!((a_len, b_len), (384_000, 384_000));
    let r = play("xf_a.flac", &["xf_b.flac"], settings(Some(2)));
    assert!(r.crossfaded);
    let out = &r.audio;
    // The output is the first track to the start of the fade, the mixture, and the second track from the start of the fade on:
    // the second track is not shifted or cut, it just begins early. So the fade starts where the output is as long as the
    // second track is from there on.
    let s0 = out.len() / 2 - b_len;
    // The fade begins two seconds before the end of the first track (when the feed has got there: within a buffer or two).
    assert!((a_len - 96_000..a_len - 96_000 + 10_000).contains(&s0), "fade starts at frame {s0}");
    assert!(first_difference(out, &a) >= s0, "nothing changed before the fade");
    let n = a_len - s0;
    assert_eq!(&out[..s0 * 2], &a[..s0 * 2]);
    for k in 0..n {
        let t = (k as f32 + 0.5) / n as f32;
        let (ga, gb) = rvp_core::dynamics::equal_power(t);
        for c in 0..2 {
            let want = a[(s0 + k) * 2 + c] * ga + b[k * 2 + c] * gb;
            let got = out[(s0 + k) * 2 + c];
            assert!((got - want).abs() < 2e-3, "frame {k} of the fade: {got} vs {want}");
        }
    }
    // After the fade it is the second track on its own, exactly.
    let tail = (s0 + n + 2) * 2;
    for (i, &x) in out[tail..].iter().enumerate() {
        assert_eq!(x, b[tail - s0 * 2 + i], "after the fade, sample {i}");
    }
    // Unrelated tones keep their power through the join (equal-power curve): 100 ms windows within 0.4 dB of the steady level.
    let rms = |from: usize| {
        let w = &out[from * 2..(from + 4800) * 2];
        (w.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / w.len() as f64).sqrt()
    };
    let steady = rms(s0 - 9600);
    for w in 0..(n / 4800).saturating_sub(1) {
        let db = 20.0 * (rms(s0 + w * 4800) / steady).log10();
        assert!(db.abs() < 0.4, "window {w}: {db} dB");
    }
}

#[test]
fn the_clock_and_now_playing_switch_in_the_middle_of_the_fade() {
    if skip() {
        return;
    }
    let plain = play("xf_a.flac", &["xf_b.flac"], settings(None));
    let faded = play("xf_a.flac", &["xf_b.flac"], settings(Some(2)));
    let (p, f) = (item_started_at(&plain), item_started_at(&faded));
    assert_eq!((p.len(), f.len()), (1, 1));
    assert_eq!(f[0].1, 1);
    // Gapless: the second item is heard when the first has ended. With the fade it is heard half the fade (about 1 s) sooner.
    let sooner = p[0].0 - f[0].0;
    assert!((850_000..1_150_000).contains(&sooner), "the switch came {sooner} us before the gapless one");
    // From the switch on the position is the second track's own: about 1 s in, then running with the clock.
    let (t_switch, _) = f[0];
    let at = |dt: i64| {
        faded.positions.iter().find(|(t, _)| *t >= t_switch + dt).map(|(t, p)| (*t - t_switch, *p)).unwrap()
    };
    for dt in [0, 500_000, 2_000_000] {
        let (got_dt, pos) = at(dt);
        let want = 1_000_000 + got_dt;
        assert!(
            (pos - want).abs() < 250_000,
            "{dt} us after the switch the position is {pos}, wanted about {want}"
        );
    }
    // Before it the clock is the first track's, running on past the 5 s mark.
    let before = faded.positions.iter().rfind(|(t, _)| *t < t_switch - 200_000).unwrap().1;
    assert!(before > 5_000_000 && before < 7_500_000, "{before}");
    // The state never left Playing in between and the position never ran backwards inside an item.
    let mut last = (0, 0);
    for &(t, pos) in &faded.positions {
        if t < t_switch && t > 500_000 {
            assert!(pos >= last.1, "position went back at {t}");
        }
        last = (t, pos);
    }
}

#[test]
fn off_means_the_gapless_path_exactly() {
    if skip() {
        return;
    }
    let (a, b) = (alone("xf_a.flac"), alone("xf_b.flac"));
    let r = play("xf_a.flac", &["xf_b.flac"], settings(None));
    assert!(!r.crossfaded);
    let want: Vec<f32> = a.iter().chain(&b).copied().collect();
    assert_eq!(r.audio.len(), want.len());
    assert!(r.audio == want, "crossfade off must be the plain join, sample for sample");
    // Auto-level off too (the default): nothing touches the samples.
    let r = play("xf_a.flac", &["xf_b.flac", "xf_c.flac"], AudioSettings::default());
    assert_eq!(r.audio.len(), 3 * a.len());
}

#[test]
fn tracks_of_one_gapless_album_run_into_each_other() {
    if skip() {
        return;
    }
    let (a, b) = (alone("gl_1.flac"), alone("gl_2.flac"));
    let r = play("gl_1.flac", &["gl_2.flac"], settings(Some(4)));
    assert!(!r.crossfaded, "consecutive tracks of a gapless album are not faded");
    let want: Vec<f32> = a.iter().chain(&b).copied().collect();
    assert!(r.audio == want);
    // Track 5 after track 1 of the same album is not "consecutive": faded.
    let r = play("gl_1.flac", &["gl_5.flac"], settings(Some(4)));
    assert!(r.crossfaded);
    assert!(r.audio.len() < a.len() + alone("gl_5.flac").len());
}

#[test]
fn short_tracks_cap_the_fade_and_tiny_ones_stay_gapless() {
    if skip() {
        return;
    }
    let a = alone("xf_a.flac");
    // A 1.5 s track gets a fade of at most a third of its length: 0.5 s, whatever the setting says.
    let short = alone("xf_short.flac");
    let r = play("xf_a.flac", &["xf_short.flac"], settings(Some(10)));
    assert!(r.crossfaded);
    let s0 = r.audio.len() / 2 - short.len() / 2;
    let n = a.len() / 2 - s0;
    // (It starts when the audio fed has passed the point, so up to one 4096-frame FLAC block shorter.)
    assert!((19_800..=24_000).contains(&n), "the fade is {n} frames, not 0.5 s");
    // 0.6 s is too short for a fade worth hearing (a third is 0.2 s): the join stays gapless.
    let tiny = alone("xf_tiny.flac");
    let r = play("xf_a.flac", &["xf_tiny.flac"], settings(Some(10)));
    assert!(!r.crossfaded);
    let want: Vec<f32> = a.iter().chain(&tiny).copied().collect();
    assert!(r.audio == want);
    // And a short track first: the fade is limited by its own length too.
    let r = play("xf_short.flac", &["xf_a.flac"], settings(Some(10)));
    assert!(r.crossfaded);
    let s0 = r.audio.len() / 2 - a.len() / 2;
    let n = short.len() / 2 - s0;
    assert!((19_800..=24_000).contains(&n), "{n}");
}

#[test]
fn items_with_a_picture_are_never_faded() {
    if skip() {
        return;
    }
    // A video with sound into a song, and a song into a video: both joins are as they always were.
    let video = play("xf_video.mp4", &[], AudioSettings::default());
    let song = alone("xf_a.flac");
    let r = play("xf_video.mp4", &["xf_a.flac"], settings(Some(3)));
    assert!(!r.crossfaded);
    assert_eq!(r.audio.len(), video.audio.len() + song.len(), "nothing overlapped");
    let r = play("xf_a.flac", &["xf_video.mp4"], settings(Some(3)));
    assert!(!r.crossfaded);
    assert_eq!(r.audio.len(), video.audio.len() + song.len());
}

#[test]
fn a_seek_during_the_fade_drops_it_cleanly() {
    if skip() {
        return;
    }
    let opts = PlayOptions {
        chain: vec![fixture("xf_b.flac")],
        audio_settings: settings(Some(2)),
        // The fade starts at about 6 s; seek back to 1 s while it is running.
        seeks: vec![(6_500_000, 1_000_000)],
        ..Default::default()
    };
    let r = play_file(&fixture("xf_a.flac"), &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended, "{:?}", r.error);
    assert!(r.crossfaded);
    assert!(r.audio.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    // Nothing of the second track leaked into the replay of the first (its tone is 880 Hz): after the seek the audio is the
    // first track again, so the mixture does not appear twice.
    let a = alone("xf_a.flac");
    let replay_start = r.audio.len() / 2 - (a.len() / 2 - 48_000);
    assert!(
        r.audio[replay_start * 2..] == a[48_000 * 2..],
        "after the seek the first track plays on from the 1 s mark, untouched"
    );
}

#[test]
fn the_visualizer_tap_sees_the_mixture() {
    if skip() {
        return;
    }
    let opts = PlayOptions {
        chain: vec![fixture("xf_b.flac")],
        audio_settings: settings(Some(2)),
        visualizer: true,
        ..Default::default()
    };
    let r = play_file(&fixture("xf_a.flac"), &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended);
    // What was heard is what the tap got: every frame of the mixed output.
    assert!(
        r.viz_frames as i64 >= r.audio.len() as i64 / 2 - 4096,
        "{} vs {}",
        r.viz_frames,
        r.audio.len() / 2
    );
    // 440 Hz sits near band 13 and 880 Hz near band 17: first only the one, in the middle of the fade both, at the end the other.
    let bands = |from: usize, to: usize| {
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        let w: Vec<_> = r.viz.iter().filter(|s| s.pts_us >= from as i64 && s.pts_us < to as i64).collect();
        for s in &w {
            lo = lo.max(s.bands[12..15].iter().copied().fold(0.0, f32::max));
            hi = hi.max(s.bands[16..19].iter().copied().fold(0.0, f32::max));
        }
        (lo, hi, w.len())
    };
    let (lo, hi, n) = bands(2_000_000, 3_000_000);
    assert!(n > 20 && lo > 0.6 && hi < 0.2, "{lo} {hi}");
    // The summaries carry the stream time of the item they belong to: the second item's time restarts at the middle of the fade
    // (2 s in), where the first item's own time was 6 s. Both tones are there around the middle of the fade.
    let mid = r.viz.iter().filter(|s| s.pts_us > 5_500_000 && s.pts_us < 6_500_000).collect::<Vec<_>>();
    let both = mid
        .iter()
        .filter(|s| s.bands[12..15].iter().any(|&v| v > 0.4) && s.bands[16..19].iter().any(|&v| v > 0.4))
        .count();
    assert!(both > 10, "{both} of {} summaries show both tones in the middle of the fade", mid.len());
}
