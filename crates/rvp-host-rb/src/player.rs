//! The driver: owns the host and the app, turns App API events into input and requests, and runs the loop
//! `tick, then events_wait until an event or the app's wake-up time`.
use crate::api;
use crate::events::{self, Ev};
use crate::host::RbHost;
use crate::input::{map_button, map_key, map_mods};
use crate::media::transport_command;
use crate::surface::RbSurface;
use bucket_v0_sys::{self as sys, caps, err, launch, power};
use rvp_app::{App, Effect};
use rvp_core::CodecFactory;
use rvp_host::{HostClock, InputEvent, Key, Modifiers};
use rvp_ui::{Cursor, MediaState, UiConfig};
use std::collections::HashMap;
use std::rc::Rc;

/// File extensions the picker offers and the manifest declares (`[[opens]]`): media, subtitles and playlists.
pub const OPEN_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mka", "mp3", "mp2", "flac", "ogg", "oga", "opus", "wav", "m4a", "m4b",
    "aac", "srt", "vtt", "m3u", "m3u8", "pls",
];

/// How long to wait for events when the app asked for no wake-up (nothing playing): the UI still needs frames for hover fades and
/// toasts, so this is a frame, not a second.
const IDLE_WAIT_US: i64 = 16_000;
/// The longest the loop sleeps (the player's own tick is 10 ms, so a longer request is clamped).
const MAX_WAIT_US: i64 = 100_000;
/// A multi-file drop whose last event never comes is opened anyway after this long.
const BATCH_TIMEOUT_US: i64 = 1_000_000;

/// Why a picker was shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickKind {
    /// Replace the queue.
    Open,
    /// Add to the queue.
    Add,
    /// A playlist file to import (the app recognises it by its extension).
    Import,
}

/// Files of one drop, pick or launch that arrive as several events.
struct Batch {
    items: Vec<(String, String)>,
    append: bool,
    from_drop: bool,
    last_us: i64,
}

/// The driver. Create it once the module starts, then call [`RbPlayer::run`] (or [`RbPlayer::run_once`] from a test).
pub struct RbPlayer {
    /// The host.
    pub host: RbHost,
    /// The application.
    pub app: App,
    picks: HashMap<i32, PickKind>,
    saves: HashMap<i32, String>,
    folder_requests: Vec<i32>,
    batch: Option<Batch>,
    cursor: i32,
    power_display: bool,
    power_system: bool,
    exit: Option<i32>,
    cpu_count: u32,
    /// Why the OS started this instance (`launch_reason`).
    pub launch_reason: i32,
}

impl RbPlayer {
    /// Read the OS, create the host and the app, tell the app the canvas size and, after a crash or a kill, say so.
    pub fn new(codecs: Rc<dyn CodecFactory>, config: UiConfig) -> Self {
        let host = RbHost::new();
        // SAFETY: no arguments.
        let launch_reason = unsafe { sys::launch_reason() };
        Self::with_host(host, codecs, config, launch_reason)
    }

    /// With a given host (tests).
    pub fn with_host(
        host: RbHost,
        codecs: Rc<dyn CodecFactory>,
        config: UiConfig,
        launch_reason: i32,
    ) -> Self {
        let mut p = Self {
            host,
            app: App::new(codecs, config),
            picks: HashMap::new(),
            saves: HashMap::new(),
            folder_requests: Vec::new(),
            batch: None,
            cursor: sys::cursor::DEFAULT,
            power_display: false,
            power_system: false,
            exit: None,
            // SAFETY: no arguments.
            cpu_count: unsafe { sys::cpu_count() }.max(1) as u32,
            launch_reason,
        };
        let (w, h, dpr) = p.host.surface.size;
        p.host.input.0.push_back(InputEvent::Resize { w, h, dpr });
        if RbSurface::is_fullscreen() {
            p.app.set_fullscreen_state(true);
        }
        match launch_reason {
            launch::AFTER_TRAP => p.toast("The player crashed and was restarted."),
            launch::AFTER_KILL => p.toast("The player was restarted."),
            _ => {}
        }
        p
    }

