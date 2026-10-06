//! The player application: owns the playback [`Session`] and the [`Ui`], turns input into player commands and
//! player state into pixels. A host (browser, Rusty Bucket, headless) only has to implement
//! [`rvp_host::Host`] with a [`FrameSink`] as its video sink, forward input, and call [`App::tick`].
//!
//! Everything the user can do is an [`rvp_ui::Action`]; [`App::apply`] is the single place where actions
//! become player calls.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod library;
mod restore;
mod snapshot;

pub use restore::{POSITION_KEY, QUEUE_KEY, SavedItem, SavedQueue};

pub use snapshot::Snapshot;

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use library::{LibState, RESUME_MIN_AUDIO_US, is_playlist_name};
use rvp_core::settings::SETTINGS_KEY;
use rvp_core::{AudioSettings, CodecFactory, Error, LevelMode, LoudnessTags, Timestamp};
use rvp_host::{
    FrameSink, Host, InputEvent, NowPlayingMeta, OpenRequest, PlayState, Playback, Rect, Storage,
    TransportCommand,
};
use rvp_player::{Playlist, Repeat, Session, SessionEvent, SessionState};
use rvp_ui::{Action, Cursor, FrameBuffer, MediaState, Mode, SPEEDS, TrackItem, Ui, UiConfig, UiModel};

/// A saved resume position is only used when the file is longer than this past it, microseconds.
const RESUME_MIN_REMAINING_US: Timestamp = 10_000_000;
/// And only when it is at least this far in.
const RESUME_MIN_POSITION_US: Timestamp = 5_000_000;
/// How often the position is written to storage while playing, microseconds.
const RESUME_SAVE_EVERY_US: Timestamp = 5_000_000;
/// A previous-item request this far into an item restarts it instead of going back.
const PREV_RESTART_US: Timestamp = 3_000_000;

/// Granularity at which a moving position triggers a redraw (20 Hz).
const LIVE_REDRAW_US: Timestamp = 50_000;
/// Granularity of the position in the library face's bar (4 Hz).
const LIB_LIVE_REDRAW_US: Timestamp = 250_000;

/// Something the host has to do on the app's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Show the file picker (in a browser this must happen inside the user's input event).
    PickFile,
    /// Show the file picker to add files to the playlist.
    AddFiles,
    /// Let the user pick a folder to add to the library; the host walks it and hands back a `rvp_host::Listing`.
    AddFolder,
    /// Walk a library folder (by root id) again and hand back a listing.
    Rescan(String),
    /// A folder left the library: the host can forget its handle.
    Forget(String),
    /// Show the file picker for playlist files (M3U, M3U8, PLS).
    ImportPlaylist,
    /// Give the user a file (an exported playlist).
    Download {
        /// Suggested file name.
        name: String,
        /// Media type.
        mime: String,
        /// The bytes.
        data: Vec<u8>,
    },
}

/// `n word` with the right number.
pub(crate) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The application.
pub struct App {
    codecs: Rc<dyn CodecFactory>,
    session: Option<Session>,
    ui: Ui,
    base: FrameBuffer,
    fb: FrameBuffer,
    model: UiModel,
    title: String,
    volume: f32,
    muted: bool,
    rate: f64,
    fullscreen: bool,
    effects: Vec<Effect>,
    drawn_video: u64,
    /// Size of the picture last drawn (it can change mid-stream at a key frame).
    drawn_size: (u32, u32),
    base_dirty: bool,
    force_draw: bool,
    last_drawn: Option<UiModel>,
    last_has_media: bool,
    last_lib_mode: bool,
    last_base_key: Option<(bool, rvp_ui::View, bool, bool)>,
    warnings_seen: usize,
    loop_a: Option<Timestamp>,
    loop_b: Option<Timestamp>,
    playlist: Playlist,
    /// The playlist item queued to follow the current one gaplessly.
    queued: Option<u32>,
    resume_key: Option<String>,
    resume_checked: bool,
    last_resume_save: Timestamp,
    np: NpState,
    now: Timestamp,
    frames_drawn: u64,
    perf: Perf,
    /// The library face: index, scanner, visualizer, what is playing.
    lib: LibState,
    /// The library revision the frame on screen was drawn for.
    drawn_lib_rev: u64,
    /// Restoring the queue after a restart, and keeping it saved.
    restore: restore::RestoreState,
    /// Crossfade and automatic level, as the user set them (kept in storage under `settings/audio`).
    settings: AudioSettings,
    settings_loaded: bool,
    /// The library revision the playing item's loudness hint was made for.
    hint_rev: u64,
}

/// What was last told to the host's now-playing sink.
#[derive(Default)]
struct NpState {
    meta: Option<NowPlayingMeta>,
    playback: Option<Playback>,
    /// Host time at which `playback` was sent.
    sent_at: Timestamp,
}

/// Where tick time goes, in host microseconds (cumulative).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Perf {
    /// Ticks run.
    pub ticks: u64,
    /// Time spent in `Session::tick` (demux, decode, audio).
    pub session_us: i64,
    /// Time spent composing and presenting frames.
    pub render_us: i64,
    /// Of which: drawing the picture layer.
    pub base_us: i64,
    /// Of which: drawing the chrome.
    pub overlay_us: i64,
    /// Of which: handing the pixels to the surface.
    pub present_us: i64,
    /// Slowest single tick.
    pub max_tick_us: i64,
}

impl App {
    /// A new app showing the empty screen. `codecs` supplies the decoders linked into the host.
    pub fn new(codecs: Rc<dyn CodecFactory>, config: UiConfig) -> Self {
        Self {
            session: None,
            ui: {
                let mut ui = Ui::new(config);
                // The visualizer starts calm: off with reduced motion, on otherwise.
                ui.set_viz_on(!config.reduce_motion);
                ui
            },
            base: FrameBuffer::new(1, 1),
            fb: FrameBuffer::new(1, 1),
            model: UiModel { volume: 1.0, rate: 1.0, ..UiModel::default() },
            title: String::new(),
            volume: 1.0,
            muted: false,
            rate: 1.0,
            fullscreen: false,
            effects: Vec::new(),
            drawn_video: u64::MAX,
            drawn_size: (0, 0),
            base_dirty: true,
            force_draw: true,
            last_drawn: None,
            last_has_media: false,
            last_lib_mode: false,
            last_base_key: None,
            warnings_seen: 0,
            loop_a: None,
            loop_b: None,
            playlist: Playlist::new(),
            queued: None,
            resume_key: None,
            resume_checked: true,
            last_resume_save: 0,
            np: NpState::default(),
            now: 0,
            frames_drawn: 0,
            perf: Perf::default(),
            lib: {
                let mut lib = LibState::new();
                lib.scanner.set_codecs(codecs.clone());
                lib
            },
            drawn_lib_rev: u64::MAX,
            restore: restore::RestoreState::default(),
            settings: AudioSettings::default(),
            settings_loaded: false,
            hint_rev: u64::MAX,
            codecs,
        }
    }

