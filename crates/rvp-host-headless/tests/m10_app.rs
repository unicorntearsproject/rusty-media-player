//! M10 end to end on the headless host, in virtual time: the app scans a generated library through the host's directory
//! capability, plays whole albums gapless through the queue (right now-playing metadata per track), switches between the Library
//! and the Player faces, runs the visualizer view (moving with the music, still when paused), makes, saves, exports and imports
//! playlists, and comes back after a restart with its index, covers and playlists.
use rvp_app::{App, Effect};
use rvp_host::{HostClock, InputEvent, Key, Modifiers, RecordingNowPlaying, ScriptedLibrary};
use rvp_host_headless::{DefaultCodecs, FileSource, PlayOptions, UiHost, play_file, walk_listing};
use rvp_ui::{Action, Detail, Enqueue, LibAction, MediaState, Mode, Scope, UiConfig, View};
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
        for set in ["library", "core"] {
            let st = Command::new("bash")
                .arg(root().join("tools/gen-fixtures.sh"))
                .arg(fixtures_dir())
                .env("RVP_FIXTURE_SET", set)
                .status()
                .expect("run tools/gen-fixtures.sh");
            assert!(
                st.success(),
                "fixture generation failed (ffmpeg and python3 needed; RVP_SKIP_FIXTURES=1 skips)"
            );
        }
    });
    fixtures_dir().join("library/music")
}

struct Rig {
    host: UiHost,
    app: App,
    effects: Vec<Effect>,
}

impl Rig {
    fn new() -> Self {
        let mut host = UiHost::new();
        host.library = Some(ScriptedLibrary::default());
        host.now_playing = Some(RecordingNowPlaying::default());
        Self::with(host)
    }

    fn with(host: UiHost) -> Self {
        Self::with_motion(host, true)
    }

    fn with_motion(host: UiHost, reduce_motion: bool) -> Self {
        let app = App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion });
        Rig { host, app, effects: Vec::new() }
    }

    /// A rig whose app has scanned the whole library.
    fn scanned() -> Self {
        let mut r = Rig::new();
        r.add_folder();
        r
    }

    /// The same, with full motion (the visualizer on).
    fn scanned_with_motion() -> Self {
        let mut host = UiHost::new();
        host.library = Some(ScriptedLibrary::default());
        host.now_playing = Some(RecordingNowPlaying::default());
        let mut r = Rig::with_motion(host, false);
        r.add_folder();
        r
    }

    fn add_folder(&mut self) {
        let listing = walk_listing("music", "music", &music());
        self.host.library.as_mut().unwrap().listings.push_back(listing);
        for _ in 0..4000 {
            self.step();
            if self.app.library().track_count() >= 201 && self.app.scan_status().is_none() {
                break;
            }
        }
        // A few more ticks let the thumbnails finish and the index be saved.
        self.run(200);
        assert_eq!(self.app.library().track_count(), 201);
    }

    fn step(&mut self) {
        self.app.tick(&mut self.host);
        self.effects.extend(self.app.take_effects());
        self.host.virtual_clock().advance(16_000);
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.step();
        }
    }

    fn now(&self) -> i64 {
        self.host.virtual_clock().now_us()
    }

    fn act(&mut self, a: Action) {
        let now = self.now();
        self.app.apply(&mut self.host, a, now);
        self.effects.extend(self.app.take_effects());
    }

    fn key(&mut self, k: Key) {
        self.host.input.0.push_back(InputEvent::KeyDown {
            key: k,
            mods: Modifiers::default(),
            repeat: false,
        });
        self.app.pump(&mut self.host);
        self.effects.extend(self.app.take_effects());
    }

    fn album(&self, artist: &str, title: &str) -> rvp_library::Album {
        self.app
            .library()
            .albums()
            .iter()
            .find(|a| a.artist == artist && a.title == title)
            .unwrap_or_else(|| panic!("{artist} / {title}"))
            .clone()
    }

    fn metas(&mut self) -> Vec<(String, String, String)> {
        self.host
            .now_playing
            .as_ref()
            .unwrap()
            .metadata
            .iter()
            .map(|m| (m.title.clone(), m.artist.clone(), m.album.clone()))
            .collect()
    }

    fn surface_hash(&self) -> u64 {
        self.host
            .surface
            .rgba
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
    }
}

