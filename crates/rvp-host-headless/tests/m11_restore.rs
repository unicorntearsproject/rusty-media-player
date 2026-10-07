//! M11 polish on the headless host: the queue and the playback position come back after a restart (also when the host's file ids
//! do not last, as in a browser), and the cover thumbnails stay inside a memory budget while every cover can still be shown.
use rvp_app::{App, Effect, POSITION_KEY, QUEUE_KEY};
use rvp_host::{HostClock, ScriptedLibrary};
use rvp_host_headless::{DefaultCodecs, UiHost, walk_listing};
use rvp_library::Library;
use rvp_player::Repeat;
use rvp_ui::{Action, Enqueue, LibAction, MediaState, Scope, UiConfig};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Once;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures_dir() -> PathBuf {
    std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root().join("target/fixtures"))
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

fn music() -> PathBuf {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let st = Command::new("bash")
            .arg(root().join("tools/gen-fixtures.sh"))
            .arg(fixtures_dir())
            .env("RVP_FIXTURE_SET", "library")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(
            st.success(),
            "fixture generation failed (ffmpeg and python3 needed; RVP_SKIP_FIXTURES=1 skips)"
        );
    });
    fixtures_dir().join("library/music")
}

/// Three 40 second FLAC files in a scratch directory.
/// The three 40 s songs of these tests, made once per process: the tests run on threads of one process, and a test that found a file another
/// was still writing (ffmpeg creates it at once and fills it over the next moments) would open half a song and see the player fail. The files
/// are written under another name and renamed when whole, so a file that exists is a file that is complete.
fn long_files() -> Vec<String> {
    static FILES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    FILES
        .get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("rvp-m11-restore-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            (1..=3)
                .map(|i| {
                    let p = dir.join(format!("long-{i}.flac"));
                    if !p.exists() {
                        let tmp = dir.join(format!("long-{i}.partial.flac"));
                        let st = Command::new("ffmpeg")
                            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
                            .arg(format!("sine=frequency={}:duration=40:sample_rate=44100", 220 * i))
                            .arg(&tmp)
                            .status()
                            .expect("ffmpeg");
                        assert!(st.success());
                        std::fs::rename(&tmp, &p).unwrap();
                    }
                    p.to_string_lossy().into_owned()
                })
                .collect()
        })
        .clone()
}

struct Rig {
    host: UiHost,
    app: App,
}

impl Rig {
    fn with(mut host: UiHost) -> Self {
        host.library.get_or_insert_with(ScriptedLibrary::default);
        Rig { host, app: App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true }) }
    }

    fn new() -> Self {
        Self::with(UiHost::new())
    }

    /// A new app (a restart) on the storage of `old`.
    fn restart(old: Rig, stable: bool) -> Self {
        let mut host = UiHost::new();
        host.stable = stable;
        host.storage.0 = old.host.storage.0;
        Self::with(host)
    }

    fn step(&mut self) -> Vec<Effect> {
        self.app.tick(&mut self.host);
        self.host.virtual_clock().advance(16_000);
        self.app.take_effects()
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.step();
        }
    }

    fn act(&mut self, a: Action) {
        let now = self.host.virtual_clock().now_us();
        self.app.apply(&mut self.host, a, now);
    }

    fn open(&mut self, paths: &[String]) {
        let items: Vec<(String, String)> = paths
            .iter()
            .map(|p| (p.clone(), Path::new(p).file_name().unwrap().to_string_lossy().into_owned()))
            .collect();
        self.app.open_items(&mut self.host, &items, false);
    }

    fn pos_ms(&self) -> i64 {
        self.app.model().position_us / 1000
    }

    fn current_name(&self) -> String {
        self.app.playlist().current().map(|i| i.name.clone()).unwrap_or_default()
    }
}