    /// The UI (read-only).
    pub fn ui(&self) -> &Ui {
        &self.ui
    }

    /// The UI, to change its settings (reduced motion).
    pub fn ui_mut(&mut self) -> &mut Ui {
        &mut self.ui
    }

    /// The current session, if a file is open.
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// The last UI model the app built.
    pub fn model(&self) -> &UiModel {
        &self.model
    }

    /// The last composed frame (what was presented).
    pub fn framebuffer(&self) -> &FrameBuffer {
        &self.fb
    }

    /// Number of frames presented to the surface so far.
    pub fn frames_drawn(&self) -> u64 {
        self.frames_drawn
    }

    /// Requests for the host (file picker). Returns and clears them.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        core::mem::take(&mut self.effects)
    }

    /// The mouse cursor the host should show.
    pub fn cursor(&self) -> Cursor {
        self.ui.cursor(&self.model)
    }

    /// Fullscreen state changed outside the app (the browser's own Esc handling).
    pub fn set_fullscreen_state(&mut self, on: bool) {
        self.fullscreen = on;
    }

    /// Draw everything again on the next tick (a window that was uncovered, a new surface).
    pub fn invalidate(&mut self) {
        self.base_dirty = true;
        self.force_draw = true;
    }

    /// The playlist.
    pub fn playlist(&self) -> &Playlist {
        &self.playlist
    }

    /// Crossfade and automatic level as set now.
    pub fn audio_settings(&self) -> AudioSettings {
        self.settings
    }

    /// What the library knows about the loudness of playlist item `id` (its track's figure and its album's).
    fn loudness_hint_for(&self, id: u32) -> Option<LoudnessTags> {
        self.playlist.get(id).and_then(|i| i.track).and_then(|t| self.lib.lib.loudness_hint(t))
    }

    /// Change the audio settings with `change`, keep them (host storage) and give them to the player. Settings that end up
    /// unchanged do nothing.
    fn update_settings<H>(&mut self, host: &mut H, change: impl FnOnce(&mut AudioSettings))
    where
        H: Host<Video = FrameSink>,
    {
        let mut s = self.settings;
        change(&mut s);
        let s = s.clamped();
        if s == self.settings {
            return;
        }
        self.settings = s;
        rvp_core::task::block_on(host.storage().store(SETTINGS_KEY, s.to_text().as_bytes()));
        if let Some(sess) = &mut self.session {
            sess.set_audio_settings(s);
        }
    }

    /// Read the saved audio settings (once, at the first tick), and keep the session and the library's measurement job in step
    /// with them.
    fn settings_tick<H>(&mut self, host: &mut H)
    where
        H: Host<Video = FrameSink>,
    {
        if !self.settings_loaded {
            self.settings_loaded = true;
            if let Some(bytes) = rvp_core::task::block_on(host.storage().load(SETTINGS_KEY)) {
                if let Some(s) = core::str::from_utf8(&bytes).ok().and_then(AudioSettings::from_text) {
                    self.settings = s;
                }
            }
        }
        let rev = self.lib.lib.revision();
        if let Some(s) = &mut self.session {
            if s.audio_settings() != self.settings {
                s.set_audio_settings(self.settings);
            }
        }
        if rev != self.hint_rev {
            // The library learnt something (a track was measured): the item playing may know its loudness now.
            self.hint_rev = rev;
            if let Some(tag) = self.session.as_ref().map(|s| s.tag()) {
                let hint = self.loudness_hint_for(tag);
                if let Some(s) = &mut self.session {
                    s.set_loudness_hint(hint);
                }
            }
        }
    }

    /// A message for a change made by a shortcut or a menu (the panel shows its own state).
    fn settings_toast(&mut self, text: &str, now: Timestamp) {
        if !self.ui.audio_settings_open() {
            self.ui.show_toast(text, now);
        }
    }

    /// Open `source` and start playing it, replacing the playlist with that one item. (A subtitle file joins
    /// the video that is playing instead.)
    pub fn open<H>(&mut self, host: &mut H, source: H::Source)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        use rvp_host::Source;
        let now = host.clock().now_us();
        if is_subtitle_name(source.name()) && self.session.is_some() {
            let name = source.name().to_string();
            if let Some(s) = &mut self.session {
                s.add_subtitle_source(source, &name);
            }
            self.ui.show_toast(&format!("Subtitles: {name}"), now);
            self.refresh_model(now);
            return;
        }
        if is_playlist_name(source.name()) {
            // A playlist file handed over as a source: it goes to the library's playlists.
            let name = source.name().to_string();
            self.import_playlist_source(source, &name);
            return;
        }
        self.save_resume(host, true);
        self.playlist.clear();
        let id = self.playlist.add(source.name(), "");
        self.playlist.set_current(id);
        self.lib.auto_mode = true;
        self.start(host, source, id, true);
    }

    /// Open files by host id (`(id, display name)`): they become the playlist and the first one plays, or with
    /// `append` they join the end of the playlist (and play if nothing is playing). Subtitle files attach to the
    /// video that is playing.
    pub fn open_items<H>(&mut self, host: &mut H, items: &[(String, String)], append: bool)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        let now = host.clock().now_us();
        let (lists, rest): (Vec<_>, Vec<_>) = items.iter().partition(|(_, name)| is_playlist_name(name));
        let (subs, media): (Vec<_>, Vec<_>) = rest.into_iter().partition(|(_, name)| is_subtitle_name(name));
        for (id, name) in lists {
            self.import_playlist_file(host, id, name, now);
        }
        let idle = !self.model.has_media() || self.model.state == MediaState::Failed;
        if !append || idle {
            if !media.is_empty() {
                self.save_resume(host, true);
            }
            if !append && !media.is_empty() {
                self.playlist.clear();
            }
        }
        let mut first_new = None;
        for (id, name) in &media {
            let item = self.playlist.add(name, id);
            first_new.get_or_insert(item);
        }
        if let Some(first) = first_new {
            if !append || idle {
                self.lib.auto_mode = true;
                self.play_item(host, first);
            } else {
                self.ui.show_toast(&format!("Added {} to the playlist", media.len()), now);
                self.queued = None; // the end of the list moved: ask again
            }
        }
        for (id, name) in subs {
            match rvp_core::task::block_on(host.open(OpenRequest::Id(id.clone()))) {
                Ok(src) => {
                    if let Some(s) = &mut self.session {
                        s.add_subtitle_source(src, name);
                        self.ui.show_toast(&format!("Subtitles: {name}"), now);
                    } else {
                        self.ui.show_toast("Open a video first, then add its subtitles.", now);
                    }
                }
                Err(e) => self.ui.show_toast(&format!("Couldn't open that: {e}"), now),
            }
        }
        self.refresh_model(now);
    }

    /// Start playing playlist item `id`.
    fn play_item<H>(&mut self, host: &mut H, id: u32)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        let now = host.clock().now_us();
        let Some(item) = self.playlist.get(id).cloned() else { return };
        if item.source.is_empty() {
            self.ui.show_toast("That file can't be opened again; pick it once more.", now);
            return;
        }
        match rvp_core::task::block_on(host.open(OpenRequest::Id(item.source.clone()))) {
            Ok(src) => {
                self.save_resume(host, true);
                self.playlist.set_current(id);
                self.start(host, src, id, true);
            }
            Err(e) => {
                self.ui.show_toast(&format!("Couldn't open {}: {e}", item.name), now);
            }
        }
    }

    /// Replace the session with one playing `source` (playlist item `id`).
    fn start<H>(&mut self, host: &mut H, source: H::Source, id: u32, resume: bool)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        use rvp_host::Source;
        let now = host.clock().now_us();
        if !self.restore.starting {
            self.restore.pending = None; // the user opened something: the saved queue is not wanted any more
            self.restore.forced_pos = None;
        }
        self.title = source.name().to_string();
        self.resume_key = Some(format!("resume:{}", self.title));
        self.resume_checked = !resume;
        self.last_resume_save = now;
        self.loop_a = None;
        self.loop_b = None;
        self.queued = None;
        let mut s = Session::new(source, self.codecs.clone());
        s.set_tag(id);
        s.set_audio_settings(self.settings);
        s.set_loudness_hint(self.loudness_hint_for(id));
        s.set_volume(self.volume);
        s.set_muted(self.muted);
        s.set_rate(self.rate, now);
        s.play();
        self.session = Some(s);
        host.video().clear();
        self.base_dirty = true;
        self.force_draw = true;
        self.warnings_seen = 0;
        self.refresh_model(now);
    }

    /// Write the playback position of the current file to storage (or forget it near the end).
    fn save_resume<H: Host<Video = FrameSink>>(&mut self, host: &mut H, force: bool) {
        let (Some(key), Some(s)) = (self.resume_key.clone(), &self.session) else { return };
        if !self.resume_checked {
            return; // nothing has played yet: do not overwrite the saved position with zero
        }
        let now = host.clock().now_us();
        if !force && now - self.last_resume_save < RESUME_SAVE_EVERY_US {
            return;
        }
        self.last_resume_save = now;
        let (Some(dur), pos) = (s.duration_us(), s.position_us(now)) else { return };
        if !s.container_has_video() && dur < RESUME_MIN_AUDIO_US {
            return; // a song starts from the top every time
        }
        let value: Vec<u8> = if s.state() == SessionState::Ended || pos + RESUME_MIN_REMAINING_US > dur {
            Vec::new() // finished (or nearly): next time starts from the beginning
        } else {
            (pos / 1000).to_le_bytes().to_vec()
        };
        rvp_core::task::block_on(host.storage().store(&key, &value));
    }

    /// Persist what must survive a page reload (a host calls this before unloading).
    pub fn save_state<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        self.save_resume(host, true);
        let now = host.clock().now_us();
        self.save_queue(host, now, true);
    }

    /// Once the file is open, jump to the position saved last time.
    fn check_resume<H: Host<Video = FrameSink>>(&mut self, host: &mut H, now: Timestamp) {
        if self.resume_checked {
            return;
        }
        let Some(s) = &mut self.session else { return };
        let Some(dur) = s.duration_us() else {
            if s.state() != SessionState::Opening {
                self.resume_checked = true;
            }
            return;
        };
        self.resume_checked = true;
        if let Some(pos) = self.restore.forced_pos.take() {
            // The queue was restored: go back to where the last run stopped, whatever the length.
            if pos >= RESUME_MIN_POSITION_US && pos + RESUME_MIN_REMAINING_US <= dur {
                s.seek(pos);
                self.ui.show_toast(&format!("Restored at {}", rvp_ui::format_time(pos)), now);
            }
            return;
        }
        if !s.container_has_video() && dur < RESUME_MIN_AUDIO_US {
            return;
        }
        let Some(key) = self.resume_key.clone() else { return };
        let Some(bytes) = rvp_core::task::block_on(host.storage().load(&key)) else { return };
        let Ok(raw) = <[u8; 8]>::try_from(bytes.as_slice()) else { return };
        let pos = i64::from_le_bytes(raw) * 1000;
        if pos >= RESUME_MIN_POSITION_US && pos + RESUME_MIN_REMAINING_US <= dur {
            s.seek(pos);
            self.ui.show_toast(&format!("Resumed at {}", rvp_ui::format_time(pos)), now);
        }
    }

    /// Mirror the playback state to the host's now-playing sink and carry out the commands it passes back.
    fn sync_now_playing<H>(&mut self, host: &mut H, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        if host.now_playing().is_none() {
            return;
        }
        // Metadata: the file's tags, or its name.
        let meta = self.session.as_ref().map(|s| {
            let m = s.metadata();
            let stem = match self.title.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 => stem,
                _ => self.title.as_str(),
            };
            NowPlayingMeta {
                title: m.title.unwrap_or_else(|| stem.to_string()),
                artist: m.artist.unwrap_or_default(),
                album: m.album.unwrap_or_default(),
                art: m.art,
                duration_us: s.duration_us(),
                has_video: s.container_has_video(),
            }
        });
        if meta != self.np.meta {
            self.np.meta = meta.clone();
            self.np.playback = None;
            if let (Some(np), Some(m)) = (host.now_playing(), &meta) {
                np.set_metadata(m);
            }
        }
        let st = match self.model.state {
            MediaState::Playing | MediaState::Buffering => PlayState::Playing,
            MediaState::Paused | MediaState::Ended => PlayState::Paused,
            _ => PlayState::Stopped,
        };
        let idx = self.playlist.current_id().and_then(|id| self.playlist.index_of(id));
        let can_next =
            idx.is_some_and(|i| i + 1 < self.playlist.len() || self.playlist.repeat() == Repeat::All);
        let playback = Playback {
            state: st,
            position_us: self.model.position_us,
            rate: self.rate as f32,
            can_next,
            can_prev: self.model.has_media(),
            can_seek: self.model.duration_us.is_some(),
        };
        let send = match &self.np.playback {
            None => true,
            Some(p) => {
                let predicted = if p.state == PlayState::Playing {
                    p.position_us + ((now - self.np.sent_at) as f64 * p.rate as f64) as i64
                } else {
                    p.position_us
                };
                p.state != playback.state
                    || p.rate != playback.rate
                    || p.can_next != playback.can_next
                    || p.can_prev != playback.can_prev
                    || p.can_seek != playback.can_seek
                    || (predicted - playback.position_us).abs() > 500_000
            }
        };
        if send {
            self.np.playback = Some(playback);
            self.np.sent_at = now;
            if let Some(np) = host.now_playing() {
                np.set_playback(&playback);
            }
        }
        // Commands from outside.
        let mut cmds = Vec::new();
        if let Some(np) = host.now_playing() {
            while let Some(c) = np.poll_command() {
                cmds.push(c);
            }
        }
        for c in cmds {
            self.run_command(host, c, now);
        }
    }

    fn run_command<H>(&mut self, host: &mut H, c: TransportCommand, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        let active = self.model.state.is_active();
        match c {
            TransportCommand::Play if !active => self.apply(host, Action::PlayPause, now),
            TransportCommand::Pause if active => self.apply(host, Action::PlayPause, now),
            TransportCommand::Toggle => self.apply(host, Action::PlayPause, now),
            TransportCommand::Stop => {
                if active {
                    self.apply(host, Action::PlayPause, now);
                }
                if let Some(s) = &mut self.session {
                    s.seek(0);
                }
            }
            TransportCommand::Next => self.apply(host, Action::Next, now),
            TransportCommand::Prev => self.apply(host, Action::Prev, now),
            TransportCommand::SeekTo(us) => {
                if let Some(s) = &mut self.session {
                    let t = Self::clamp_pos(us, s.duration_us());
                    s.seek(t);
                }
            }
            TransportCommand::SeekBy(us) => {
                if let Some(s) = &mut self.session {
                    let t = Self::clamp_pos(s.position_us(now) + us, s.duration_us());
                    s.seek(t);
                }
            }
            TransportCommand::SetRate(r) => self.set_speed(r as f64, now),
            TransportCommand::SetVolume(v) => self.apply(host, Action::SetVolume(v), now),
            _ => {}
        }
    }

    /// Gapless chaining and playlist upkeep after each session tick.
    fn run_playlist<H>(&mut self, host: &mut H, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        // Events from the session.
        let mut started = None;
        let mut failed = Vec::new();
        if let Some(s) = &mut self.session {
            while let Some(e) = s.poll_event() {
                match e {
                    SessionEvent::ItemStarted { tag } => started = Some(tag),
                    SessionEvent::ItemFailed { tag, error } => failed.push((tag, error)),
                    SessionEvent::Subtitle { .. } => {}
                }
            }
        }
        for (tag, error) in failed {
            let name = self.playlist.get(tag).map(|i| i.name.clone()).unwrap_or_default();
            self.ui.show_toast(&format!("Skipped {name}: {}", friendly_error(&error)), now);
            self.playlist.remove(tag);
            self.queued = None;
        }
        if let Some(tag) = started {
            if self.playlist.set_current(tag) {
                self.title = self.playlist.get(tag).map(|i| i.name.clone()).unwrap_or_default();
                self.loop_a = None;
                self.loop_b = None;
                self.warnings_seen = 0;
                self.queued = None;
                self.resume_key = Some(format!("resume:{}", self.title));
                self.resume_checked = true;
                self.last_resume_save = now;
                self.base_dirty = true;
                self.ui.show_toast(&format!("Now playing {}", self.title), now);
            }
        }
        // Queue the next item when the current one is about to run out.
        let want = self.session.as_ref().is_some_and(|s| s.wants_next(now));
        if want && self.queued.is_none() {
            if let Some(next) = self.playlist.peek_next() {
                let src = self.playlist.get(next).map(|i| i.source.clone()).filter(|s| !s.is_empty());
                if let Some(src) = src {
                    match rvp_core::task::block_on(host.open(OpenRequest::Id(src))) {
                        Ok(source) => {
                            let hint = self.loudness_hint_for(next);
                            if let Some(s) = &mut self.session {
                                s.queue_next_with(source, next, hint);
                            }
                            self.queued = Some(next);
                        }
                        Err(e) => {
                            let name = self.playlist.get(next).map(|i| i.name.clone()).unwrap_or_default();
                            self.ui.show_toast(&format!("Skipped {name}: {e}"), now);
                            self.playlist.remove(next);
                        }
                    }
                }
            }
        }
        // The session ended with nothing queued (a failed or late prefetch): move on by hand.
        let ended = self.session.as_ref().is_some_and(|s| s.state() == SessionState::Ended);
        if ended && self.queued.is_none() {
            if let Some(next) = self.playlist.advance() {
                self.play_item(host, next);
            }
        }
    }

    /// Process pending input right away (a browser host calls this inside the DOM event so the file picker
    /// and fullscreen requests happen within the user gesture).
    pub fn pump<H>(&mut self, host: &mut H)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        let now = host.clock().now_us();
        self.now = now;
        while let Some(ev) = host.input().poll() {
            self.on_event(host, ev, now);
        }
    }

    fn on_event<H>(&mut self, host: &mut H, ev: InputEvent, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        if let InputEvent::Drop { id } = &ev {
            // Hosts resolve their own ids without waiting on anything external.
            match rvp_core::task::block_on(host.open(OpenRequest::Id(id.clone()))) {
                Ok(src) => self.open(host, src),
                Err(e) => self.ui.show_toast(&format!("Couldn't open that: {e}"), now),
            }
        }
        let actions = if self.ui.mode() == Mode::Library {
            let video = host.video();
            let frame = (video.width > 0).then_some((video.rgba.as_slice(), video.width, video.height));
            let ctx = Self::lib_ctx(&self.lib, frame);
            self.ui.handle_lib(&ev, now, &self.model, &ctx)
        } else {
            self.ui.handle(&ev, now, &self.model)
        };
        for a in actions {
            self.apply(host, a, now);
        }
        self.drain_ui_commands(host, now);
        self.refresh_model(now);
    }

    /// Run one step: input, playback, UI timers, and present a new picture if anything changed.
    /// Returns true if a frame was presented.
    pub fn tick<H>(&mut self, host: &mut H) -> bool
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        let t0 = host.clock().now_us();
        self.pump(host);
        self.settings_tick(host);
        let mut session = self.session.take();
        if let Some(s) = &mut session {
            s.tick(host);
        }
        self.session = session;
        let tn = host.clock().now_us();
        self.check_resume(host, tn);
        self.run_playlist(host, tn);
        self.lib_tick(host, tn);
        self.restore_tick(host, tn);
        self.save_resume(host, false);
        self.save_queue(host, tn, false);
        let t1 = host.clock().now_us();
        self.now = t1;
        self.refresh_model(t1);
        self.sync_now_playing(host, t1);
        let drawn = self.render(host, t1);
        let t2 = host.clock().now_us();
        self.perf.ticks += 1;
        self.perf.session_us += t1 - t0;
        self.perf.render_us += t2 - t1;
        self.perf.max_tick_us = self.perf.max_tick_us.max(t2 - t0);
        drawn
    }

    /// Cumulative timing of the ticks so far.
    pub fn perf(&self) -> Perf {
        self.perf
    }

    // ---- actions ---------------------------------------------------------------------------------------

    /// Apply one user action to the player.
    pub fn apply<H>(&mut self, host: &mut H, action: Action, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        match action {
            Action::OpenFile => self.effects.push(Effect::PickFile),
            Action::PlayPause => {
                if let Some(s) = &mut self.session {
                    match s.state() {
                        SessionState::Playing | SessionState::Buffering => s.pause(now),
                        SessionState::Paused | SessionState::Ended => s.play(),
                        SessionState::Opening => {
                            // Not open yet: toggle the intent to play once it is.
                            if self.model.state.is_active() { s.pause(now) } else { s.play() }
                        }
                        SessionState::Failed => {}
                    }
                }
            }
            Action::SeekBy(ms) => {
                if let Some(s) = &mut self.session {
                    let target = Self::clamp_pos(s.position_us(now) + ms as i64 * 1000, s.duration_us());
                    s.seek(target);
                    let secs = ms.abs() / 1000;
                    self.ui.show_toast(&format!("{} {secs} s", if ms < 0 { "\u{2212}" } else { "+" }), now);
                }
            }
            Action::SeekFraction(f) => {
                if let Some(s) = &mut self.session {
                    if let Some(d) = s.duration_us() {
                        s.seek(Self::clamp_pos((f.clamp(0.0, 1.0) as f64 * d as f64) as i64, Some(d)));
                    }
                }
            }
            Action::SeekStart => {
                if let Some(s) = &mut self.session {
                    s.seek(0);
                }
            }
            Action::SeekEnd => {
                if let Some(s) = &mut self.session {
                    if let Some(d) = s.duration_us() {
                        s.seek((d - 250_000).max(0));
                    }
                }
            }
            Action::VolumeBy(p) => {
                self.volume = (self.volume + p as f32 / 100.0).clamp(0.0, 1.0);
                if p > 0 {
                    self.muted = false;
                }
                self.push_audio_state();
                self.ui.show_toast(&format!("Volume {}%", (self.volume * 100.0 + 0.5) as i32), now);
            }
            Action::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.muted = self.volume <= 0.0 && self.muted;
                if v > 0.0 {
                    self.muted = false;
                }
                self.push_audio_state();
            }
            Action::ToggleMute => {
                self.muted = !self.muted;
                self.push_audio_state();
                self.ui.show_toast(if self.muted { "Muted" } else { "Sound on" }, now);
            }
            Action::ToggleFullscreen => {
                self.fullscreen = !self.fullscreen;
                host.surface().set_fullscreen(self.fullscreen);
            }
            Action::SpeedStep(d) => self.set_speed(Ui::next_speed(self.rate as f32, d) as f64, now),
            Action::SetSpeed(r) => self.set_speed(r as f64, now),
            Action::ResetSpeed => self.set_speed(1.0, now),
            Action::Next => match self.playlist.next() {
                Some(id) => self.play_item(host, id),
                None => self.ui.show_toast("That's the end of the playlist.", now),
            },
            Action::Prev => {
                let pos = self.session.as_ref().map_or(0, |s| s.position_us(now));
                if pos > PREV_RESTART_US || self.playlist.len() <= 1 {
                    if let Some(s) = &mut self.session {
                        s.seek(0);
                    }
                } else if let Some(id) = self.playlist.prev() {
                    self.play_item(host, id);
                }
            }
            Action::CycleRepeat => {
                let r = self.playlist.repeat().cycled();
                self.playlist.set_repeat(r);
                self.requeue();
                self.ui.show_toast(
                    match r {
                        Repeat::Off => "Repeat off",
                        Repeat::All => "Repeat all",
                        Repeat::One => "Repeat one",
                    },
                    now,
                );
            }
            Action::ToggleShuffle => {
                let on = !self.playlist.shuffle();
                self.playlist.set_shuffle(on, now as u64);
                self.requeue();
                self.ui.show_toast(if on { "Shuffle on" } else { "Shuffle off" }, now);
            }
            Action::PlayItem(id) => self.play_item(host, id),
            Action::RemoveItem(id) => {
                let was_current = self.playlist.remove(id);
                self.requeue();
                if was_current {
                    match self.playlist.current_id() {
                        Some(next) => self.play_item(host, next),
                        None => {
                            self.session = None;
                            host.video().clear();
                            self.base_dirty = true;
                            self.title.clear();
                        }
                    }
                }
            }
            Action::MoveItem(id, d) => {
                self.playlist.move_item(id, d as i32);
                self.requeue();
            }
            Action::ClearPlaylist => {
                self.playlist.clear();
                self.requeue();
                self.ui.show_toast("Playlist cleared", now);
            }
            Action::AddFiles => self.effects.push(Effect::AddFiles),
            Action::ShowPlaylist => {
                if self.ui.mode() == Mode::Library {
                    self.ui.show_view(rvp_ui::View::Queue);
                } else {
                    let m = self.model.clone();
                    self.ui.open_playlist_popup(&m);
                }
            }
            Action::FrameStep(d) => {
                if let Some(s) = &mut self.session {
                    if matches!(s.state(), SessionState::Playing | SessionState::Buffering) {
                        s.pause(now);
                    }
                    s.step_frame(d > 0);
                }
            }
            Action::LoopMark => match (self.loop_a, self.loop_b) {
                (None, _) => self.set_loop_point(true, now),
                (Some(_), None) => self.set_loop_point(false, now),
                (Some(_), Some(_)) => self.clear_loop(now),
            },
            Action::SeekAbs(us) => {
                if let Some(s) = &mut self.session {
                    s.seek(Self::clamp_pos(us, s.duration_us()));
                }
            }
            Action::ChapterStep(d) => {
                let pos = self.session.as_ref().map_or(0, |s| s.position_us(now));
                let ch = self.model.chapters.clone();
                if ch.is_empty() {
                    self.ui.show_toast("No chapters in this file.", now);
                } else {
                    // Forward: the first chapter after here. Back: the start of this chapter, or the one before
                    // when already near its start.
                    let target = if d > 0 {
                        ch.iter().find(|c| c.start_us > pos + 500_000)
                    } else {
                        let cur = ch.iter().rposition(|c| c.start_us <= pos).unwrap_or(0);
                        if pos - ch[cur].start_us > 3_000_000 || cur == 0 {
                            Some(&ch[cur])
                        } else {
                            Some(&ch[cur - 1])
                        }
                    };
                    match target {
                        Some(c) => {
                            if let Some(s) = &mut self.session {
                                s.seek(c.start_us);
                            }
                            let title =
                                if c.title.is_empty() { String::from("Chapter") } else { c.title.clone() };
                            self.ui.show_toast(&format!("Chapter: {title}"), now);
                        }
                        None => self.ui.show_toast("That was the last chapter.", now),
                    }
                }
            }
            Action::SetLoopA => self.set_loop_point(true, now),
            Action::SetLoopB => self.set_loop_point(false, now),
            Action::ClearLoop => self.clear_loop(now),
            Action::CycleAudio => {
                let tracks = self.model.audio_tracks.clone();
                if tracks.len() <= 1 {
                    self.ui.show_toast("Only one audio track in this file.", now);
                } else {
                    let cur = tracks.iter().position(|t| Some(t.id) == self.model.selected_audio);
                    let next = &tracks[cur.map_or(0, |i| (i + 1) % tracks.len())];
                    self.select_audio(next.id, &next.label.clone(), now);
                }
            }
            Action::SelectAudio(id) => {
                let label = self
                    .model
                    .audio_tracks
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| t.label.clone())
                    .unwrap_or_default();
                self.select_audio(id, &label, now);
            }
            Action::CycleSubtitles => {
                let tracks = self.model.subtitle_tracks.clone();
                if tracks.is_empty() {
                    self.ui.show_toast("No subtitles in this file.", now);
                } else {
                    // Off -> first -> second -> ... -> off.
                    let next = match self.model.selected_subtitle {
                        None => Some(tracks[0].id),
                        Some(cur) => {
                            let i = tracks.iter().position(|t| t.id == cur).unwrap_or(tracks.len());
                            tracks.get(i + 1).map(|t| t.id)
                        }
                    };
                    self.select_subtitle(next, now);
                }
            }
            Action::SelectSubtitle(id) => self.select_subtitle(id, now),
            Action::SetMode(m) => self.set_mode(m),
            Action::ToggleMode => {
                let m = if self.ui.mode() == Mode::Library { Mode::Player } else { Mode::Library };
                self.set_mode(m);
            }
            Action::ShowView(v) => self.ui.show_view(v),
            Action::OpenDetail(d) => self.ui.open_detail(d),
            Action::GoBack => {
                let ctx = Self::lib_ctx(&self.lib, None);
                self.ui.go_back(&self.model, &ctx);
            }
            Action::ToggleVisualizer => {
                let ctx = Self::lib_ctx(&self.lib, None);
                self.ui.toggle_visualizer(&self.model, &ctx);
            }
            Action::Lib(a) => self.apply_lib(host, a, now),
            Action::ShowAudioSettings => self.ui.open_audio_settings(),
            Action::SetCrossfade(on) => {
                self.update_settings(host, |s| s.crossfade = on);
                let secs = self.settings.crossfade_secs;
                self.settings_toast(
                    &if on { format!("Crossfade on ({secs} s)") } else { "Crossfade off".into() },
                    now,
                );
            }
            Action::SetCrossfadeSecs(n) => {
                self.update_settings(host, |s| s.crossfade_secs = n);
                self.settings_toast(&format!("Crossfade {} s", self.settings.crossfade_secs), now);
            }
            Action::SetAutoLevel(on) => {
                self.update_settings(host, |s| s.auto_level = on);
                let t = self.settings.target_lufs;
                self.settings_toast(
                    &if on { format!("Auto-level on ({t} LUFS)") } else { "Auto-level off".into() },
                    now,
                );
            }
            Action::SetTargetLufs(l) => {
                self.update_settings(host, |s| s.target_lufs = l);
                self.settings_toast(&format!("Level target {} LUFS", self.settings.target_lufs), now);
            }
            Action::SetLevelMode(m) => {
                self.update_settings(host, |s| s.level_mode = m);
                self.settings_toast(
                    if m == LevelMode::Album { "Leveling whole albums" } else { "Leveling each track" },
                    now,
                );
            }
        }
    }

    /// Switch faces; the picture layer is drawn again.
    fn set_mode(&mut self, m: Mode) {
        self.ui.set_mode(m);
        self.base_dirty = true;
        self.force_draw = true;
    }

    fn set_loop_point(&mut self, is_a: bool, now: Timestamp) {
        let Some(s) = &self.session else { return };
        let pos = s.position_us(now);
        if is_a {
            self.loop_a = Some(pos);
            if self.loop_b.is_some_and(|b| b <= pos) {
                self.loop_b = None;
            }
            self.ui.show_toast(&format!("Loop start {}", rvp_ui::format_time(pos)), now);
        } else {
            match self.loop_a {
                Some(a) if pos > a + 200_000 => {
                    self.loop_b = Some(pos);
                    self.ui.show_toast(
                        &format!("Looping {} to {}", rvp_ui::format_time(a), rvp_ui::format_time(pos)),
                        now,
                    );
                }
                Some(_) => {
                    self.ui.show_toast("Loop end must come after the start.", now);
                    return;
                }
                None => {
                    self.ui.show_toast("Set the loop start first (I).", now);
                    return;
                }
            }
        }
        if let (Some(a), Some(b), Some(s)) = (self.loop_a, self.loop_b, &mut self.session) {
            s.set_loop(Some((a, b)));
        } else if let Some(s) = &mut self.session {
            s.set_loop(None);
        }
    }

    fn clear_loop(&mut self, now: Timestamp) {
        self.loop_a = None;
        self.loop_b = None;
        if let Some(s) = &mut self.session {
            s.set_loop(None);
        }
        self.ui.show_toast("Loop cleared", now);
    }

    /// The play order changed: forget the item queued as next so the right one is queued.
    fn requeue(&mut self) {
        if let Some(s) = &mut self.session {
            s.cancel_next();
        }
        self.queued = None;
    }

    fn select_audio(&mut self, id: u32, label: &str, now: Timestamp) {
        if let Some(s) = &mut self.session {
            s.select_audio(id, now);
        }
        self.ui.show_toast(&format!("Audio: {label}"), now);
    }

    fn select_subtitle(&mut self, id: Option<u32>, now: Timestamp) {
        if let Some(s) = &mut self.session {
            s.select_subtitle(id);
        }
        let msg = match id.and_then(|i| self.model.subtitle_tracks.iter().find(|t| t.id == i)) {
            Some(t) => format!("Subtitles: {}", t.label),
            None => "Subtitles off".to_string(),
        };
        self.ui.show_toast(&msg, now);
    }

    fn clamp_pos(t: Timestamp, dur: Option<Timestamp>) -> Timestamp {
        let t = t.max(0);
        match dur {
            Some(d) if d > 0 => t.min((d - 1_000).max(0)),
            _ => t,
        }
    }

    fn set_speed(&mut self, r: f64, now: Timestamp) {
        let r = r.clamp(SPEEDS[0] as f64, SPEEDS[SPEEDS.len() - 1] as f64);
        self.rate = r;
        if let Some(s) = &mut self.session {
            s.set_rate(r, now);
        }
        self.ui.show_toast(&format!("Speed {}", rvp_ui::actions::speed_label(r as f32)), now);
    }

    fn push_audio_state(&mut self) {
        if let Some(s) = &mut self.session {
            s.set_volume(self.volume);
            s.set_muted(self.muted);
        }
    }

    // ---- model -----------------------------------------------------------------------------------------

    fn refresh_model(&mut self, now: Timestamp) {
        let mut m = UiModel {
            volume: self.volume,
            muted: self.muted,
            rate: self.rate as f32,
            loop_a: self.loop_a,
            loop_b: self.loop_b,
            fullscreen: self.fullscreen,
            audio: self.settings,
            level_gain_db: self
                .session
                .as_ref()
                .filter(|_| self.settings.auto_level)
                .map(|s| s.level_gain_db()),
            ..UiModel::default()
        };
        m.playlist = self.queue_entries();
        m.queue_rev = [
            self.playlist.revision() as u64,
            self.playlist.current_id().unwrap_or(0) as u64,
            self.lib.lib.revision(),
        ]
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, v| (h ^ v).wrapping_mul(0x100_0000_01b3));
        let cur_item = self.playlist.current();
        m.now_track = cur_item.and_then(|i| i.track);
        m.now_art = m.now_track.and_then(|t| self.lib.lib.track(t)).map_or(0, |t| t.art);
        m.repeat = match self.playlist.repeat() {
            Repeat::Off => 0,
            Repeat::All => 1,
            Repeat::One => 2,
        };
        m.shuffle = self.playlist.shuffle();
        if let Some(s) = &self.session {
            m.title = self.title.clone();
            m.duration_us = s.duration_us();
            let pos = s.position_us(now);
            m.position_us = m.duration_us.map_or(pos, |d| pos.min(d));
            m.has_video = s.has_video();
            // Audio shows what its tags say (a library track, what the library shows for it); a video keeps its file name.
            if !m.has_video {
                if let Some(t) = m.now_track.and_then(|t| self.lib.lib.track(t)) {
                    m.title = t.display_title().to_string();
                } else if !self.lib.now.title.is_empty() {
                    m.title = self.lib.now.title.clone();
                }
            }
            m.artist = self.lib.now.artist.clone();
            m.album = self.lib.now.album.clone();
            m.state = match s.state() {
                SessionState::Opening => MediaState::Opening,
                SessionState::Paused => MediaState::Paused,
                SessionState::Buffering => MediaState::Buffering,
                SessionState::Playing => MediaState::Playing,
                SessionState::Ended => {
                    if let Some(d) = m.duration_us {
                        m.position_us = d;
                    }
                    MediaState::Ended
                }
                SessionState::Failed => MediaState::Failed,
            };
            if m.state == MediaState::Failed {
                m.error = s.error().map(|e| friendly_error(&e));
            }
            for (n, st) in s.audio_tracks().iter().enumerate() {
                let name = match &st.language {
                    Some(l) if !l.is_empty() && l != "und" => rvp_player::language_name(l),
                    _ => format!("Audio {}", n + 1),
                };
                let layout = match st.audio.map(|a| a.channels) {
                    Some(1) => ", mono".to_string(),
                    Some(2) => ", stereo".to_string(),
                    Some(c) => format!(", {c} ch"),
                    None => String::new(),
                };
                m.audio_tracks.push(TrackItem { id: st.id, label: format!("{name} ({}{layout})", st.codec) });
            }
            m.selected_audio = s.selected_audio();
            for t in s.subtitle_tracks() {
                m.subtitle_tracks.push(TrackItem { id: t.id, label: t.label });
            }
            m.selected_subtitle = s.selected_subtitle();
            m.chapters = s
                .chapters()
                .into_iter()
                .map(|c| rvp_ui::ChapterItem { start_us: c.start_us, title: c.title })
                .collect();
            m.repeat = match self.playlist.repeat() {
                Repeat::Off => 0,
                Repeat::All => 1,
                Repeat::One => 2,
            };
            m.shuffle = self.playlist.shuffle();
            m.subtitle = s.subtitle_text().map(|t| t.to_string());
            m.subtitle_cues = s.subtitle_cues().to_vec();
            m.video_size = self.drawn_size;
            let warnings = s.warnings();
            if warnings.len() > self.warnings_seen {
                for w in &warnings[self.warnings_seen..] {
                    if w.starts_with("video disabled") {
                        self.ui.show_toast(
                            "No picture: that video codec isn't on the guest list yet. Audio plays on.",
                            now,
                        );
                    } else if w.starts_with("audio disabled") {
                        self.ui.show_toast(
                            "No sound: that audio codec isn't on the guest list. Video plays on.",
                            now,
                        );
                    }
                }
                self.warnings_seen = warnings.len();
            }
        }
        self.model = m;
    }

    // ---- drawing ---------------------------------------------------------------------------------------

    fn render<H>(&mut self, host: &mut H, now: Timestamp) -> bool
    where
        H: Host<Video = FrameSink>,
    {
        let (sw, sh, dpr) = host.surface().size();
        let (sw, sh) = (sw.max(1), sh.max(1));
        if (sw, sh) != (self.fb.width, self.fb.height) {
            self.fb.resize(sw, sh);
            self.base.resize(sw, sh);
            self.base_dirty = true;
            self.force_draw = true;
        }
        self.ui.set_size(sw, sh, dpr);
        let ui_dirty = self.ui.update(now, &self.model);

        let lib_mode = self.ui.mode() == Mode::Library;
        let has_media = self.model.has_media();
        // The layer under the chrome depends on the face, the view and whether the visualizer is on.
        let base_key = (
            lib_mode,
            self.ui.lib_state().view(),
            self.ui.lib_state().detail().is_some(),
            self.ui.lib_state().viz_on(),
        );
        if has_media != self.last_has_media
            || lib_mode != self.last_lib_mode
            || self.last_base_key != Some(base_key)
        {
            self.last_has_media = has_media;
            self.last_lib_mode = lib_mode;
            self.last_base_key = Some(base_key);
            self.base_dirty = true;
        }
        let tb0 = host.clock().now_us();
        let video = host.video();
        let video_changed = video.count != self.drawn_video;
        if video_changed || self.base_dirty {
            if lib_mode {
                let ctx = Self::lib_ctx(&self.lib, None);
                self.ui.draw_base_lib(&mut self.base, &self.model, &ctx);
            } else {
                let frame = (video.width > 0).then_some((video.rgba.as_slice(), video.width, video.height));
                self.ui.draw_base(&mut self.base, &self.model, frame);
            }
            self.drawn_video = video.count;
            self.drawn_size = (video.width, video.height);
            self.base_dirty = false;
        }
        // The seek bar and clock move while playing with the controls up; otherwise position changes are invisible.
        // Moving the seek bar and clock does not need 60 redraws a second: position is compared at 50 ms steps (a
        // quarter of a second in the library, whose bar only shows whole seconds).
        let mut cmp = self.model.clone();
        let step = if lib_mode { LIB_LIVE_REDRAW_US } else { LIVE_REDRAW_US };
        cmp.position_us = if self.ui.controls_visible() { cmp.position_us / step * step } else { 0 };
        let changed = self.last_drawn.as_ref() != Some(&cmp);
        let progress = self.lib.scan_status.as_ref().map_or(usize::MAX, |s| s.done);
        let lib_changed = lib_mode
            && (self.lib.lib.revision() != self.drawn_lib_rev || progress != self.lib.last_scan_progress);
        if !(video_changed || ui_dirty || changed || lib_changed || self.force_draw) {
            return false;
        }
        self.force_draw = false;
        self.last_drawn = Some(cmp);
        self.drawn_lib_rev = self.lib.lib.revision();
        self.lib.last_scan_progress = progress;
        let tb1 = host.clock().now_us();
        // Watching with the controls away: the picture layer is the whole frame, so skip the copy and the chrome.
        let bare = !lib_mode
            && self.model.state == MediaState::Playing
            && self.model.has_video
            && self.model.subtitle.is_none()
            && self.model.subtitle_cues.is_empty()
            && !self.ui.has_overlay();
        if !bare {
            self.fb.copy_from(&self.base);
            if lib_mode {
                let video = host.video();
                let frame = (video.width > 0 && self.model.has_video).then_some((
                    video.rgba.as_slice(),
                    video.width,
                    video.height,
                ));
                let ctx = Self::lib_ctx(&self.lib, frame);
                self.ui.draw_overlay_lib(&mut self.fb, &self.model, &ctx);
            } else {
                self.ui.draw_overlay(&mut self.fb, &self.model);
            }
        }
        let tb2 = host.clock().now_us();
        let pixels = if bare { &self.base.pixels } else { &self.fb.pixels };
        host.surface().present_rgba(pixels, Rect { x: 0, y: 0, w: sw, h: sh });
        let tb3 = host.clock().now_us();
        self.perf.base_us += tb1 - tb0;
        self.perf.overlay_us += tb2 - tb1;
        self.perf.present_us += tb3 - tb2;
        self.frames_drawn += 1;
        true
    }
}

/// True for the names of sidecar subtitle files.
fn is_subtitle_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".srt") || n.ends_with(".vtt") || n.ends_with(".ass") || n.ends_with(".ssa")
}

/// An error in the app's voice.
pub fn friendly_error(e: &Error) -> String {
    match e {
        Error::Unsupported(m) => {
            let mut c = m.chars();
            let cap: String =
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
            format!("{cap} isn't on the guest list.")
        }
        Error::Truncated => "This file ends too early. Is the download complete?".into(),
        Error::Invalid(m) => format!("This file looks damaged ({m})."),
        Error::Host(m) => format!("Couldn't read the file ({m})."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_speak_the_brand_voice() {
        assert_eq!(
            friendly_error(&Error::Unsupported("video codec `hevc`".into())),
            "Video codec `hevc` isn't on the guest list."
        );
        assert!(friendly_error(&Error::Truncated).contains("ends too early"));
    }
}
