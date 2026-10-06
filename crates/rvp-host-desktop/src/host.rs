//! The desktop [`Host`]: the clock, the audio sink, the frame sink, the window surface, the input queue, storage, directory access and
//! the system media controls, wired together.
use crate::audio::DesktopAudio;
use crate::media::DesktopNowPlaying;
use crate::source::FileSource;
use crate::storage::FileStorage;
use rvp_core::Timestamp;
use rvp_host::{
    FrameSink, Host, HostClock, HostError, InputEvent, InputEvents, Library, Listing, NowPlaying,
    OpenRequest, Rect, StandardFolder, StandardKind, Surface,
};
use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

/// Monotonic time since the program started.
pub struct DesktopClock {
    start: Instant,
    wake: Cell<Timestamp>,
}

impl DesktopClock {
    /// Start now.
    pub fn new() -> Self {
        Self { start: Instant::now(), wake: Cell::new(i64::MAX) }
    }

    /// The earliest time anyone asked to be woken at since the last call (and forget it).
    pub fn take_wake(&self) -> Option<Timestamp> {
        let w = self.wake.replace(i64::MAX);
        (w != i64::MAX).then_some(w)
    }
}

impl Default for DesktopClock {
    fn default() -> Self {
        Self::new()
    }
}

impl HostClock for DesktopClock {
    fn now_us(&self) -> Timestamp {
        self.start.elapsed().as_micros() as Timestamp
    }

    fn request_wake(&self, at_us: Timestamp) {
        self.wake.set(self.wake.get().min(at_us));
    }
}

/// Takes a finished frame (RGBA, width, height) to the window.
pub type WindowPresent = Box<dyn FnMut(&[u8], u32, u32)>;

/// The window's pixels: what the app presents goes to the `softbuffer` surface, and the last frame is kept for screenshots.
pub struct DesktopSurface {
    /// Physical width, height and the scale factor.
    pub size: (u32, u32, f32),
    pub(crate) fullscreen_request: Option<bool>,
    pub(crate) sink: Option<WindowPresent>,
    /// The last frame presented (kept only when `keep` is set).
    pub last: Vec<u8>,
    /// Keep a copy of the last frame (for `--screenshot`).
    pub keep: bool,
    /// Frames presented.
    pub presents: u64,
}

impl DesktopSurface {
    fn new() -> Self {
        Self {
            size: (1280, 720, 1.0),
            fullscreen_request: None,
            sink: None,
            last: Vec::new(),
            keep: false,
            presents: 0,
        }
    }
}

impl Surface for DesktopSurface {
    fn size(&self) -> (u32, u32, f32) {
        self.size
    }

    fn present_rgba(&mut self, rgba: &[u8], _dirty: Rect) {
        let (w, h, _) = self.size;
        if rgba.len() != w as usize * h as usize * 4 {
            return; // a frame for an old size: the next one is right
        }
        if self.keep {
            self.last.clear();
            self.last.extend_from_slice(rgba);
        }
        self.presents += 1;
        if let Some(sink) = &mut self.sink {
            sink(rgba, w, h);
        }
    }

    fn set_fullscreen(&mut self, on: bool) {
        self.fullscreen_request = Some(on);
    }
}

/// Events the window loop pushes for the app.
#[derive(Default)]
pub struct DesktopInput(pub VecDeque<InputEvent>);

impl InputEvents for DesktopInput {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

/// Folder listings made by the walker threads, and which folders are known.
pub struct DesktopLibrary {
    tx: Sender<Listing>,
    rx: Receiver<Listing>,
    /// Folders in the library: root id -> path.
    pub roots: BTreeMap<String, PathBuf>,
    ready: VecDeque<Listing>,
}

impl DesktopLibrary {
    fn new() -> Self {
        let (tx, rx) = channel();
        Self { tx, rx, roots: BTreeMap::new(), ready: VecDeque::new() }
    }

    /// Walk `dir` (on a thread) and remember it as a library folder.
    pub fn add(&mut self, dir: PathBuf) {
        self.roots.insert(crate::walk::root_id(&dir), dir.clone());
        crate::walk::spawn_walk(dir, self.tx.clone());
    }

    /// Walk a known folder again.
    pub fn rescan(&mut self, root: &str) {
        if let Some(dir) = self.roots.get(root).cloned().or_else(|| crate::walk::root_path(root)) {
            crate::walk::spawn_walk(dir, self.tx.clone());
        }
    }

    /// Forget a folder.
    pub fn forget(&mut self, root: &str) {
        self.roots.remove(root);
    }
}

impl Library for DesktopLibrary {
    fn take_listing(&mut self) -> Option<Listing> {
        while let Ok(l) = self.rx.try_recv() {
            self.ready.push_back(l);
        }
        self.ready.pop_front()
    }

