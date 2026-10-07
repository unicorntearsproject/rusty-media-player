//! The play history on the headless host: a play counts after 30 s of listening, a short skip does not, the pause setting stops the
//! recording, and what was recorded survives a restart and a clear.
use rvp_app::{APP_SETTINGS_KEY, App};
use rvp_host::{HostClock, ScriptedLibrary};
use rvp_host_headless::{DefaultCodecs, UiHost, walk_listing};
use rvp_library::HISTORY_KEY;
use rvp_ui::{Action, Enqueue, LibAction, Scope, UiConfig};
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

/// Three 40 s songs in a scratch folder, written once per process (under another name, renamed when whole).
fn songs_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("rvp-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 1..=3 {
            let p = dir.join(format!("song-{i}.flac"));
            let tmp = dir.join(format!("song-{i}.partial.flac"));
            let st = Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
                .arg(format!("sine=frequency={}:duration=40:sample_rate=44100", 220 * i))
                .arg(&tmp)
                .status()
                .expect("ffmpeg");
            assert!(st.success());
            std::fs::rename(&tmp, &p).unwrap();
        }
        dir
    })
    .clone()
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

struct Rig {
    host: UiHost,
    app: App,
}

impl Rig {
    fn with(mut host: UiHost) -> Self {
        host.library.get_or_insert_with(ScriptedLibrary::default);
        let mut r =
            Rig { host, app: App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true }) };
        r.host.library.as_mut().unwrap().listings.push_back(walk_listing("music", "music", &songs_dir()));
        r.run(3000);
        r
    }

    fn new() -> Self {
        Self::with(UiHost::new())
    }

    fn restart(old: Rig) -> Self {
        let mut host = UiHost::new();
        host.storage.0 = old.host.storage.0;
        Self::with(host)
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.app.tick(&mut self.host);
            self.host.virtual_clock().advance(16_000);
            self.app.take_effects();
        }
    }

    fn act(&mut self, a: Action) {
        let now = self.host.virtual_clock().now_us();
        self.app.apply(&mut self.host, a, now);
    }

    fn track_id(&self, n: usize) -> u32 {
        let mut ids: Vec<(String, u32)> =
            self.app.library().all_tracks().iter().map(|t| (t.path.clone(), t.id)).collect();
        ids.sort();
        ids[n].1
    }

    fn play(&mut self, n: usize) {
        let id = self.track_id(n);
        self.act(Action::Lib(LibAction::Play(Scope::Track(id), Enqueue::Now)));
    }
}

#[test]
fn a_play_counts_after_half_a_minute_and_a_short_skip_does_not() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    assert_eq!(r.app.library().all_tracks().len(), 3);
    // Song 1 is heard for 35 s, then skipped: one play.
    r.play(0);
    r.run(35_000);
    r.play(1);
    r.run(4000);
    let a = r.track_id(0);
    assert_eq!(r.app.library().play_count(a), 1);
    let plays = r.app.library().plays().to_vec();
    assert_eq!(plays.len(), 1);
    assert!((30_000..37_000).contains(&plays[0].heard_ms), "heard {} ms", plays[0].heard_ms);
    assert!(!plays[0].finished);
    assert!(plays[0].at > 1_700_000_000, "dated by the host's calendar: {}", plays[0].at);
    // Song 2 was left after 4 s: not a play. Song 3 plays through to its end: a finished play.
    r.play(2);
    r.run(43_000);
    let (b, c) = (r.track_id(1), r.track_id(2));
    assert_eq!(r.app.library().play_count(b), 0);
    assert_eq!(r.app.library().play_count(c), 1);
    let last = r.app.library().plays().last().unwrap().clone();
    assert!(last.finished && last.heard_ms >= 38_000, "{last:?}");
    // A second listen to song 1 raises its count to 2 and puts it first in "last played".
    r.play(0);
    r.run(31_000);
    r.play(1);
    r.run(500);
    assert_eq!(r.app.library().play_count(a), 2);
    assert_eq!(r.app.library().history_rows(false).first().unwrap().count, 2);
}

#[test]
fn the_history_survives_a_restart_and_a_clear() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.play(0);
    r.run(32_000);
    r.app.save_state(&mut r.host);
    assert!(r.host.storage.0.contains_key(HISTORY_KEY), "saved");
    let mut r2 = Rig::restart(r);
    let a = r2.track_id(0);
    assert_eq!(r2.app.library().play_count(a), 1);
    assert_eq!(r2.app.library().history_rows(false).len(), 1);
    // Clearing asks first; the question's confirm button wipes the history.
    r2.act(Action::Lib(LibAction::ClearHistory));
    assert_eq!(r2.app.library().play_count(a), 1, "nothing happens before the answer");
    r2.run(100);
    assert!(r2.app.model().dialog.is_some());
    let clicked = r2
        .app
        .model()
        .dialog
        .as_ref()
        .unwrap()
        .buttons
        .iter()
        .position(|b| b.label == "Clear history")
        .unwrap();
    r2.act(Action::DialogButton(clicked as u8));
    assert_eq!(r2.app.library().play_count(a), 0);
    assert!(r2.app.library().plays().is_empty());
    let r3 = Rig::restart(r2);
    assert!(r3.app.library().plays().is_empty(), "the cleared history stays cleared");
}

#[test]
fn removing_one_play_keeps_the_others() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.play(0);
    r.run(31_000);
    r.play(1);
    r.run(31_000);
    r.play(2);
    r.run(500);
    assert_eq!(r.app.library().plays().len(), 2);
    let seq = r.app.library().history_rows(false)[0].play.seq;
    r.act(Action::Lib(LibAction::RemoveFromHistory(seq)));
    assert_eq!(r.app.library().plays().len(), 1);
    let a = r.track_id(0);
    assert_eq!(r.app.library().play_count(a), 1);
}

#[test]
fn paused_history_records_nothing() {
    if skip() {
        return;
    }
    let mut host = UiHost::new();
    host.storage.0.insert(APP_SETTINGS_KEY.into(), b"rvp-app-settings 1\nhistory_paused=1\n".to_vec());
    let mut r = Rig::with(host);
    r.play(0);
    r.run(36_000);
    r.play(1);
    r.run(500);
    assert!(r.app.library().plays().is_empty());
}
