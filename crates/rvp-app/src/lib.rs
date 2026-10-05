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

mod snapshot;

pub use snapshot::Snapshot;

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_core::{CodecFactory, Error, Timestamp};
use rvp_host::{FrameSink, Host, InputEvent, OpenRequest, Rect, Storage};
use rvp_player::{Playlist, Repeat, Session, SessionEvent, SessionState};
use rvp_ui::{
    Action, Cursor, FrameBuffer, MediaState, PlaylistEntry, SPEEDS, TrackItem, Ui, UiConfig, UiModel,
};

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

/// Something the host has to do on the app's behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Show the file picker (in a browser this must happen inside the user's input event).
    PickFile,
    /// Show the file picker to add files to the playlist.
    AddFiles,
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
    warnings_seen: usize,
    loop_a: Option<Timestamp>,
    loop_b: Option<Timestamp>,
    playlist: Playlist,
    /// The playlist item queued to follow the current one gaplessly.
    queued: Option<u32>,
    resume_key: Option<String>,
    resume_checked: bool,
    last_resume_save: Timestamp,
    now: Timestamp,
    frames_drawn: u64,
    perf: Perf,
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
            codecs,
            session: None,
            ui: Ui::new(config),
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
            warnings_seen: 0,
            loop_a: None,
            loop_b: None,
            playlist: Playlist::new(),
            queued: None,
            resume_key: None,
            resume_checked: true,
            last_resume_save: 0,
            now: 0,
            frames_drawn: 0,
            perf: Perf::default(),
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

    /// The playlist.
    pub fn playlist(&self) -> &Playlist {
        &self.playlist
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
        self.save_resume(host, true);
        self.playlist.clear();
        let id = self.playlist.add(source.name(), "");
        self.playlist.set_current(id);
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
        let (subs, media): (Vec<_>, Vec<_>) = items.iter().partition(|(_, name)| is_subtitle_name(name));
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
        self.title = source.name().to_string();
        self.resume_key = Some(format!("resume:{}", self.title));
        self.resume_checked = !resume;
        self.last_resume_save = now;
        self.loop_a = None;
        self.loop_b = None;
        self.queued = None;
        let mut s = Session::new(source, self.codecs.clone());
        s.set_tag(id);
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
        let Some(key) = self.resume_key.clone() else { return };
        let Some(bytes) = rvp_core::task::block_on(host.storage().load(&key)) else { return };
        let Ok(raw) = <[u8; 8]>::try_from(bytes.as_slice()) else { return };
        let pos = i64::from_le_bytes(raw) * 1000;
        if pos >= RESUME_MIN_POSITION_US && pos + RESUME_MIN_REMAINING_US <= dur {
            s.seek(pos);
            self.ui.show_toast(&format!("Resumed at {}", rvp_ui::format_time(pos)), now);
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
                            if let Some(s) = &mut self.session {
                                s.queue_next(source, next);
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
        let actions = self.ui.handle(&ev, now, &self.model);
        for a in actions {
            self.apply(host, a, now);
        }
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
        let mut session = self.session.take();
        if let Some(s) = &mut session {
            s.tick(host);
        }
        self.session = session;
        let tn = host.clock().now_us();
        self.check_resume(host, tn);
        self.run_playlist(host, tn);
        self.save_resume(host, false);
        let t1 = host.clock().now_us();
        self.now = t1;
        self.refresh_model(t1);
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
                let m = self.model.clone();
                self.ui.open_playlist_popup(&m);
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
        }
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
            ..UiModel::default()
        };
        let cur = self.playlist.current_id();
        m.playlist = self
            .playlist
            .items()
            .iter()
            .map(|i| PlaylistEntry { id: i.id, label: i.name.clone(), current: Some(i.id) == cur })
            .collect();
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
            m.repeat = match self.playlist.repeat() {
                Repeat::Off => 0,
                Repeat::All => 1,
                Repeat::One => 2,
            };
            m.shuffle = self.playlist.shuffle();
            m.subtitle = s.subtitle_text().map(|t| t.to_string());
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

        let has_media = self.model.has_media();
        if has_media != self.last_has_media {
            self.last_has_media = has_media;
            self.base_dirty = true;
        }
        let tb0 = host.clock().now_us();
        let video = host.video();
        let video_changed = video.count != self.drawn_video;
        if video_changed || self.base_dirty {
            let frame = (video.width > 0).then_some((video.rgba.as_slice(), video.width, video.height));
            self.ui.draw_base(&mut self.base, &self.model, frame);
            self.drawn_video = video.count;
            self.drawn_size = (video.width, video.height);
            self.base_dirty = false;
        }
        // The seek bar and clock move while playing with the controls up; otherwise position changes are invisible.
        // Moving the seek bar and clock does not need 60 redraws a second: position is compared at 50 ms steps.
        let mut cmp = self.model.clone();
        cmp.position_us =
            if self.ui.controls_visible() { cmp.position_us / LIVE_REDRAW_US * LIVE_REDRAW_US } else { 0 };
        let changed = self.last_drawn.as_ref() != Some(&cmp);
        if !(video_changed || ui_dirty || changed || self.force_draw) {
            return false;
        }
        self.force_draw = false;
        self.last_drawn = Some(cmp);
        let tb1 = host.clock().now_us();
        // Watching with the controls away: the picture layer is the whole frame, so skip the copy and the chrome.
        let bare = self.model.state == MediaState::Playing
            && self.model.has_video
            && self.model.subtitle.is_none()
            && !self.ui.has_overlay();
        if !bare {
            self.fb.copy_from(&self.base);
            self.ui.draw_overlay(&mut self.fb, &self.model);
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
    n.ends_with(".srt") || n.ends_with(".vtt")
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