/// The audio of one file played alone (interleaved stereo f32).
fn alone(path: &str) -> Vec<f32> {
    let r = play_file(path, &PlayOptions::default()).unwrap();
    r.audio
}

/// The longest run of exactly-zero frames in `audio` (stereo) within `radius` frames of `at`.
fn zero_run_near(audio: &[f32], at: usize, radius: usize) -> usize {
    let (lo, hi) = (at.saturating_sub(radius), (at + radius).min(audio.len() / 2));
    let (mut best, mut cur) = (0, 0);
    for f in lo..hi {
        if audio[f * 2] == 0.0 && audio[f * 2 + 1] == 0.0 {
            cur += 1;
            best = best.max(cur);
        } else {
            cur = 0;
        }
    }
    best
}

#[test]
fn the_app_scans_a_folder_it_is_handed_and_persists_it() {
    if skip() {
        return;
    }
    let mut r = Rig::scanned();
    assert_eq!(r.app.library().albums().len(), 20);
    assert!(r.app.scan_status().is_none());
    assert!(r.app.library().all_tracks().iter().all(|t| !t.src.is_empty()), "every file can be opened");
    assert!(r.app.library().wanted_art().is_empty(), "thumbnails are in memory");
    // The index and the covers were saved.
    assert!(r.host.storage.0.contains_key(rvp_library::INDEX_KEY));
    assert!(
        r.host.storage.0.keys().filter(|k| k.starts_with("library/art/")).count() == 14,
        "{:?}",
        r.host.storage.0.keys().collect::<Vec<_>>()
    );

    // A restart: a new app on the same storage has the library before anything is scanned, and the covers follow.
    let storage = std::mem::take(&mut r.host.storage.0);
    let mut host = UiHost::new();
    host.library = Some(ScriptedLibrary::default());
    host.storage.0 = storage;
    let mut r2 = Rig::with(host);
    r2.run(300);
    assert_eq!(r2.app.library().track_count(), 201);
    assert_eq!(r2.app.library().albums().len(), 20);
    assert!(r2.app.library().wanted_art().is_empty(), "covers loaded from storage");
    assert!(
        r2.app.library().all_tracks().iter().all(|t| t.src.is_empty()),
        "nothing can play until the folder is listed again"
    );
    // Listing the folder again makes the files reachable and reads nothing.
    r2.add_folder();
    assert_eq!(r2.app.library().all_tracks().iter().filter(|t| t.src.is_empty()).count(), 0);
}

