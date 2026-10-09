//! Native headless host for tests: a virtual-time clock, scripted input, a file `Source`, a null audio
//! sink that drains in virtual time, a frame-hashing video sink and an in-memory surface/storage.
//!
//! Nothing here touches wall-clock time: a two-hour movie can run in seconds and give identical output.
pub use rvp_host::mock::{FakeClock as VirtualClock, ScriptedInput};

use rvp_core::{AudioParams, Timestamp, VideoFrame};
use rvp_host::{
    AudioSink, FileEntry, FrameSink, Host, HostClock, HostError, InputEvents, Library, Listing, NowPlaying,
    OpenRequest, RecordingNowPlaying, RecordingTap, Rect, ScriptedLibrary, Source, Storage, Surface,
    VideoSink, VisualizerTap,
};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::rc::Rc;

/// Version string shown by the CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Walk the directory `dir` and describe every file below it as a [`Listing`] for root `root_id` (files sorted by path,
/// ids are absolute paths, which [`FileSource::open`] takes).
pub fn walk_listing(root_id: &str, name: &str, dir: &std::path::Path) -> Listing {
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<FileEntry>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                walk(base, &p, out);
            } else if md.is_file() {
                let rel = p
                    .strip_prefix(base)
                    .unwrap_or(&p)
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                let mtime_ms = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as i64);
                out.push(FileEntry {
                    id: p.to_string_lossy().into_owned(),
                    path: rel,
                    size: md.len(),
                    mtime_ms,
                });
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    Listing { root: root_id.to_string(), name: name.to_string(), files }
}

/// A [`Source`] over a native file.
pub struct FileSource {
    file: std::fs::File,
    len: u64,
    name: String,
}

impl FileSource {
    /// Open `path`.
    pub fn open(path: &str) -> Result<Self, HostError> {
        let file = std::fs::File::open(path).map_err(|e| HostError(format!("{path}: {e}")))?;
        let len = file.metadata().map_err(|e| HostError(e.to_string()))?.len();
        let name =
            std::path::Path::new(path).file_name().map_or(path.to_string(), |n| n.to_string_lossy().into());
        Ok(Self { file, len, name })
    }
}

