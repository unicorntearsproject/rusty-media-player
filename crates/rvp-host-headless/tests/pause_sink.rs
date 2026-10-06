//! Pausing the session pauses the audio device: what is queued must not play on while the clock is stopped (pause, seek while
//! paused, the wait for the start buffer), on the plain path, in the middle of a crossfade and with the automatic level on.
//! The stream the sink receives must be the same as without the pause. Fixtures: `tools/gen-fixtures.sh`, set `levels`.
use rvp_core::AudioSettings;
use rvp_host::{AudioSink, HostClock};
use rvp_host_headless::{FileSource, HeadlessHost, PlayOptions, SessionState, play_file};
use rvp_player::Session;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
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

#[derive(Clone, Copy)]
enum When {
    /// Once the position passes this many microseconds.
    At(i64),
    /// A little after a crossfade began.
    Fading,
}

/// Play to the end with one pause (held for `hold_us`, with an optional seek while paused) and return what the sink received.
fn run(settings: AudioSettings, chain: &[&str], when: When, hold_us: i64, seek_to: Option<i64>) -> Vec<f32> {
    let mut host = HeadlessHost::new();
    host.audio.capture = Some(Vec::new());
    let clock = host.virtual_clock();
    let codecs = rvp_host_headless::DefaultCodecs::default();
    let mut session = Session::new(FileSource::open(&fixture("xf_a.flac")).unwrap(), Rc::new(codecs));
    session.set_audio_settings(settings);
    session.play();
    let mut chain = chain.iter();
    let mut paused_once = false;
    let mut fading_since = None;
    let step = |session: &mut Session, host: &mut HeadlessHost, chain: &mut std::slice::Iter<&str>| {
        session.tick(host);
        let now = clock.now_us();
        if session.wants_next(now) {
            if let Some(p) = chain.next() {
                session.queue_next(FileSource::open(&fixture(p)).unwrap(), 1);
            }
        }
        clock.advance(5_000);
        now
    };
    for _ in 0..4_000_000 {
        let now = step(&mut session, &mut host, &mut chain);
        if session.state() == SessionState::Ended {
            break;
        }
        assert_ne!(session.state(), SessionState::Failed, "{:?}", session.error());
        if paused_once || session.state() != SessionState::Playing {
            continue;
        }
        let due = match when {
            When::At(t) => session.position_us(now) >= t,
            When::Fading => {
                if session.crossfading() && fading_since.is_none() {
                    fading_since = Some(now);
                }
                fading_since.is_some_and(|t| now - t >= 500_000)
            }
        };
        if !due {
            continue;
        }
        paused_once = true;
        session.pause(now);
        step(&mut session, &mut host, &mut chain);
        assert!(host.audio.is_paused(), "the device is paused with the session");
        let (pos, queued) = (session.position_us(clock.now_us()), host.audio.queued_frames());
        assert!(queued > 0, "something was queued to be kept");
        let mut last_queued = queued;
        let mut sought = false;
        let hold_end = clock.now_us() + hold_us;
        while clock.now_us() < hold_end {
            step(&mut session, &mut host, &mut chain);
            assert!(host.audio.is_paused(), "still paused");
            let q = host.audio.queued_frames();
            assert!(q >= last_queued || sought, "paused queue must not drain ({last_queued} -> {q})");
            last_queued = q;
            if let (Some(to), false) = (seek_to, sought) {
                if clock.now_us() >= hold_end - hold_us / 2 {
                    session.seek(to);
                    sought = true;
                    last_queued = 0;
                }
            }
            if seek_to.is_none() {
                assert_eq!(session.position_us(clock.now_us()), pos, "position frozen while paused");
            }
        }
        if let Some(to) = seek_to {
            assert!(
                (session.position_us(clock.now_us()) - to).abs() < 200_000,
                "paused seek lands at the target"
            );
            assert!(host.audio.is_paused());
        }
        session.play();
        let mut guard = 0;
        while host.audio.is_paused() && guard < 2000 {
            step(&mut session, &mut host, &mut chain);
            guard += 1;
        }
        assert!(!host.audio.is_paused(), "resumed with the session");
    }
    assert_eq!(session.state(), SessionState::Ended, "{:?}", session.error());
    assert!(paused_once, "the pause point was reached");
    host.audio.capture.take().unwrap()
}

fn uninterrupted(settings: AudioSettings, chain: &[&str]) -> Vec<f32> {
    let opts = PlayOptions {
        chain: chain.iter().map(|p| fixture(p)).collect(),
        audio_settings: settings,
        ..Default::default()
    };
    let r = play_file(&fixture("xf_a.flac"), &opts).unwrap();
    assert_eq!(r.state, SessionState::Ended);
    r.audio
}

fn fade() -> AudioSettings {
    AudioSettings { crossfade: true, crossfade_secs: 2, ..AudioSettings::default() }
}

fn levelled() -> AudioSettings {
    AudioSettings { auto_level: true, ..AudioSettings::default() }
}

#[test]
fn pause_and_resume_stop_the_device_and_lose_nothing() {
    if skip() {
        return;
    }
    let plain = AudioSettings::default();
    let got = run(plain, &[], When::At(2_000_000), 2_000_000, None);
    assert!(got == uninterrupted(plain, &[]), "the stream is the same as without the pause");
}

#[test]
fn pause_with_the_automatic_level_on() {
    if skip() {
        return;
    }
    let got = run(levelled(), &[], When::At(1_500_000), 1_000_000, None);
    assert!(got == uninterrupted(levelled(), &[]));
}

#[test]
fn pause_in_the_middle_of_a_crossfade() {
    if skip() {
        return;
    }
    let got = run(fade(), &["xf_b.flac"], When::Fading, 1_500_000, None);
    assert!(got == uninterrupted(fade(), &["xf_b.flac"]));
}

#[test]
fn seek_while_paused_keeps_the_device_paused_until_play() {
    if skip() {
        return;
    }
    let got = run(AudioSettings::default(), &[], When::At(2_000_000), 2_000_000, Some(5_000_000));
    // 8 s of tone, 3 s were heard before the seek and the rest from 5 s.
    assert!(got.len() / 2 < 384_000 && got.len() / 2 > 200_000, "{} frames", got.len() / 2);
}