#[test]
fn a_lossless_album_plays_gapless_with_the_right_metadata_for_every_track() {
    if skip() {
        return;
    }
    let mut r = Rig::scanned();
    let album = r.album("Aurora Vale", "Daybreak");
    assert_eq!(album.tracks.len(), 10);
    r.host.audio.capture = Some(Vec::new());
    r.act(Action::Lib(LibAction::Play(Scope::Album(album.id), Enqueue::Now)));
    assert_eq!(r.app.playlist().len(), 10);
    // Run until the queue has played out (about five seconds of virtual time) and a little more.
    for _ in 0..20 {
        r.run(500);
        if r.app.model().state == MediaState::Ended {
            break;
        }
    }
    assert_eq!(r.app.model().state, MediaState::Ended);
    // Metadata: one update per track, in album order, with the tags the files carry.
    let want: Vec<(String, String, String)> = album
        .tracks
        .iter()
        .map(|&id| {
            let t = r.app.library().track(id).unwrap();
            (t.display_title().to_string(), t.display_artist().to_string(), t.display_album().to_string())
        })
        .collect();
    assert_eq!(r.metas(), want);
    // The sound: exactly the concatenation of the ten files played alone, no gap and nothing lost at the joins.
    let tracks: Vec<_> =
        album.tracks.iter().map(|&id| r.app.library().track(id).unwrap().src.clone()).collect();
    let mut expect: Vec<f32> = Vec::new();
    for p in &tracks {
        expect.extend(alone(p));
    }
    let got = r.host.audio.capture.take().unwrap();
    // Tracks are 24 kHz and come out at the device's 48 kHz, so each join may differ from playing the files alone by the
    // resampler's edge (a few dozen frames); the whole is the files joined end to end, nothing more and nothing less.
    let lens: Vec<usize> = tracks.iter().map(|p| alone(p).len() / 2).collect();
    let (played, total) = (got.len() / 2, lens.iter().sum::<usize>());
    assert!(played.abs_diff(total) <= 10 * 48, "frames: {played} played, {total} in the files");
    // The first track is untouched by any join and the last one ends the stream: both match what they are alone, sample for sample.
    let first = &expect[..(lens[0] - 600) * 2];
    assert!(got[..first.len()] == *first, "the first track differs from the file");
    let last_len = (lens[lens.len() - 1] - 600) * 2;
    assert!(
        got[got.len() - last_len..] == expect[expect.len() - last_len..],
        "the last track differs from the file"
    );
    // No silence at any join: not even a millisecond of exact zeros.
    let mut at = 0;
    for l in &lens[..lens.len() - 1] {
        at += l;
        let zeros = zero_run_near(&got, at, 2400);
        assert!(zeros < 48, "a gap of {zeros} frames near the join at frame {at}");
    }
    // The model followed along: now playing is the last track.
    assert_eq!(r.app.model().now_track, Some(*album.tracks.last().unwrap()));
}

#[test]
fn a_mixed_format_album_joins_across_containers_without_gaps() {
    if skip() {
        return;
    }
    let mut r = Rig::scanned();
    let album = r.album("Various Artists", "Neon Mixtape Vol. 1");
    assert_eq!(album.tracks.len(), 16);
    r.host.audio.capture = Some(Vec::new());
    r.act(Action::Lib(LibAction::Play(Scope::Album(album.id), Enqueue::Now)));
    for _ in 0..30 {
        r.run(500);
        if r.app.model().state == MediaState::Ended {
            break;
        }
    }
    assert_eq!(r.app.model().state, MediaState::Ended);
    let want: Vec<String> = album
        .tracks
        .iter()
        .map(|&id| r.app.library().track(id).unwrap().display_title().to_string())
        .collect();
    let got_titles: Vec<String> = r.metas().into_iter().map(|m| m.0).collect();
    assert_eq!(got_titles, want, "metadata follows the tracks across mp3, flac, opus, aac and vorbis");
    let tracks: Vec<_> =
        album.tracks.iter().map(|&id| r.app.library().track(id).unwrap().src.clone()).collect();
    let lens: Vec<usize> = tracks.iter().map(|p| alone(p).len() / 2).collect();
    let got = r.host.audio.capture.take().unwrap();
    let total: usize = lens.iter().sum();
    let played = got.len() / 2;
    assert!(played.abs_diff(total) <= 16 * 96, "{played} frames played, {total} in the files");
    // No silence at a join: at most a hundredth of a second... in fact not even a millisecond of exact zeros.
    let mut at = 0;
    for l in &lens[..lens.len() - 1] {
        at += l;
        let zeros = zero_run_near(&got, at, 2400);
        assert!(zeros < 48, "a gap of {zeros} frames near the join at frame {at}");
    }
}