#[test]
fn the_queue_and_the_position_come_back_after_a_restart_paused() {
    if skip() {
        return;
    }
    let files = long_files();
    let mut r = Rig::new();
    r.open(&files);
    r.run(500);
    r.act(Action::Next); // second file
    r.run(14_000);
    assert_eq!(r.current_name(), "long-2.flac");
    let at = r.pos_ms();
    assert!((12_000..16_000).contains(&at), "{at}");
    r.app.save_state(&mut r.host);
    assert!(r.host.storage.0.contains_key(QUEUE_KEY) && r.host.storage.0.contains_key(POSITION_KEY));

    // Restart: the same queue, the same file, paused where it was, and it stays there.
    let mut r2 = Rig::restart(r, true);
    r2.run(600);
    assert_eq!(r2.app.playlist().len(), 3);
    assert_eq!(r2.current_name(), "long-2.flac");
    assert_eq!(r2.app.model().state, MediaState::Paused);
    let restored = r2.pos_ms();
    assert!((restored - at).abs() < 700, "restored at {restored} ms, saved at {at} ms");
    r2.run(2000);
    assert_eq!(r2.pos_ms(), restored, "paused means paused");
    assert!(r2.host.audio.frames_written() > 0);
    // Playing goes on from there.
    r2.act(Action::PlayPause);
    r2.run(1500);
    assert!(r2.pos_ms() > restored + 1000);
}

#[test]
fn shuffle_repeat_and_the_current_item_are_kept() {
    if skip() {
        return;
    }
    let files = long_files();
    let mut r = Rig::new();
    r.open(&files);
    r.run(300);
    r.act(Action::CycleRepeat);
    r.act(Action::ToggleShuffle);
    r.run(300);
    let (cur, repeat) = (r.current_name(), r.app.playlist().repeat());
    assert_eq!(repeat, Repeat::All);
    r.app.save_state(&mut r.host);
    let mut r2 = Rig::restart(r, true);
    r2.run(400);
    assert_eq!(r2.current_name(), cur);
    assert_eq!(r2.app.playlist().repeat(), Repeat::All);
    assert!(r2.app.playlist().shuffle());
}

#[test]
fn something_opened_on_the_command_line_wins_over_the_saved_queue() {
    if skip() {
        return;
    }
    let files = long_files();
    let mut r = Rig::new();
    r.open(&files);
    r.run(300);
    r.app.save_state(&mut r.host);
    let mut r2 = Rig::restart(r, true);
    r2.open(&files[2..]); // before the first tick, as `rvp file` does
    r2.run(600);
    assert_eq!(r2.app.playlist().len(), 1);
    assert_eq!(r2.current_name(), "long-3.flac");
    assert_eq!(r2.app.model().state, MediaState::Playing);
    // And the new queue is what is saved now.
    let saved = rvp_app::SavedQueue::decode(&r2.host.storage.0[QUEUE_KEY]).unwrap();
    assert_eq!(saved.items.len(), 1);
}

#[test]
fn a_bad_or_empty_saved_queue_starts_clean() {
    if skip() {
        return;
    }
    let mut host = UiHost::new();
    host.storage.0.insert(QUEUE_KEY.into(), b"RVQ1garbage".to_vec());
    host.storage.0.insert(POSITION_KEY.into(), vec![1, 2, 3]);
    let mut r = Rig::with(host);
    r.run(300);
    assert!(r.app.playlist().is_empty());
    assert_eq!(r.app.model().state, MediaState::Idle);
}

#[test]
fn library_tracks_wait_for_their_folder_when_the_hosts_ids_do_not_last() {
    if skip() {
        return;
    }
    // A browser-like host: file ids belong to one session, so only the library's track ids are saved.
    let mut host = UiHost::new();
    host.stable = false;
    let mut r = Rig::with(host);
    let listing = walk_listing("music", "music", &music());
    r.host.library.as_mut().unwrap().listings.push_back(listing.clone());
    r.run(6000);
    let album = r.app.library().albums().iter().find(|a| a.title == "Daybreak").unwrap().clone();
    r.act(Action::Lib(LibAction::Play(Scope::Album(album.id), Enqueue::Now)));
    r.act(Action::Next);
    r.act(Action::Next);
    r.run(300);
    let (len, cur) = (r.app.playlist().len(), r.current_name());
    assert_eq!(len, 10);
    r.app.save_state(&mut r.host);
    let saved = rvp_app::SavedQueue::decode(&r.host.storage.0[QUEUE_KEY]).unwrap();
    assert!(saved.items.iter().all(|i| i.source.is_empty() && i.track.is_some()), "no session ids are saved");

    let mut r2 = Rig::restart(r, false);
    r2.run(600);
    assert_eq!(r2.app.playlist().len(), 10, "the queue is back");
    assert_eq!(r2.app.model().state, MediaState::Idle, "but nothing can open until the folder is listed");
    r2.host.library.as_mut().unwrap().listings.push_back(listing);
    r2.run(3000);
    assert_eq!(r2.current_name(), cur);
    assert_eq!(r2.app.model().state, MediaState::Paused);
}