    fn connected_roots(&self) -> Vec<String> {
        self.roots.iter().filter(|(_, p)| p.is_dir()).map(|(id, _)| id.clone()).collect()
    }

    fn standard_folders(&mut self) -> Vec<StandardFolder> {
        standard_folders()
    }

    fn add_path(&mut self, path: &str) -> bool {
        let dir = PathBuf::from(path);
        if !dir.is_dir() {
            return false;
        }
        self.add(dir);
        true
    }
}

/// The user's Music and Videos folders as the system names them: the XDG user directories (`user-dirs.dirs`) on Linux, the Known Folders
/// on Windows, `~/Music` and `~/Movies` on macOS. Only those that exist, and never the home folder itself (a system without a
/// Videos folder points it there).
pub fn standard_folders() -> Vec<StandardFolder> {
    let Some(dirs) = directories::UserDirs::new() else { return Vec::new() };
    let home = dirs.home_dir().to_path_buf();
    let mut out = Vec::new();
    for (kind, dir) in [(StandardKind::Music, dirs.audio_dir()), (StandardKind::Videos, dirs.video_dir())] {
        let Some(dir) = dir else { continue };
        if !dir.is_dir() || dir == home.as_path() {
            continue;
        }
        let name = dir.file_name().map_or_else(|| "Folder".to_string(), |n| n.to_string_lossy().into_owned());
        out.push(StandardFolder { kind, name, path: dir.to_string_lossy().into_owned() });
    }
    out
}

/// Everything the app needs from the desktop.
pub struct DesktopHost {
    /// Clock.
    pub clock: DesktopClock,
    /// Audio sink.
    pub audio: DesktopAudio,
    /// Latest decoded picture.
    pub video: FrameSink,
    /// The window's pixels.
    pub surface: DesktopSurface,
    /// Pending input.
    pub input: DesktopInput,
    /// Files under the data directory.
    pub storage: FileStorage,
    /// Library folders.
    pub library: DesktopLibrary,
    /// The system media controls, if they could be started.
    pub media: Option<DesktopNowPlaying>,
    /// Update checks and the app-menu entry (off in scripted runs unless asked for).
    pub services: Option<crate::services::DesktopServices>,
    /// Replaces library files (the tag editor).
    pub writer: crate::writer::DesktopWriter,
    /// Fetching a link for the theme dialog.
    pub net: crate::net::DesktopNet,
}

impl DesktopHost {
    /// A host storing under `data_dir`.
    pub fn new(data_dir: PathBuf, silent_audio: bool) -> Self {
        Self {
            clock: DesktopClock::new(),
            audio: DesktopAudio::new(silent_audio),
            video: FrameSink::new(),
            surface: DesktopSurface::new(),
            input: DesktopInput::default(),
            storage: FileStorage::new(data_dir),
            library: DesktopLibrary::new(),
            media: None,
            services: None,
            writer: Default::default(),
            net: crate::net::DesktopNet::new(),
        }
    }
}

impl Host for DesktopHost {
    type Source = FileSource;
    type Audio = DesktopAudio;
    type Video = FrameSink;
    type Store = FileStorage;

    fn clock(&self) -> &dyn HostClock {
        &self.clock
    }
    fn audio(&mut self) -> &mut DesktopAudio {
        &mut self.audio
    }
    fn video(&mut self) -> &mut FrameSink {
        &mut self.video
    }
    fn surface(&mut self) -> &mut dyn Surface {
        &mut self.surface
    }
    fn input(&mut self) -> &mut dyn InputEvents {
        &mut self.input
    }
    fn storage(&mut self) -> &mut FileStorage {
        &mut self.storage
    }
    async fn open(&mut self, req: OpenRequest) -> Result<FileSource, HostError> {
        match req {
            OpenRequest::Id(path) => FileSource::open(&path),
            OpenRequest::Pick => Err(HostError("the window shows the file dialog itself".into())),
        }
    }
    fn now_playing(&mut self) -> Option<&mut dyn NowPlaying> {
        self.media.as_mut().map(|m| m as &mut dyn NowPlaying)
    }
    fn library(&mut self) -> Option<&mut dyn Library> {
        Some(&mut self.library)
    }
    fn net(&mut self) -> Option<&mut dyn rvp_host::Net> {
        Some(&mut self.net)
    }
    fn file_writer(&mut self) -> Option<&mut dyn rvp_host::FileWriter> {
        Some(&mut self.writer)
    }
    fn app_services(&mut self) -> Option<&mut dyn rvp_host::AppServices> {
        self.services.as_mut().map(|s| s as &mut dyn rvp_host::AppServices)
    }
    fn opens_links(&self) -> bool {
        true
    }
    fn stable_ids(&self) -> bool {
        true
    }
}