#[test]
fn queue_operations_through_actions() {
    if skip() {
        return;
    }
    let mut r = Rig::scanned();
    let a = r.album("The Midnight Owls", "Hoot & Holler");
    let b = r.album("Delta Hiss", "Feedback Loop");
    r.act(Action::Lib(LibAction::Play(Scope::Album(a.id), Enqueue::Now)));
    r.run(300);
    assert_eq!(r.app.playlist().len(), 8);
    let cur = r.app.playlist().current().unwrap().track;
    assert_eq!(cur, Some(a.tracks[0]));
    // Play next: three tracks of another album go right after the current one, in order.
    r.act(Action::Lib(LibAction::Play(Scope::Track(b.tracks[2]), Enqueue::Next)));
    r.act(Action::Lib(LibAction::Play(Scope::Track(b.tracks[3]), Enqueue::Next)));
    let order: Vec<Option<u32>> = r.app.playlist().items().iter().map(|i| i.track).collect();
    assert_eq!(order[0], Some(a.tracks[0]));
    assert_eq!(order[1], Some(b.tracks[3]), "each 'play next' lands right after the current item");
    assert_eq!(order[2], Some(b.tracks[2]));
    // Add to queue appends.
    r.act(Action::Lib(LibAction::Play(Scope::Album(b.id), Enqueue::Append)));
    assert_eq!(r.app.playlist().len(), 8 + 2 + 12);
    assert_eq!(r.app.playlist().items().last().unwrap().track, Some(*b.tracks.last().unwrap()));
    // The model's queue lists them with tags.
    r.run(50);
    let q = r.app.model().playlist.clone();
    assert_eq!(q.len(), 22);
    assert!(q[1].subtitle.contains("Delta Hiss"), "{:?}", q[1]);
    assert!(q[0].current && !q[1].current);
    // Move one item to play next, remove another, clear.
    let last = r.app.playlist().items().last().unwrap().id;
    r.act(Action::Lib(LibAction::QueueToNext(last)));
    assert_eq!(r.app.playlist().items()[1].id, last);
    r.act(Action::RemoveItem(last));
    assert_eq!(r.app.playlist().len(), 21);
    // Replace with a shuffled album: every track once, shuffle on.
    r.act(Action::Lib(LibAction::Play(Scope::Album(b.id), Enqueue::ShuffleNow)));
    assert!(r.app.playlist().shuffle());
    assert_eq!(r.app.playlist().len(), 12);
    r.act(Action::ClearPlaylist);
    assert_eq!(r.app.playlist().len(), 0);
}