#[test]
fn thumbnails_stay_inside_their_memory_budget_and_come_back_when_asked_for() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    let thumb = 144 * 144 * 3;
    r.app.set_thumb_budget(thumb * 3 + 100);
    let listing = walk_listing("music", "music", &music());
    r.host.library.as_mut().unwrap().listings.push_back(listing);
    r.run(8000);
    let lib: &Library = r.app.library();
    assert_eq!(lib.track_count(), 201);
    let arts: Vec<u64> = lib.albums().iter().map(|a| a.art).filter(|&a| a != 0).collect();
    assert!(arts.len() >= 10, "{}", arts.len());
    assert!(lib.thumb_bytes() <= thumb * 3 + 100, "{} bytes in memory", lib.thumb_bytes());
    assert!(lib.thumb_evictions() > 0, "a budget this small must have dropped some");
    assert!(!lib.thumbs_need_save());
    // Every cover is still there: each one a view asks for is read back from storage within a few ticks, and memory holds.
    for a in arts {
        for _ in 0..60 {
            if r.app.library().thumb(a).is_some() {
                break;
            }
            r.step();
        }
        assert!(r.app.library().thumb(a).is_some(), "cover {a:x} did not come back");
        assert!(r.app.library().thumb_bytes() <= thumb * 3 + 100);
    }
}

/// What went wrong once in a while in `something_opened_on_the_command_line_wins_over_the_saved_queue` was this and not the player: when many
/// tests ask for the songs at the same moment, each must get whole files. (The old helper let a second thread skip generation because the
/// file already existed, empty or half written, and open it: the player then rightly said Failed.) Eight threads ask at once; every file
/// they get is complete, and a song opened from it plays.
#[test]
fn many_tests_asking_for_the_songs_at_once_all_get_whole_files() {
    if skip() {
        return;
    }
    let handles: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                let files = long_files();
                files.iter().map(|f| std::fs::metadata(f).unwrap().len()).collect::<Vec<_>>()
            })
        })
        .collect();
    let sizes: Vec<Vec<u64>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for s in &sizes {
        assert_eq!(s, &sizes[0], "a thread saw a file of another size: {sizes:?}");
        assert!(s.iter().all(|&n| n > 100_000), "an incomplete song: {s:?}");
    }
}

/// The same ordering the other way round, made certain: a command-line open over a saved queue while one of the saved items is broken
/// (a half-written file stands in) does not take the new item down with it: the opened song plays and only the broken one is skipped.
#[test]
fn a_broken_saved_item_does_not_stop_the_command_line_song() {
    if skip() {
        return;
    }
    let files = long_files();
    let mut r = Rig::new();
    r.open(&files);
    r.run(300);
    r.app.save_state(&mut r.host);
    // Replace the first saved song by a few bytes of nothing (what a file that is still being written looks like).
    let broken = Path::new(&files[0]).with_file_name("broken-saved.flac");
    std::fs::write(&broken, [0u8; 64]).unwrap();
    let saved = rvp_app::SavedQueue::decode(&r.host.storage.0[QUEUE_KEY]).unwrap();
    let mut saved = saved;
    saved.items[0].source = broken.to_string_lossy().into_owned();
    r.host.storage.0.insert(QUEUE_KEY.into(), saved.encode());
    let mut r2 = Rig::restart(r, true);
    r2.open(&files[2..]);
    r2.run(600);
    assert_eq!(r2.current_name(), "long-3.flac");
    assert_eq!(r2.app.model().state, MediaState::Playing);
}
