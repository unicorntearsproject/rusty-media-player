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
use rvp_core::{CodecFactory, Error, StreamKind, Timestamp};
use rvp_host::{FrameSink, Host, InputEvent, OpenRequest, Rect};
use rvp_player::{Session, SessionState};
use rvp_ui::{Action, Cursor, FrameBuffer, MediaState, SPEEDS, TrackItem, Ui, UiConfig, UiModel};

/// Granularity at which a moving position triggers a redraw (20 Hz).
const LIVE_REDRAW_US: Timestamp = 50_000;

/// Something the host has to do on the app's behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Show the file picker (in a browser this must happen inside the user's input event).
    PickFile,
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

    /// Open `source` and start playing it, replacing the current item.
    pub fn open<H>(&mut self, host: &mut H, source: H::Source)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        use rvp_host::Source;
        let now = host.clock().now_us();
        self.title = source.name().to_string();
        let mut s = Session::new(source, self.codecs.clone());
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
    pub fn apply<H: Host<Video = FrameSink>>(&mut self, host: &mut H, action: Action, now: Timestamp) {
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
            Action::CycleAudio | Action::SelectAudio(_) => {
                let n = self.model.audio_tracks.len();
                let msg = if n <= 1 {
                    "Only one audio track in this file."
                } else {
                    "Switching audio tracks lands in M8."
                };
                self.ui.show_toast(msg, now);
            }
            Action::CycleSubtitles | Action::SelectSubtitle(_) => {
                let msg = if self.model.subtitle_tracks.is_empty() {
                    "No subtitles in this file."
                } else {
                    "Subtitles land in M8."
                };
                self.ui.show_toast(msg, now);
            }
        }
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
            fullscreen: self.fullscreen,
            ..UiModel::default()
        };
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
            let audio_streams = s.streams();
            for (n, st) in audio_streams.iter().filter(|st| st.kind == StreamKind::Audio).enumerate() {
                let name = st.language.clone().unwrap_or_else(|| format!("Audio {}", n + 1));
                let layout = match st.audio.map(|a| a.channels) {
                    Some(1) => ", mono".to_string(),
                    Some(2) => ", stereo".to_string(),
                    Some(c) => format!(", {c} ch"),
                    None => String::new(),
                };
                m.audio_tracks.push(TrackItem { id: st.id, label: format!("{name} ({}{layout})", st.codec) });
            }
            m.selected_audio = s.selected_audio();
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
        let bare = self.model.state == MediaState::Playing && self.model.has_video && !self.ui.has_overlay();
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
