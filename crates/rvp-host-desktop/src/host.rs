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

    fn unix_time(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
    }

    fn utc_offset_secs(&self) -> i32 {
        local_utc_offset(self.unix_time())
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
    /// The set of folders changed since it was last saved (the window saves it: it is what is read back at the next start).
    roots_dirty: bool,
    /// What `standard_folders` left out on purpose.
    folders_note: Option<String>,
}

impl DesktopLibrary {
    fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            roots: BTreeMap::new(),
            ready: VecDeque::new(),
            roots_dirty: false,
            folders_note: None,
        }
    }

    /// Walk `dir` (on a thread) and remember it as a library folder.
    pub fn add(&mut self, dir: PathBuf) {
        self.remember(dir.clone());
        crate::walk::spawn_walk(dir, self.tx.clone());
    }

    /// Know `dir` as a library folder without walking it (a remembered folder that is not there right now: an unplugged drive). It stays
    /// in the saved list, shows as not connected, and is walked again by a rescan once it is back.
    pub fn remember(&mut self, dir: PathBuf) {
        if self.roots.insert(crate::walk::root_id(&dir), dir).is_none() {
            self.roots_dirty = true;
        }
    }

    /// Whether the folders changed since the last call (and forget that).
    pub fn take_roots_dirty(&mut self) -> bool {
        std::mem::take(&mut self.roots_dirty)
    }

    /// Walk a known folder again.
    pub fn rescan(&mut self, root: &str) {
        if let Some(dir) = self.roots.get(root).cloned().or_else(|| crate::walk::root_path(root)) {
            crate::walk::spawn_walk(dir, self.tx.clone());
        }
    }

    /// Forget a folder.
    pub fn forget(&mut self, root: &str) {
        if self.roots.remove(root).is_some() {
            self.roots_dirty = true;
        }
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
        let (folders, note) = standard_folders_checked();
        self.folders_note = note;
        folders
    }

    fn standard_folders_note(&self) -> Option<String> {
        self.folders_note.clone()
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

/// Whether a folder is too broad to be a library: the home folder itself, anything above it (`/`, `/home`) or a system folder. A system that
/// names the home folder as its Videos folder (a setting some distributions make when the user has none) would otherwise have the first
/// run walk the whole home: repositories, build output, caches.
pub fn too_broad(dir: &std::path::Path, home: &std::path::Path) -> bool {
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let (d, h) = (canon(dir), canon(home));
    d == h
        || h.starts_with(&d)
        || d.parent().is_none()
        || [
            "/usr", "/etc", "/var", "/opt", "/bin", "/lib", "/sbin", "/boot", "/sys", "/proc", "/dev",
            "/run", "/tmp",
        ]
        .iter()
        .any(|sys| d == std::path::Path::new(sys))
}

/// The user's Music and Videos folders as the system names them: the XDG user directories (`user-dirs.dirs`) on Linux, the Known Folders
/// on Windows, `~/Music` and `~/Movies` on macOS. Only those that exist, and never one that is too broad (see [`too_broad`]): then
/// `~/Videos` (or `~/Movies`) stands in when it exists, and the second value says what was left out.
pub fn standard_folders_checked() -> (Vec<StandardFolder>, Option<String>) {
    let Some(dirs) = directories::UserDirs::new() else { return (Vec::new(), None) };
    pick_standard_folders(dirs.home_dir(), dirs.audio_dir(), dirs.video_dir())
}

/// [`standard_folders_checked`] for the given home, Music and Videos folders (the system's answers), so it can be tried with any of them.
pub fn pick_standard_folders(
    home: &std::path::Path,
    audio: Option<&std::path::Path>,
    video: Option<&std::path::Path>,
) -> (Vec<StandardFolder>, Option<String>) {
    let mut out = Vec::new();
    let mut note = None;
    for (kind, dir, fallbacks, what) in [
        (StandardKind::Music, audio, &["Music"][..], "Music"),
        (StandardKind::Videos, video, &["Videos", "Movies"][..], "Videos"),
    ] {
        let Some(dir) = dir else { continue };
        let mut chosen = dir.to_path_buf();
        if too_broad(&chosen, home) {
            // The home folder (or something above it) named as the folder: use the usual place if there is one, else leave it out.
            match fallbacks.iter().map(|f| home.join(f)).find(|p| p.is_dir() && !too_broad(p, home)) {
                Some(p) => chosen = p,
                None => {
                    if dir.is_dir() {
                        note = Some(format!(
                            "Your {what} folder is set to your home folder \u{2014} add a folder instead."
                        ));
                    }
                    continue;
                }
            }
        }
        if !chosen.is_dir() {
            continue;
        }
        let name =
            chosen.file_name().map_or_else(|| "Folder".to_string(), |n| n.to_string_lossy().into_owned());
        out.push(StandardFolder { kind, name, path: chosen.to_string_lossy().into_owned() });
    }
    (out, note)
}

/// The user's Music and Videos folders (see [`standard_folders_checked`]).
pub fn standard_folders() -> Vec<StandardFolder> {
    standard_folders_checked().0
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
    fn can_quit(&self) -> bool {
        true
    }
    fn stable_ids(&self) -> bool {
        true
    }
}

/// Seconds local time is ahead of UTC at `t` (seconds since 1970); 0 when the system will not say.
#[cfg(unix)]
fn local_utc_offset(t: i64) -> i32 {
    // `struct tm` as glibc, musl and the BSDs lay it out: nine ints, then the offset and the zone name.
    #[repr(C)]
    struct Tm {
        ints: [i32; 9],
        gmtoff: i64,
        zone: *const u8,
    }
    unsafe extern "C" {
        fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    }
    let mut tm = Tm { ints: [0; 9], gmtoff: 0, zone: core::ptr::null() };
    // SAFETY: `t` and `tm` are valid for the call; `localtime_r` only writes the struct, which is at least as large as the C one.
    let ok = unsafe { !localtime_r(&t, &mut tm).is_null() };
    if ok { tm.gmtoff as i32 } else { 0 }
}

#[cfg(not(unix))]
fn local_utc_offset(_t: i64) -> i32 {
    0
}

#[cfg(test)]
mod standard_folder_tests {
    use super::*;

    fn home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rvp-std-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_videos_folder_that_is_the_home_folder_is_not_added() {
        let h = home("h1");
        std::fs::create_dir_all(h.join("Music")).unwrap();
        std::fs::create_dir_all(h.join("repos/big")).unwrap();
        // `XDG_VIDEOS_DIR="$HOME/"`: Videos is the home folder itself. There is no ~/Videos to stand in: it is left out, with a note.
        let (found, note) = pick_standard_folders(&h, Some(&h.join("Music")), Some(&h));
        assert_eq!(found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Music"]);
        assert!(
            note.as_deref().is_some_and(|n| n.contains("Videos folder is set to your home folder")),
            "{note:?}"
        );
        // With a ~/Videos the usual place stands in.
        std::fs::create_dir_all(h.join("Videos")).unwrap();
        let (found, note) = pick_standard_folders(&h, Some(&h.join("Music")), Some(&h));
        assert_eq!(found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Music", "Videos"]);
        assert!(note.is_none());
        // Trailing slash or not, and the root and the folder above home, are just as broad.
        assert!(too_broad(&PathBuf::from(format!("{}/", h.display())), &h));
        assert!(too_broad(std::path::Path::new("/"), &h));
        assert!(too_broad(h.parent().unwrap(), &h));
        assert!(!too_broad(&h.join("Videos"), &h));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_music_folder_is_valid() {
        let h = home("h2");
        let real = home("h2-real-music");
        std::os::unix::fs::symlink(&real, h.join("Music")).unwrap();
        let (found, note) = pick_standard_folders(&h, Some(&h.join("Music")), None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Music");
        assert!(note.is_none());
    }
}