#[test]
fn playlists_are_made_saved_exported_and_imported() {
    if skip() {
        return;
    }
    let mut r = Rig::scanned();
    let album = r.album("Mono Mimi", "Lofi Sketches");
    // Make one from an album through the prompt: the UI collects the name, the app makes the playlist.
    r.act(Action::ShowView(View::Albums));
    r.act(Action::Lib(LibAction::NewPlaylistFrom(Scope::Album(album.id))));
    assert!(r.app.ui().lib_state().typing());
    for c in "Chill".chars() {
        r.key(Key::Char(c));
    }
    r.key(Key::Enter);
    r.run(50);
    let pl = r.app.library().playlists()[0].clone();
    assert_eq!(pl.name, "Chill");
    assert_eq!(pl.track_ids(), album.tracks);
    assert!(r.host.storage.0.contains_key(rvp_library::PLAYLISTS_KEY), "playlists are saved right away");
    // Add one more track from elsewhere.
    let extra = r.album("Delta Hiss", "Feedback Loop").tracks[0];
    r.act(Action::Lib(LibAction::AddToPlaylist(pl.id, Scope::Track(extra))));
    assert_eq!(r.app.library().playlist(pl.id).unwrap().entries.len(), 12);
    // Play it.
    r.act(Action::Lib(LibAction::Play(Scope::Playlist(pl.id), Enqueue::Now)));
    r.run(200);
    assert_eq!(r.app.playlist().len(), 12);
    // Export both formats: the effect carries the file.
    r.effects.clear();
    r.act(Action::Lib(LibAction::ExportPlaylist(pl.id, false)));
    r.act(Action::Lib(LibAction::ExportPlaylist(pl.id, true)));
    let files: Vec<(String, String)> = r
        .effects
        .iter()
        .filter_map(|e| match e {
            Effect::Download { name, data, .. } => {
                Some((name.clone(), String::from_utf8(data.clone()).unwrap()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].0, "Chill.m3u8");
    assert!(files[0].1.starts_with("#EXTM3U\n") && files[0].1.contains("#EXTINF:"));
    assert_eq!(files[1].0, "Chill.pls");
    assert!(files[1].1.starts_with("[playlist]"));
    // Import them again by opening the files (what a drop or the picker does): equal lists come out.
    let dir = fixtures_dir().join("m10-playlists");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text) in &files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let items: Vec<(String, String)> =
        files.iter().map(|(n, _)| (dir.join(n).to_string_lossy().into_owned(), n.clone())).collect();
    r.app.open_items(&mut r.host, &items, true);
    r.run(300);
    let lists = r.app.library().playlists();
    assert_eq!(lists.len(), 3, "{:?}", lists.iter().map(|p| &p.name).collect::<Vec<_>>());
    assert_eq!(lists[1].entries, lists[0].entries);
    assert_eq!(lists[2].entries, lists[0].entries);
    // The app opened the last import for the user to look at.
    assert_eq!(r.app.ui().lib_state().detail(), Some(Detail::Playlist(lists[2].id)));
    // Delete one.
    r.act(Action::Lib(LibAction::DeletePlaylist(lists[2].id)));
    assert_eq!(r.app.library().playlists().len(), 2);
}

#[test]
fn a_playlist_with_a_missing_file_keeps_it_flagged() {
    if skip() {
        return;
    }
    let mut r = Rig::scanned();
    let dir = fixtures_dir().join("m10-playlists");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mixed.m3u");
    std::fs::write(&path, "\u{feff}#EXTM3U\r\n#EXTINF:1,Gone\r\nNowhere/Gone/01 Gone.mp3\r\nAurora Vale/Polar Nights/01 Glass Meadow.mp3\r\nAurora Vale/Polar Nights/02 Static Orbit.mp3\r\n").unwrap();
    r.app.open_items(&mut r.host, &[(path.to_string_lossy().into_owned(), "mixed.m3u".into())], true);
    r.run(300);
    let p = r.app.library().playlists().last().unwrap();
    assert_eq!(p.entries.len(), 3);
    assert_eq!(p.entries[0].track, None);
    assert!(p.missing() >= 1);
    // Playing it skips what is missing and plays what is there.
    let id = p.id;
    let have = p.track_ids().len();
    r.act(Action::Lib(LibAction::Play(Scope::Playlist(id), Enqueue::Now)));
    assert_eq!(r.app.playlist().len(), have);
}

#[test]
fn audio_opened_from_outside_goes_to_the_library_face_and_video_to_the_player() {
    if skip() {
        return;
    }
    let mut r = Rig::new();
    r.run(100);
    assert_eq!(r.app.ui().mode(), Mode::Player);
    let audio =
        music().join("Aurora Vale/Daybreak").read_dir().unwrap().map(|e| e.unwrap().path()).min().unwrap();
    r.app.open(&mut r.host, FileSource::open(&audio.to_string_lossy()).unwrap());
    r.run(600);
    assert_eq!(r.app.ui().mode(), Mode::Library, "a song opens the Library face");
    assert_eq!(r.app.ui().lib_state().view(), View::NowPlaying);
    // Its tags and cover are what the screen shows.
    let m = r.app.model();
    assert_eq!(m.artist, "Aurora Vale");
    assert_eq!(m.album, "Daybreak");
    assert!(!m.title.is_empty() && !m.title.ends_with(".flac"));
    // A video switches to the Player face.
    let video = fixtures_dir().join("av1_opus.webm");
    r.app.open(&mut r.host, FileSource::open(&video.to_string_lossy()).unwrap());
    r.run(800);
    assert_eq!(r.app.ui().mode(), Mode::Player);
    // And the switch is one key: B goes to the library, B again back, with playback undisturbed.
    r.key(Key::Char('b'));
    assert_eq!(r.app.ui().mode(), Mode::Library);
    assert_eq!(r.app.model().state, MediaState::Playing);
    r.key(Key::Char('b'));
    assert_eq!(r.app.ui().mode(), Mode::Player);
}

#[test]
fn the_visualizer_view_moves_with_the_music_and_stops_when_paused() {
    if skip() {
        return;
    }
    let mut r = Rig::scanned_with_motion();
    let album = r.album("Delta Hiss", "Feedback Loop");
    r.act(Action::Lib(LibAction::Play(Scope::Album(album.id), Enqueue::Now)));
    r.act(Action::ShowView(View::Visualizer));
    r.run(600);
    assert_eq!(r.app.ui().mode(), Mode::Library);
    assert!(r.app.viz().frames() > 5, "the visualizer draws while playing");
    let mut hashes = Vec::new();
    for _ in 0..8 {
        r.run(100);
        hashes.push(r.surface_hash());
    }
    hashes.sort();
    hashes.dedup();
    assert!(hashes.len() >= 6, "the picture moves: {} different frames in 8", hashes.len());
    // Paused: the picture settles and stays exactly the same.
    r.act(Action::PlayPause);
    r.run(2500); // the controls settle and the toasts are gone
    let still = r.surface_hash();
    let snap = r.host.surface.rgba.clone();
    for _ in 0..6 {
        r.run(150);
        if r.surface_hash() != still {
            let (w, mut x0, mut y0, mut x1, mut y1, mut n) =
                (1280usize, usize::MAX, usize::MAX, 0usize, 0usize, 0usize);
            for (i, (a, b)) in snap.chunks_exact(4).zip(r.host.surface.rgba.chunks_exact(4)).enumerate() {
                if a != b {
                    let (x, y) = (i % w, i / w);
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                    n += 1;
                }
            }
            eprintln!(
                "DIFF {n} pixels in ({x0},{y0})-({x1},{y1}); state {:?} viz frames {}",
                r.app.model().state,
                r.app.viz().frames()
            );
        }
        assert_eq!(r.surface_hash(), still, "the visualizer moved while paused");
    }
    // Playing again moves it.
    r.act(Action::PlayPause);
    r.run(400);
    assert_ne!(r.surface_hash(), still);
    // All five effects draw; stepping through them changes the picture.
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..5 {
        r.act(Action::Lib(LibAction::VizStep(1)));
        r.run(300);
        seen.insert(r.app.viz().effect);
    }
    assert_eq!(seen.len(), 5);
}

#[test]
fn the_visualizer_is_off_by_default_with_reduced_motion_and_the_view_says_so() {
    if skip() {
        return;
    }
    let mut r = Rig::scanned();
    // The rig runs with reduced motion on: the animation starts off.
    assert!(!r.app.ui().lib_state().viz_on());
    let album = r.album("Delta Hiss", "Feedback Loop");
    r.act(Action::Lib(LibAction::Play(Scope::Album(album.id), Enqueue::Now)));
    r.act(Action::ShowView(View::Visualizer));
    r.run(500);
    assert_eq!(r.app.viz().frames(), 0, "no frames are made while it is off");
    r.run(500);
    assert_eq!(r.app.viz().frames(), 0);
    // Turning it on starts a calm version.
    r.key(Key::Enter);
    r.run(600);
    assert!(r.app.ui().lib_state().viz_on());
    assert!(r.app.viz().frames() > 3);
}

#[test]
fn keys_and_the_context_of_the_library_work_through_the_app() {
    if skip() {
        return;
    }
    let mut r = Rig::scanned();
    r.act(Action::SetMode(Mode::Library));
    r.key(Key::Char('3'));
    assert_eq!(r.app.ui().lib_state().view(), View::Tracks);
    r.run(50);
    r.key(Key::Down);
    r.key(Key::Down);
    r.key(Key::Enter); // plays the list from the second row
    r.run(300);
    assert_eq!(r.app.model().state, MediaState::Playing);
    assert_eq!(r.app.playlist().len(), 201, "the whole list is the queue");
    assert_eq!(
        r.app.playlist().current().unwrap().track,
        r.app.library().sorted_tracks(rvp_library::TrackSort::Title, true).get(1).copied()
    );
    // The now-playing view names it; the bar's title is the track's.
    r.key(Key::Char('6'));
    assert_eq!(
        r.app.model().title,
        r.app.library().track(r.app.model().now_track.unwrap()).unwrap().display_title()
    );
    // Search, then Escape back.
    r.key(Key::Char('/'));
    for c in "tidal".chars() {
        r.key(Key::Char(c));
    }
    assert_eq!(r.app.ui().lib_state().view(), View::Search);
    r.run(50);
    r.key(Key::Escape);
    assert_eq!(r.app.ui().lib_state().view(), View::NowPlaying);
}