    /// The exit status once the app was told to end (`TERMINATE`, `INTERRUPT`).
    pub fn exit_status(&self) -> Option<i32> {
        self.exit
    }

    /// The number of cores the OS lets the app use (updated by `CPU_COUNT_CHANGED`).
    pub fn cpu_count(&self) -> u32 {
        self.cpu_count
    }

    fn now(&self) -> i64 {
        self.host.clock.now_us()
    }

    fn toast(&mut self, text: &str) {
        let now = self.now();
        self.app.ui_mut().show_toast(text, now);
    }

    /// One pass: the events that arrived while the adapter waited for something else, the app's tick, what it asked for, the
    /// visualizer batch, the cursor and the sleep inhibitors.
    pub fn step(&mut self) {
        let backlog: Vec<Ev> = self.host.shared.backlog.borrow_mut().drain(..).collect();
        self.dispatch(backlog);
        self.flush_stale_batch();
        self.app.tick(&mut self.host);
        self.handle_effects();
        self.host.viz.flush();
        if let Some(e) = self.host.audio.take_failure() {
            api::warn(&format!("the audio stream failed: {}", api::code_name(e)));
            self.toast("The audio device went away.");
        }
        self.update_cursor();
        self.update_power();
    }

    /// How long `events_wait` may sleep: until the app's wake-up time (at most `MAX_WAIT_US`), a frame if it has none, and not at
    /// all if events are already waiting.
    pub fn wait_timeout_us(&mut self) -> i64 {
        if !self.host.shared.backlog.borrow().is_empty() || !self.host.input.0.is_empty() {
            return 0;
        }
        let now = self.now();
        match self.host.clock.take_wake() {
            Some(w) => (w - now).clamp(0, MAX_WAIT_US),
            None => IDLE_WAIT_US,
        }
    }

    /// One turn of the loop: [`step`](Self::step), sleep, handle what came. Returns false once the app should end.
    pub fn run_once(&mut self) -> bool {
        self.step();
        if self.exit.is_some() {
            return false;
        }
        let timeout = self.wait_timeout_us();
        let evs = events::wait(timeout, self.host.shared.limits.events);
        self.dispatch(evs);
        self.exit.is_none()
    }

    /// Run until `TERMINATE` or `INTERRUPT`; returns the exit status for `bucket_main`.
    pub fn run(&mut self) -> i32 {
        while self.run_once() {}
        self.exit.unwrap_or(0)
    }

    /// Ask the OS for a clean relaunch (`restart`), after saving everything: for a worker that stopped answering, where a trap
    /// would lose the state of the other threads. `restart` does not return when it works (the app ends at once, and the OS counts
    /// the restart against the manifest's `restart_limit`), so coming back means the OS refused: the result is always false.
    pub fn restart(&mut self) -> bool {
        self.save_everything();
        // SAFETY: no arguments.
        let r = unsafe { sys::restart() };
        api::warn(&format!("restart was refused: {}", api::code_name(r)));
        false
    }

    fn save_everything(&mut self) {
        self.app.save_state(&mut self.host);
        self.host.storage.flush();
    }

    /// Save, give up the media session, release the sleep inhibitors and set the exit status.
    fn shutdown(&mut self) {
        if let Some(c) = self.host.audio.clock() {
            api::trace(&format!("audio.exit frames_played={} underruns={}", c.frames_played, c.underruns));
        }
        self.save_everything();
        if self.host.shared.has(caps::NOW_PLAYING) {
            self.host.now_playing.clear();
        }
        self.set_power(false, false);
        self.exit = Some(0);
    }

    // ---- events ----

