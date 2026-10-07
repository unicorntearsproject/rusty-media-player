//! The first run and the last face.
//!
//! What is kept, in one small value (`settings/setup`): which face the app was last on (Library or Player), and whether the first-run
//! steps were done. On the very first run the app opens on the **Library** (the music side, not the "open a file" card) and adds the
//! system's Music and Videos folders when the host has them and the library is empty. Later runs open on the face the last run ended on;
//! a Player face with nothing to show (no queue came back) falls back to the Library rather than to the empty card. Opening a file from
//! outside still goes to the face that suits it (see `auto_switch_mode`).
use crate::App;
use alloc::string::String;
use rvp_host::{FrameSink, Host, Storage};
use rvp_ui::Mode;

/// Where the choices are kept.
pub const SETUP_KEY: &str = "settings/setup";

/// What is kept between runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Setup {
    /// The kept value was read (or there was none).
    loaded: bool,
    /// There was no kept value: this is the first run.
    first_run: bool,
    /// The face the last run ended on (`None` before it is known).
    face: Option<Mode>,
    /// The standard folders were looked at.
    folders_done: bool,
    /// A Player face was restored: if nothing comes back to play, the Library is the better landing.
    face_fallback: bool,
    /// The kept value should be written.
    dirty: bool,
    /// Why a standard folder was left out of the first run (the system names the home folder as it); shown once, not kept.
    folders_note: Option<String>,
}

impl Setup {
    /// The text to keep.
    pub fn to_text(&self) -> String {
        alloc::format!(
            "rvp-setup 1\nface={}\nfolders={}\n",
            if self.face == Some(Mode::Player) { "player" } else { "library" },
            if self.folders_done { "done" } else { "todo" }
        )
    }

    /// Read kept text.
    pub fn from_text(text: &str) -> Option<Setup> {
        let mut lines = text.lines();
        if lines.next()?.trim() != "rvp-setup 1" {
            return None;
        }
        let mut s = Setup { loaded: true, ..Setup::default() };
        for l in lines {
            let Some((k, v)) = l.split_once('=') else { continue };
            match (k.trim(), v.trim()) {
                ("face", "player") => s.face = Some(Mode::Player),
                ("face", _) => s.face = Some(Mode::Library),
                ("folders", "done") => s.folders_done = true,
                _ => {}
            }
        }
        Some(s)
    }

    /// Why a standard folder was left out of the first run, if one was.
    pub fn folders_note(&self) -> Option<&str> {
        self.folders_note.as_deref()
    }

    /// True on the first run (until the next one).
    pub fn is_first_run(&self) -> bool {
        self.first_run
    }
}

impl App {
    /// The face to start on, the standard folders on the first run, and keeping the last face.
    pub(crate) fn setup_tick<H>(&mut self, host: &mut H)
    where
        H: Host<Video = FrameSink>,
    {
        if !self.setup.loaded {
            let stored = rvp_core::task::block_on(host.storage().load(SETUP_KEY));
            match stored.as_deref().and_then(|b| core::str::from_utf8(b).ok()).and_then(Setup::from_text) {
                Some(s) => {
                    self.setup = s;
                    // Nothing was opened from outside yet (this runs before the first input is read).
                    if self.setup.face == Some(Mode::Player) {
                        self.setup.face_fallback = true;
                        self.ui.set_mode(Mode::Player);
                    } else {
                        self.ui.set_mode(Mode::Library);
                    }
                }
                None => {
                    self.setup = Setup { loaded: true, first_run: true, ..Setup::default() };
                    self.ui.set_mode(Mode::Library);
                    self.setup.face = Some(Mode::Library);
                    self.setup.dirty = true;
                }
            }
            self.base_dirty = true;
            self.force_draw = true;
        }
        // The system's Music and Videos folders, once, when the library is still empty (a first run, or a run that never had them).
        if !self.setup.folders_done && self.lib.loaded {
            self.setup.folders_done = true;
            self.setup.dirty = true;
            if self.lib.lib.roots().is_empty() {
                if let Some(l) = host.library() {
                    for f in l.standard_folders() {
                        l.add_path(&f.path);
                    }
                    self.setup.folders_note = l.standard_folders_note();
                    if let Some(note) = self.setup.folders_note.clone() {
                        let now = rvp_host::HostClock::now_us(host.clock());
                        self.ui.show_toast(&note, now);
                    }
                }
            }
        }
        // A Player face that has nothing to show once the queue had its chance to come back: the Library instead.
        if self.setup.face_fallback && self.restore.done {
            self.setup.face_fallback = false;
            if self.session.is_none() && self.playlist.is_empty() && self.ui.mode() == Mode::Player {
                self.ui.set_mode(Mode::Library);
                self.base_dirty = true;
                self.force_draw = true;
            }
        }
        // Keep the face the app is on (not while a restored Player face is still waiting for its queue).
        let face = self.ui.mode();
        if !self.setup.face_fallback && self.setup.face != Some(face) {
            self.setup.face = Some(face);
            self.setup.dirty = true;
        }
        if self.setup.dirty {
            self.setup.dirty = false;
            let text = self.setup.to_text();
            rvp_core::task::block_on(host.storage().store(SETUP_KEY, text.as_bytes()));
        }
    }

    /// What the first-run steps did (for tests and the snapshot).
    pub fn setup(&self) -> &Setup {
        &self.setup
    }
}
