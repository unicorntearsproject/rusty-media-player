//! Gapless chains with items that have no audio, or that are not ready in time: they start right away and the picture does not
//! stutter. Driven against a session in virtual time, with the host time of every frame recorded.
use rvp_host_headless::{FileSource, HeadlessHost, SessionEvent, SessionState};
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
            .env("RVP_FIXTURE_SET", "m8")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    dir().join("m8").join(name).to_string_lossy().into_owned()
}

/// A file that is slow to arrive: every read waits until the host clock reaches `ready_at` (a slow disk or network).
struct Slow {
    inner: FileSource,
    clock: Rc<rvp_host_headless::VirtualClock>,
    ready_at: i64,
}

impl rvp_host::Source for Slow {
    async fn size(&self) -> Option<u64> {
        self.inner.size().await
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, rvp_host::HostError> {
        while rvp_host::HostClock::now_us(&*self.clock) < self.ready_at {
            rvp_core::task::yield_now().await;
        }
        self.inner.read_at(offset, buf).await
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}

/// What a chain run saw.
struct Run {
    /// (host time, frame pts) of every picture shown.
    frames: Vec<(i64, i64)>,
    /// Host time of each `ItemStarted`, and its tag.
    started: Vec<(i64, u32)>,
    audio: Vec<f32>,
    end: i64,
    state: SessionState,
    dropped: u64,
}

/// Play `first` and then `rest` as a chain. Each next item is queued when the session asks, and its file arrives `delay_us` later
/// (a slow disk or network: the item is not there when it is needed).
fn run(first: &str, rest: &[&str], delay_us: i64) -> Run {
    let mut host = HeadlessHost::new();
    host.audio.capture = Some(Vec::new());
    let clock = host.virtual_clock();
    let codecs = rvp_host_headless::DefaultCodecs {
        stall: None,
        clock: Some(clock.clone()),
        video_cost_us: 0,
        platform: None,
        ..Default::default()
    };
    let mut session = Session::new(FileSource::open(&fixture(first)).unwrap(), Rc::new(codecs));
    session.play();
    let mut chain = rest.iter();
    let (mut tag, mut seen, mut wanted_at) = (0u32, 0usize, None::<i64>);
    let (mut frames, mut started) = (Vec::new(), Vec::new());
    loop {
        session.tick(&mut host);
        let now = rvp_host::HostClock::now_us(&*clock);
        if std::env::var_os("RVP_TRACE").is_some()
            && (3_900_000..5_100_000).contains(&now)
            && now % 50_000 == 0
        {
            println!(
                "{now}: {:?} pos {} frames {}",
                session.state(),
                session.position_us(now),
                host.video.frames.len()
            );
        }
        while seen < host.video.frames.len() {
            frames.push((now, host.video.frames[seen].0));
            seen += 1;
        }
        while let Some(e) = session.poll_event() {
            if let SessionEvent::ItemStarted { tag } = e {
                started.push((now, tag));
            }
        }
        if session.wants_next(now) {
            if let Some(p) = chain.next() {
                tag += 1;
                let src = Slow {
                    inner: FileSource::open(&fixture(p)).unwrap(),
                    clock: clock.clone(),
                    ready_at: now + delay_us,
                };
                session.queue_next(src, tag);
                let _ = &mut wanted_at;
            }
        }
        if matches!(session.state(), SessionState::Ended | SessionState::Failed) || now > 60_000_000 {
            break;
        }
        clock.advance(std::env::var("RVP_TICK").ok().and_then(|v| v.parse().ok()).unwrap_or(5_000));
    }
    let end = rvp_host::HostClock::now_us(&*clock);
    Run {
        frames,
        started,
        audio: host.audio.capture.take().unwrap_or_default(),
        end,
        state: session.state(),
        dropped: session.video_stats().dropped,
    }
}

/// A solo run of one file: how many audio frames and pictures it makes on its own.
fn solo(name: &str) -> (usize, usize) {
    let r = run(name, &[], 0);
    assert_eq!(r.state, SessionState::Ended, "{name}");
    (r.audio.len() / 2, r.frames.len())
}

/// What each chain is: the files, and whether each has a picture.
const CHAINS: &[(&str, &[&str])] = &[
    ("av_2s.mkv", &["video_only.mkv"]),
    ("video_only.mkv", &["av_2s.mkv"]),
    ("video_only.mkv", &["video_only.mp4"]),
    ("av_2s.mkv", &["video_only.mkv", "av_2s_opus.mkv"]),
    ("gap_0.mkv", &["video_only.mkv", "gap_1.mkv"]),
    ("av_2s.mkv", &["av_2s_opus.mkv"]),
];

fn has_video(name: &str) -> bool {
    !name.starts_with("gap_")
}

/// The longest wait between two pictures on the screen at host speed (virtual time), in the frames of `r`.
fn longest_wait(r: &Run) -> i64 {
    r.frames.windows(2).map(|w| w[1].0 - w[0].0).max().unwrap_or(0)
}

#[test]
fn a_next_item_without_audio_starts_at_once_and_nothing_stutters() {
    if skip() {
        return;
    }
    for (first, rest) in CHAINS {
        let r = run(first, rest, 0);
        let all: Vec<&str> = std::iter::once(*first).chain(rest.iter().copied()).collect();
        assert_eq!(r.state, SessionState::Ended, "{all:?}");
        assert_eq!(r.started.len(), rest.len(), "{all:?}: every item announces itself");
        assert_eq!(r.dropped, 0, "{all:?}: no frame is dropped");
        // Everything is played: the pictures and the sound of each file, as on its own, with no stale sound from an earlier item.
        let (audio, frames) = all.iter().map(|n| solo(n)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        assert_eq!(r.audio.len() / 2, audio, "{all:?}: audio frames");
        assert_eq!(r.frames.len(), frames, "{all:?}: pictures");
        // Pictures keep coming at the frame rate, also over the joins: never more than 1.5 frames (40 ms each) apart.
        assert!(longest_wait(&r) <= 60_000, "{all:?}: a picture waited {} us", longest_wait(&r));
        // Each item with a picture shows its first one right after it starts.
        let mut before = solo(first).1;
        for (&(at, _), name) in r.started.iter().zip(rest.iter()) {
            if has_video(name) {
                let first_pic = r.frames[before];
                assert!(
                    first_pic.0 >= at && first_pic.0 - at <= 60_000,
                    "{all:?}: {name} showed its first picture {} us after starting",
                    first_pic.0 - at
                );
            }
            before += solo(name).1;
        }
        // And the whole thing takes as long as its items put together (plus a start-up buffer of the first), not longer.
        let sum: i64 = all.iter().map(|_| 2_000_000).sum();
        assert!(r.end <= sum + 250_000, "{all:?}: {} us for {sum} us of media", r.end);
    }
}

#[test]
fn a_next_item_that_is_late_starts_the_moment_it_is_there() {
    if skip() {
        return;
    }
    // The file of the next item arrives 2.3 s after it is asked for: the current item (2 s) is over by then.
    const LATE: i64 = 2_300_000;
    for (first, rest) in CHAINS {
        let r = run(first, rest, LATE);
        let all: Vec<&str> = std::iter::once(*first).chain(rest.iter().copied()).collect();
        assert_eq!(r.state, SessionState::Ended, "{all:?}");
        assert_eq!(r.dropped, 0, "{all:?}: no frame is dropped");
        let (audio, frames) = all.iter().map(|n| solo(n)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        assert_eq!(r.audio.len() / 2, audio, "{all:?}: audio frames");
        assert_eq!(r.frames.len(), frames, "{all:?}: pictures");
        // Between the late arrivals the pictures run evenly; across one the wait is the lateness and not more than a frame over it.
        let mut waits: Vec<i64> =
            r.frames.windows(2).map(|w| w[1].0 - w[0].0).filter(|w| *w > 60_000).collect();
        waits.sort_unstable();
        for w in &waits {
            assert!(*w <= LATE - 1_900_000 + 80_000 + 100_000, "{all:?}: a picture waited {w} us");
        }
        // The second item shows its first picture within a couple of frames of the file arriving (it was asked for at about 0).
        if has_video(rest[0]) {
            let first_pic = r.frames[solo(first).1];
            assert!(
                first_pic.0 >= LATE && first_pic.0 <= LATE + 120_000,
                "{all:?}: the first picture of {} came at {} us, the file at {LATE} us",
                rest[0],
                first_pic.0
            );
        }
    }
}