    /// Handle decoded events. `OPEN` groups its files with flag bit 1 ("more follow"). A host that never sets the bit (older
    /// ones) is told by a batch of several `OPEN`s with no bit at all: those arrive together as one queue, so all but the
    /// last say "more".
    pub fn dispatch(&mut self, evs: Vec<Ev>) {
        let last_open = evs.iter().rposition(|e| matches!(e, Ev::Open { .. }));
        let flagged = evs.iter().any(|e| matches!(e, Ev::Open { more: true, .. }));
        for (i, e) in evs.into_iter().enumerate() {
            match e {
                Ev::Open { file, more, .. } => {
                    let more = more || (!flagged && last_open.is_some_and(|l| i < l));
                    self.on_file(file, more, false, true)
                }
                e => self.on_event(e),
            }
        }
    }

    fn push(&mut self, ev: InputEvent) {
        self.host.input.0.push_back(ev);
    }

    fn on_event(&mut self, e: Ev) {
        match e {
            Ev::Key { down, key, mods, repeat } => {
                if let Some(key) = map_key(key) {
                    let mods = map_mods(mods);
                    self.push(if down {
                        InputEvent::KeyDown { key, mods, repeat }
                    } else {
                        InputEvent::KeyUp { key, mods }
                    });
                }
            }
            Ev::Text(t) => {
                // Composed input only (an IME): each character as a key press, so the library search takes it.
                for c in t.chars().filter(|c| !c.is_control()) {
                    let key = if c == ' ' { Key::Space } else { Key::Char(c) };
                    self.push(InputEvent::KeyDown {
                        key: key.clone(),
                        mods: Modifiers::default(),
                        repeat: false,
                    });
                    self.push(InputEvent::KeyUp { key, mods: Modifiers::default() });
                }
            }
            Ev::PointerMove { x, y } => self.push(InputEvent::PointerMove { x, y }),
            Ev::Pointer { down, x, y, button } => {
                if let Some(button) = map_button(button) {
                    self.push(if down {
                        InputEvent::PointerDown { x, y, button }
                    } else {
                        InputEvent::PointerUp { x, y, button }
                    });
                }
            }
            // There is no "pointer left" input: a position far outside the canvas clears every hover state.
            Ev::PointerLeave => self.push(InputEvent::PointerMove { x: -1.0e6, y: -1.0e6 }),
            Ev::Wheel { dx, dy, pixels } => {
                // One detent is one line: 40 px at scale 1 (the UI's own notch), so lines turn into 40 x scale physical pixels.
                let k = if pixels { 1.0 } else { sys::WHEEL_LINE_PX * self.host.surface.size.2 };
                self.push(InputEvent::Wheel { dx: dx * k, dy: dy * k });
            }
            Ev::Resize { w, h, scale, fullscreen } => {
                let (w, h) = (w.max(1), h.max(1));
                let scale = if scale > 0.0 { scale } else { 1.0 };
                self.host.surface.size = (w, h, scale);
                self.app.set_fullscreen_state(fullscreen);
                self.push(InputEvent::Resize { w, h, dpr: scale });
            }
            Ev::Focus(on) => self.push(InputEvent::Focus(on)),
            Ev::DragOver(on) => self.push(InputEvent::DragOver(on)),
            Ev::Drop { file, more } => self.on_file(file, more, true, true),
            Ev::Open { file, more, .. } => self.on_file(file, more, false, true),
            Ev::FilePicked { request, file, more } => {
                let kind = self.picks.get(&request).copied();
                if !more || file < 0 {
                    self.picks.remove(&request);
                }
                if let Some(kind) = kind {
                    if file >= 0 {
                        // A pick is one request with several events; the last one has no "more" flag.
                        self.on_file(file, more, kind == PickKind::Add, false);
                    }
                }
            }
            Ev::Suspend => self.set_visible(false),
            Ev::Resume { .. } => self.set_visible(true),
            Ev::Visibility(state) => self.set_visible(state == sys::visibility::VISIBLE),
            Ev::Terminate { .. } | Ev::Interrupt => self.shutdown(),
            Ev::Reload => self.save_everything(),
            Ev::MemoryPressure(level) => {
                // The cover thumbnails are the one cache we can shrink at once (they are rebuilt from the library on demand).
                self.app.set_thumb_budget(if level >= 2 { 4 << 20 } else { 16 << 20 });
            }
            Ev::CapsChanged(new) => {
                let old = self.host.shared.caps.replace(new);
                if new & caps::NOW_PLAYING != 0 && old & caps::NOW_PLAYING == 0 {
                    // The shell began showing now-playing after the item started: tell it what plays.
                    self.host.now_playing.replay();
                }
            }
            Ev::CpuCount(n) => self.cpu_count = n.max(1),
            Ev::Transport { command, value_us, value_f32 } => {
                if let Some(c) = transport_command(command, value_us, value_f32) {
                    api::trace(&format!("np.command {c:?}"));
                    self.host.now_playing.push_command(c);
                }
            }
            Ev::FolderAdded { request, root } => {
                let Some(i) = self.folder_requests.iter().position(|r| *r == request) else { return };
                self.folder_requests.remove(i);
                match root {
                    // The OS starts the first walk itself; its `LIBRARY_LISTING` is what the player waits for.
                    Ok(root) => api::trace(&format!("library.folder_added root={root}")),
                    Err(err::CANCELLED) => {}
                    Err(_) => self.toast("Couldn't add that folder."),
                }
            }
            Ev::LibraryListing { root, partial } => self.host.library.on_listing(&root, partial),
            Ev::LibraryChanged { root } => self.rescan(&root),
            Ev::FileSaved { request, status } => {
                if let Some(name) = self.saves.remove(&request) {
                    match status {
                        0 => self.toast(&format!("Saved {name}")),
                        err::CANCELLED => {}
                        _ => self.toast(&format!("Couldn't save {name}")),
                    }
                }
            }
            Ev::AudioDeviceChanged { stream, rate, channels } => {
                self.host.audio.device_changed(stream, rate, channels)
            }
            Ev::AudioError { stream, error } => {
                if self.host.audio.stream() == Some(stream) {
                    self.host.audio.fail(error);
                }
            }
            // Retries come from the loop itself (every task is polled each tick); the event only ended the sleep.
            Ev::IoReady { .. } | Ev::Wake => {}
            // Not used: no `frame_request`, no theme (we draw with our own tokens), no Bucket Bar commands, no progress display.
            Ev::Frame { .. }
            | Ev::ThemeChanged
            | Ev::Command { .. }
            | Ev::LibraryProgress { .. }
            | Ev::Unknown(_) => {}
        }
    }