impl Source for FileSource {
    async fn size(&self) -> Option<u64> {
        Some(self.len)
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, HostError> {
        self.file.seek(SeekFrom::Start(offset)).map_err(|e| HostError(e.to_string()))?;
        self.file.read(buf).map_err(|e| HostError(e.to_string()))
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Audio sink that "plays" in virtual time: queued frames drain at the sample rate while unpaused.
pub struct NullAudio {
    clock: Rc<VirtualClock>,
    params: Option<AudioParams>,
    queued: u64,
    written: u64,
    paused: bool,
    last_update: Timestamp,
    /// Device buffer delay reported to the player.
    pub latency_us: Timestamp,
    /// Last volume set.
    pub volume: f32,
    capacity_frames: u64,
    /// When set, every accepted sample is appended here (interleaved `f32`).
    pub capture: Option<Vec<f32>>,
    /// A failing output (see [`NullAudio::inject_failure`]): nothing drains, the queue and the position stand still.
    fail: Option<FailingOutput>,
    recovered: Option<String>,
    /// The (virtual) times the sink tried to get its output back, for tests of the backoff.
    pub retries: Vec<Timestamp>,
}

/// What a failing fake output remembers.
#[derive(Debug)]
struct FailingOutput {
    issue: rvp_host::AudioIssue,
    /// The virtual time from which a retry succeeds (`None`: never).
    recover_at: Option<Timestamp>,
    attempt: u32,
    next_try: Timestamp,
}

impl NullAudio {
    /// True while no failure is injected (or it has recovered).
    pub fn issue_is_none(&self) -> bool {
        self.fail.is_none()
    }

    /// Make the output fail as a busy, unplugged or broken device does: what is queued stops draining (so the audio clock and the position
    /// stand still), and the sink retries with the real backoff; the first retry at or after `recover_at` (virtual microseconds) works.
    pub fn inject_failure(&mut self, issue: rvp_host::AudioIssue, recover_at: Option<Timestamp>) {
        self.drain();
        let now = self.clock.now_us();
        self.fail = Some(FailingOutput {
            issue,
            recover_at,
            attempt: 0,
            next_try: now + rvp_host::retry_delay_us(0),
        });
    }

    fn new(clock: Rc<VirtualClock>) -> Self {
        Self {
            clock,
            params: None,
            queued: 0,
            written: 0,
            paused: true,
            last_update: 0,
            latency_us: 20_000,
            volume: 1.0,
            capacity_frames: 0,
            capture: None,
            fail: None,
            recovered: None,
            retries: Vec::new(),
        }
    }

    /// True while the device is paused (nothing queued plays).
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Total frames accepted since `open`.
    pub fn frames_written(&self) -> u64 {
        self.written
    }

    fn drain(&mut self) {
        let now = self.clock.now_us();
        if let (false, Some(p), None) = (self.paused, self.params, &self.fail) {
            let played = ((now - self.last_update) as u128 * p.sample_rate as u128 / 1_000_000) as u64;
            self.queued = self.queued.saturating_sub(played);
        }
        self.last_update = now;
    }
}

impl AudioSink for NullAudio {
    fn open(&mut self, want: AudioParams) -> Result<AudioParams, HostError> {
        // A new stream: nothing from an earlier one is left in the buffer (the drain is computed lazily, so the count of what
        // an earlier stream left there can be stale).
        self.queued = 0;
        self.written = 0;
        self.params = Some(want);
        self.capacity_frames = want.sample_rate as u64; // one second of buffer
        self.last_update = self.clock.now_us();
        Ok(want)
    }

    fn queued_frames(&self) -> usize {
        // Exact value as of "now" without needing `&mut`: recompute the drain.
        match (self.paused || self.fail.is_some(), self.params) {
            (false, Some(p)) => {
                let played = ((self.clock.now_us() - self.last_update) as u128 * p.sample_rate as u128
                    / 1_000_000) as u64;
                self.queued.saturating_sub(played) as usize
            }
            _ => self.queued as usize,
        }
    }

    fn output_latency_us(&self) -> Timestamp {
        self.latency_us
    }

    fn write(&mut self, interleaved: &[f32]) -> usize {
        self.drain();
        let Some(p) = self.params else { return 0 };
        let frames = interleaved.len() / p.channels.max(1) as usize;
        let room = self.capacity_frames.saturating_sub(self.queued) as usize;
        let n = frames.min(room);
        self.queued += n as u64;
        self.written += n as u64;
        if let Some(c) = &mut self.capture {
            c.extend_from_slice(&interleaved[..n * p.channels.max(1) as usize]);
        }
        n
    }

    fn flush(&mut self) {
        self.drain();
        self.queued = 0;
    }

    fn set_paused(&mut self, paused: bool) {
        self.drain();
        self.paused = paused;
        // Pressing play asks for the output again at once.
        if let (false, Some(f)) = (paused, &mut self.fail) {
            f.next_try = self.clock.now_us();
        }
    }

    fn issue(&self) -> Option<rvp_host::AudioIssue> {
        self.fail.as_ref().map(|f| f.issue.clone())
    }

    fn maintain(&mut self) {
        let now = self.clock.now_us();
        let Some(f) = &mut self.fail else { return };
        if now < f.next_try {
            return;
        }
        self.retries.push(now);
        if f.recover_at.is_some_and(|r| now >= r) {
            self.recovered = Some(f.issue.device.clone());
            self.fail = None;
            self.last_update = now;
        } else {
            f.attempt += 1;
            f.next_try = now + rvp_host::retry_delay_us(f.attempt);
        }
    }

    fn take_recovered(&mut self) -> Option<String> {
        self.recovered.take()
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
    }
}

/// Video sink that records a hash and the pts of every presented frame.
#[derive(Debug, Default)]
pub struct HashVideo {
    /// `(pts, FNV-1a hash of all planes)` per presented frame.
    pub frames: Vec<(Timestamp, u64)>,
}

impl VideoSink for HashVideo {
    fn present(&mut self, frame: &VideoFrame) {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for plane in &frame.planes {
            for &b in plane {
                h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
        self.frames.push((frame.pts, h));
    }
}

/// In-memory surface.
#[derive(Debug)]
pub struct MemSurface {
    /// Width, height, device pixel ratio.
    pub size: (u32, u32, f32),
    /// Last uploaded framebuffer.
    pub rgba: Vec<u8>,
    /// Fullscreen flag.
    pub fullscreen: bool,
}

impl Surface for MemSurface {
    fn size(&self) -> (u32, u32, f32) {
        self.size
    }

    fn present_rgba(&mut self, rgba: &[u8], _dirty: Rect) {
        self.rgba.clear();
        self.rgba.extend_from_slice(rgba);
    }

    fn set_fullscreen(&mut self, on: bool) {
        self.fullscreen = on;
    }
}

/// In-memory key-value storage.
#[derive(Debug, Default)]
pub struct MemStorage(pub HashMap<String, Vec<u8>>);

impl Storage for MemStorage {
    async fn load(&mut self, key: &str) -> Option<Vec<u8>> {
        self.0.get(key).cloned()
    }

    async fn store(&mut self, key: &str, value: &[u8]) {
        self.0.insert(key.to_string(), value.to_vec());
    }
}

/// The headless [`Host`].
pub struct HeadlessHost {
    clock: Rc<VirtualClock>,
    /// Audio sink.
    pub audio: NullAudio,
    /// Video sink.
    pub video: HashVideo,
    /// UI surface.
    pub surface: MemSurface,
    /// Scripted input.
    pub input: ScriptedInput,
    /// Storage.
    pub storage: MemStorage,
    /// When set, the host offers a visualizer tap and records what it receives.
    pub tap: Option<RecordingTap>,
    /// When set, the host offers directory access: push listings (see [`walk_listing`]) for the app to scan.
    pub library: Option<ScriptedLibrary>,
    /// When set, the host offers a now-playing sink and records what it receives.
    pub now_playing: Option<RecordingNowPlaying>,
}

impl Default for HeadlessHost {
    fn default() -> Self {
        Self::new()
    }
}

impl HeadlessHost {
    /// A host at virtual time 0 with a 1280x720 surface.
    pub fn new() -> Self {
        let clock = Rc::new(VirtualClock::new());
        Self {
            audio: NullAudio::new(clock.clone()),
            clock,
            video: HashVideo::default(),
            surface: MemSurface { size: (1280, 720, 1.0), rgba: Vec::new(), fullscreen: false },
            input: ScriptedInput::default(),
            storage: MemStorage::default(),
            tap: None,
            library: None,
            now_playing: None,
        }
    }

    /// Shared handle to the virtual clock (to advance time from a test driver).
    pub fn virtual_clock(&self) -> Rc<VirtualClock> {
        self.clock.clone()
    }
}

impl Host for HeadlessHost {
    type Source = FileSource;
    type Audio = NullAudio;
    type Video = HashVideo;
    type Store = MemStorage;

    fn clock(&self) -> &dyn HostClock {
        &*self.clock
    }
    fn audio(&mut self) -> &mut NullAudio {
        &mut self.audio
    }
    fn video(&mut self) -> &mut HashVideo {
        &mut self.video
    }
    fn surface(&mut self) -> &mut dyn Surface {
        &mut self.surface
    }
    fn input(&mut self) -> &mut dyn InputEvents {
        &mut self.input
    }
    fn storage(&mut self) -> &mut MemStorage {
        &mut self.storage
    }
    async fn open(&mut self, req: OpenRequest) -> Result<FileSource, HostError> {
        match req {
            OpenRequest::Id(path) => FileSource::open(&path),
            OpenRequest::Pick => Err(HostError("no file picker in the headless host".into())),
        }
    }
    fn now_playing(&mut self) -> Option<&mut dyn NowPlaying> {
        self.now_playing.as_mut().map(|n| n as &mut dyn NowPlaying)
    }
    fn visualizer(&mut self) -> Option<&mut dyn VisualizerTap> {
        self.tap.as_mut().map(|t| t as &mut dyn VisualizerTap)
    }
    fn library(&mut self) -> Option<&mut dyn Library> {
        self.library.as_mut().map(|l| l as &mut dyn Library)
    }
    fn stable_ids(&self) -> bool {
        true
    }
}

/// A headless host for the full application ([`rvp_app::App`]): like [`HeadlessHost`], but the video sink keeps
/// the latest picture as RGBA so the app can compose it with the UI, and the surface keeps what was presented.
pub struct UiHost {
    clock: Rc<VirtualClock>,
    /// Audio sink.
    pub audio: NullAudio,
    /// Video sink (the app reads the latest frame from it).
    pub video: FrameSink,
    /// Surface; `rgba` is the last presented frame.
    pub surface: MemSurface,
    /// Scripted input.
    pub input: ScriptedInput,
    /// Storage.
    pub storage: MemStorage,
    /// Frames presented to the surface.
    pub presents: u64,
    /// When set, the host offers a visualizer tap and records what it receives.
    pub tap: Option<RecordingTap>,
    /// When set, the host offers directory access: push listings (see [`walk_listing`]) for the app to scan.
    pub library: Option<ScriptedLibrary>,
    /// When set, the host offers a now-playing sink and records what it receives.
    pub now_playing: Option<RecordingNowPlaying>,
    /// What [`Host::stable_ids`] answers (true: ids are file paths; false acts like a browser's session-bound ids).
    pub stable: bool,
    /// When set, the host offers update and app-menu services (scripted by the test).
    pub services: Option<rvp_host::ScriptedServices>,
    /// When set, the host can fetch pages (scripted by the test).
    pub net: Option<rvp_host::ScriptedNet>,
    /// When set, the host can replace library files (a [`FsWriter`], or a scripted one).
    pub writer: Option<Box<dyn rvp_host::FileWriter>>,
    /// What [`Host::opens_links`] answers.
    pub links: bool,
    /// What [`Host::can_quit`] answers.
    pub quit: bool,
}

/// A [`rvp_host::FileWriter`] that really replaces files below the folders it was told about (root id -> directory), through a
/// temporary file and a rename, like the desktop's.
#[derive(Default)]
pub struct FsWriter {
    /// Root id -> directory.
    pub roots: HashMap<String, std::path::PathBuf>,
    answers: HashMap<u32, Result<(), String>>,
    next: u32,
}

impl rvp_host::FileWriter for FsWriter {
    fn can_write(&mut self, root: &str) -> Result<(), String> {
        if self.roots.contains_key(root) { Ok(()) } else { Err("unknown folder".into()) }
    }

    fn write(&mut self, root: &str, path: &str, data: Vec<u8>) -> u32 {
        let answer = (|| {
            let base = self.roots.get(root).ok_or("unknown folder")?;
            if path.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
                return Err("that path is not inside the folder".to_string());
            }
            let target = base.join(path);
            if !target.is_file() {
                return Err("the file is not there".to_string());
            }
            let tmp = target.with_extension("rvp-tmp");
            std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &target).map_err(|e| e.to_string())
        })();
        self.next += 1;
        self.answers.insert(self.next, answer);
        self.next
    }

    fn poll_write(&mut self, ticket: u32) -> Option<Result<(), String>> {
        self.answers.remove(&ticket)
    }
}

impl Default for UiHost {
    fn default() -> Self {
        Self::new()
    }
}

impl UiHost {
    /// A host at virtual time 0 with a 1280x720 surface.
    pub fn new() -> Self {
        let clock = Rc::new(VirtualClock::new());
        Self {
            audio: NullAudio::new(clock.clone()),
            clock,
            video: FrameSink::new(),
            surface: MemSurface { size: (1280, 720, 1.0), rgba: Vec::new(), fullscreen: false },
            input: ScriptedInput::default(),
            storage: MemStorage::default(),
            tap: None,
            library: None,
            now_playing: None,
            presents: 0,
            stable: true,
            services: None,
            net: None,
            writer: None,
            links: false,
            quit: false,
        }
    }

    /// Shared handle to the virtual clock.
    pub fn virtual_clock(&self) -> Rc<VirtualClock> {
        self.clock.clone()
    }
}

impl Host for UiHost {
    type Source = FileSource;
    type Audio = NullAudio;
    type Video = FrameSink;
    type Store = MemStorage;

    fn clock(&self) -> &dyn HostClock {
        &*self.clock
    }
    fn audio(&mut self) -> &mut NullAudio {
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
    fn storage(&mut self) -> &mut MemStorage {
        &mut self.storage
    }
    async fn open(&mut self, req: OpenRequest) -> Result<FileSource, HostError> {
        match req {
            OpenRequest::Id(path) => FileSource::open(&path),
            OpenRequest::Pick => Err(HostError("no file picker in the headless host".into())),
        }
    }
    fn now_playing(&mut self) -> Option<&mut dyn NowPlaying> {
        self.now_playing.as_mut().map(|n| n as &mut dyn NowPlaying)
    }
    fn visualizer(&mut self) -> Option<&mut dyn VisualizerTap> {
        self.tap.as_mut().map(|t| t as &mut dyn VisualizerTap)
    }
    fn library(&mut self) -> Option<&mut dyn Library> {
        self.library.as_mut().map(|l| l as &mut dyn Library)
    }
    fn net(&mut self) -> Option<&mut dyn rvp_host::Net> {
        self.net.as_mut().map(|n| n as &mut dyn rvp_host::Net)
    }
    fn app_services(&mut self) -> Option<&mut dyn rvp_host::AppServices> {
        self.services.as_mut().map(|s| s as &mut dyn rvp_host::AppServices)
    }
    fn file_writer(&mut self) -> Option<&mut dyn rvp_host::FileWriter> {
        match &mut self.writer {
            Some(w) => Some(w.as_mut()),
            None => None,
        }
    }
    fn opens_links(&self) -> bool {
        self.links
    }
    fn can_quit(&self) -> bool {
        self.quit
    }
    fn stable_ids(&self) -> bool {
        self.stable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvp_core::task::block_on;

    #[test]
    fn audio_drains_in_virtual_time() {
        let mut host = HeadlessHost::new();
        let p = host.audio.open(AudioParams { sample_rate: 48_000, channels: 2 }).unwrap();
        assert_eq!(p.sample_rate, 48_000);
        host.audio.set_paused(false);
        let accepted = host.audio.write(&vec![0.0; 2 * 24_000]); // 0.5 s
        assert_eq!(accepted, 24_000);
        host.virtual_clock().advance(250_000);
        assert_eq!(host.audio.queued_frames(), 12_000);
        host.virtual_clock().advance(1_000_000);
        assert_eq!(host.audio.queued_frames(), 0);
        // Pausing freezes the queue.
        host.audio.write(&vec![0.0; 2 * 1000]);
        host.audio.set_paused(true);
        host.virtual_clock().advance(5_000_000);
        assert_eq!(host.audio.queued_frames(), 1000);
    }

    #[test]
    fn audio_write_is_bounded_by_device_buffer() {
        let mut host = HeadlessHost::new();
        host.audio.open(AudioParams { sample_rate: 1000, channels: 1 }).unwrap();
        assert_eq!(host.audio.write(&vec![0.0; 5000]), 1000);
    }

    #[test]
    fn file_source_reads_and_host_open_errors() {
        let path = std::env::temp_dir().join(format!("rvp-headless-test-{}", std::process::id()));
        std::fs::write(&path, b"0123456789").unwrap();
        let mut host = HeadlessHost::new();
        block_on(async {
            let mut src = host.open(OpenRequest::Id(path.to_string_lossy().into())).await.unwrap();
            assert_eq!(src.size().await, Some(10));
            let mut buf = [0u8; 4];
            assert_eq!(src.read_at(3, &mut buf).await.unwrap(), 4);
            assert_eq!(&buf, b"3456");
            assert_eq!(src.read_at(10, &mut buf).await.unwrap(), 0);
            assert!(host.open(OpenRequest::Pick).await.is_err());
            host.storage().store("k", b"v").await;
            assert_eq!(host.storage().load("k").await.as_deref(), Some(&b"v"[..]));
        });
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn video_sink_hashes_deterministically() {
        use rvp_core::{ColorMatrix, ColorRange, PixelFormat};
        let frame = VideoFrame {
            width: 2,
            height: 2,
            format: PixelFormat::Yuv420p8,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            planes: [vec![1, 2, 3, 4], vec![5], vec![6]],
            strides: [2, 1, 1],
            pts: 40_000,
        };
        let mut a = HashVideo::default();
        let mut b = HashVideo::default();
        a.present(&frame);
        b.present(&frame);
        assert_eq!(a.frames, b.frames);
        assert_eq!(a.frames[0].0, 40_000);
    }
}

pub mod play;
pub use play::{DefaultCodecs, PlayOptions, PlayReport, play_file, write_wav_f32};
pub use rvp_player::TraceEntry as TraceEntryRef;
pub use rvp_player::{SessionEvent, SessionState, SubtitleTrack};