    fn set_visible(&mut self, visible: bool) {
        if visible && !self.host.surface.visible {
            self.host.surface.visible = true;
            // Frames were not sent while hidden.
            self.app.invalidate();
        } else if !visible {
            self.host.surface.visible = false;
        }
    }

    /// A file handle from outside (a drop, an open, a picked file): find its id and name, and open the group when it is complete.
    fn on_file(&mut self, handle: i32, more: bool, append: bool, from_drop: bool) {
        if handle <= 0 {
            return;
        }
        // SAFETY: `text_call` passes a buffer of the size it announces.
        let name = api::text_call(|p, cap| unsafe { sys::file_name(handle, p, cap) }).unwrap_or_default();
        // SAFETY: as above.
        let id = match api::text_call(|p, cap| unsafe { sys::file_id(handle, p, cap) }) {
            // A stable id reopens the file after a restart, so the handle is not needed (the OS limits how many an app holds).
            Ok(id) if !id.is_empty() => {
                // SAFETY: no pointers.
                unsafe { sys::file_close(handle) };
                id
            }
            // No id (a file the OS cannot name again): keep the handle for as long as the player needs it.
            _ => self.host.shared.stash_handle(handle),
        };
        let name = if name.is_empty() { id.clone() } else { name };
        api::trace(&format!("open name={name:?} id_len={} more={more}", id.len()));
        let now = self.now();
        let b =
            self.batch.get_or_insert_with(|| Batch { items: Vec::new(), append, from_drop, last_us: now });
        b.items.push((id, name));
        b.last_us = now;
        if !more {
            self.open_batch();
        }
    }

    fn flush_stale_batch(&mut self) {
        if self.batch.as_ref().is_some_and(|b| self.now() - b.last_us > BATCH_TIMEOUT_US) {
            self.open_batch();
        }
    }

    fn open_batch(&mut self) {
        let Some(b) = self.batch.take() else { return };
        if b.from_drop {
            self.push(InputEvent::DragOver(false));
        }
        self.app.open_items(&mut self.host, &b.items, b.append);
    }

    // ---- requests from the app ----

    fn handle_effects(&mut self) {
        for e in self.app.take_effects() {
            match e {
                Effect::PickFile => self.request_pick(false),
                Effect::AddFiles => self.request_pick(true),
                Effect::ImportPlaylist => self.pick(PickKind::Import, "m3u,m3u8,pls"),
                Effect::AddFolder => self.request_folder(),
                Effect::Rescan(root) => self.rescan(&root),
                Effect::Forget(root) => self.host.library.forget(&root),
                Effect::Download { name, mime, data } => {
                    // SAFETY: the ranges are the strings' and the slice's.
                    let r = unsafe {
                        sys::file_save(
                            name.as_ptr(),
                            api::len32(name.len()),
                            mime.as_ptr(),
                            api::len32(mime.len()),
                            data.as_ptr(),
                            api::len32(data.len()),
                        )
                    };
                    if r > 0 {
                        self.saves.insert(r, name);
                    } else {
                        self.toast("Couldn't save that file.");
                    }
                }
            }
        }
    }

    /// Ask for a new walk of `root`; an unreadable root says `-IO` and the user is told.
    fn rescan(&mut self, root: &str) {
        let r = self.host.library.rescan(root);
        if r == err::IO {
            self.toast("Couldn't read that folder.");
        } else if r < 0 {
            api::warn(&format!("library_rescan `{root}` failed: {}", api::code_name(r)));
        }
    }

    /// Show the folder picker for the library (`Effect::AddFolder`); the OS answers with `FOLDER_ADDED`.
    pub fn request_folder(&mut self) {
        // SAFETY: no arguments.
        let r = unsafe { sys::library_add_folder() };
        if r > 0 {
            self.folder_requests.push(r);
        } else {
            self.toast("Folders are not available here.");
        }
    }

    /// Show the file picker for media: to replace the queue, or (`append`) to add to it. What `Effect::PickFile` and
    /// `Effect::AddFiles` do.
    pub fn request_pick(&mut self, append: bool) {
        self.pick(if append { PickKind::Add } else { PickKind::Open }, &OPEN_EXTENSIONS.join(","));
    }

    fn pick(&mut self, kind: PickKind, kinds: &str) {
        // SAFETY: the range is the string's.
        let r = unsafe { sys::file_pick(kinds.as_ptr(), api::len32(kinds.len()), sys::pick::MULTI) };
        if r > 0 {
            self.picks.insert(r, kind);
        } else {
            self.toast("Couldn't show the file picker.");
        }
    }

    fn update_cursor(&mut self) {
        let want = match self.app.cursor() {
            Cursor::Default => sys::cursor::DEFAULT,
            Cursor::Pointer => sys::cursor::POINTER,
            Cursor::Grabbing => sys::cursor::GRABBING,
            Cursor::Hidden => sys::cursor::NONE,
        };
        if want != self.cursor {
            self.cursor = want;
            // SAFETY: no pointers.
            unsafe { sys::cursor_set(want) };
        }
    }

    /// Keep the display on while a picture plays and the machine awake while anything plays. Released when playback pauses, and
    /// by the OS when the app ends.
    fn update_power(&mut self) {
        let m = self.app.model();
        let playing = matches!(m.state, MediaState::Playing | MediaState::Buffering);
        let (display, system) = (playing && m.has_video, playing);
        self.set_power(display, system);
    }

    fn set_power(&mut self, display: bool, system: bool) {
        for (kind, now, was) in [
            (power::DISPLAY, display, &mut self.power_display),
            (power::SYSTEM, system, &mut self.power_system),
        ] {
            if *was != now {
                *was = now;
                // SAFETY: no pointers.
                unsafe { sys::power_inhibit(kind, now as i32) };
            }
        }
    }
}
