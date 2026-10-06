//! A scripted, deterministic mock of the Rusty Bucket host: the `bucket_v0` imports implemented in plain Rust, on virtual time.
//!
//! Install a [`MockHost`] on the test's thread ([`MockHost::install`]) and run the code under test, which calls the imports of
//! `bucket-v0-sys` as it would in a `.bucket`. Time only moves when the code waits in `events_wait` (or the test calls
//! [`State::advance`]), so runs are repeatable and instant. The mock keeps a log of everything it was asked to do and lets the
//! test script what the host does: events, slow files (`-BUSY` then `IO_READY`), an audio device with a clock, a media library
//! with partial listings, a full key-value store, and functions an older host would answer with `-UNSUPPORTED`.
//!
//! It follows the draft v0.3 pages and is not a simulator: no sandbox, no real devices. Where the pages were unclear it reads them
//! as `bucket-sim` (rust-os `tools/bucket-sim`) does, which is what the v0.3 clarifications wrote down: argument errors of
//! `events_wait`, `restart` and `exit` that never return ([`AppEnded`], [`catch_end`]), the thread limit that counts the main thread,
//! a strict `canvas_present` (`-BUSY` for a pending `RESIZE` first), `-NO_SPACE` for counts and `-TOO_LARGE` for sizes, `file_read_at`
//! with `cap` 0, folder handles, `file_open_sibling`, a new stream for every `audio_open`, `now_playing_*` validation, the library walk
//! (the OS starts the first walk after `FOLDER_ADDED`; the first `LIBRARY_LISTING` replaces the kept one and the rest append; a rescan
//! of an unreadable root is `-IO`), cache-class eviction in the key-value store and fetches that fail (`IO_READY`, then `-IO`).
use bucket_v0_sys as sys;
use bucket_v0_sys::backend::{Guard, deref32};
use sys::{Backend, Event, err, ev};

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

pub mod events;

const BLOCK: u64 = 64 * 1024;

/// How the app ended itself. `restart()` and `exit()` do not return: the host unwinds the app, which here is a panic with this
/// payload that [`catch_end`] turns back into a value (and that does not run the panic hook).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEnded {
    /// `restart()` took effect.
    Restart,
    /// `exit(status)`.
    Exit(i32),
}

/// Run `f` (which may call `restart` or `exit`) and report how it ended: its value, or [`AppEnded`] when the mock unwound it. Any
/// other panic goes on.
pub fn catch_end<R>(f: impl FnOnce() -> R) -> Result<R, AppEnded> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => Ok(r),
        Err(p) => match p.downcast::<AppEnded>() {
            Ok(e) => Err(*e),
            Err(p) => std::panic::resume_unwind(p),
        },
    }
}

/// What `bucket_save_state` is: the buffer to fill, and the length it reports (negative = failure).
#[derive(Clone)]
pub struct SaveHook(pub Arc<SaveFn>);

/// The function of a [`SaveHook`].
pub type SaveFn = dyn Fn(&mut [u8]) -> i32 + Send + Sync;

impl std::fmt::Debug for SaveHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SaveHook")
    }
}

/// A file the host can hand out.
#[derive(Debug, Clone)]
pub struct FileSpec {
    /// File name.
    pub name: String,
    /// Content.
    pub data: Arc<Vec<u8>>,
    /// Stable id (`file_id`), or `None` for a file without one.
    pub id: Option<String>,
    /// Delay of a cold block, microseconds. 0 = always ready (never `-BUSY`).
    pub latency_us: i64,
    /// `file_size` answers `-UNSUPPORTED` (a stream of unknown length).
    pub unknown_size: bool,
    /// A folder handle (`file_pick` flag bit 1): `file_size` and `file_read_at` answer `-INVALID`, and `file_open_sibling` opens
    /// a file inside it.
    pub folder: bool,
    /// The folder this file is in (or, for a folder handle, its own key): the key of [`State::add_sibling`].
    pub dir: Option<String>,
    /// A cold block fails: `IO_READY` still arrives and the retry answers `-IO`.
    pub fail_io: bool,
}

impl FileSpec {
    /// A file that is always ready.
    pub fn new(name: &str, data: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            data: Arc::new(data),
            id: Some(format!("id:{name}")),
            latency_us: 0,
            unknown_size: false,
            folder: false,
            dir: None,
            fail_io: false,
        }
    }

    /// A folder handle with key `dir`.
    pub fn folder(name: &str, dir: &str) -> Self {
        Self {
            folder: true,
            dir: Some(dir.into()),
            id: Some(format!("id:{name}")),
            ..Self::new(name, Vec::new())
        }
    }

    /// Lives in the folder `dir` (see [`State::add_sibling`]).
    pub fn in_dir(mut self, dir: &str) -> Self {
        self.dir = Some(dir.into());
        self
    }

    /// Reads of cold blocks fail with `-IO` after `IO_READY` (needs [`FileSpec::slow`]).
    pub fn failing(mut self) -> Self {
        self.fail_io = true;
        self
    }

    /// Cold blocks take `latency_us` to arrive: the first read returns `-BUSY`, and `IO_READY` follows.
    pub fn slow(mut self, latency_us: i64) -> Self {
        self.latency_us = latency_us;
        self
    }

    /// No stable id.
    pub fn without_id(mut self) -> Self {
        self.id = None;
        self
    }
}

#[derive(Debug)]
struct OpenFile {
    spec: FileSpec,
    /// Block index to the time it is ready.
    blocks: HashMap<u64, i64>,
}

/// What the app told the shell about the item (`now_playing_metadata`), decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaRec {
    /// `struct_size` the app sent.
    pub struct_size: u32,
    /// Title.
    pub title: String,
    /// Artist.
    pub artist: String,
    /// Album.
    pub album: String,
    /// Art MIME type.
    pub art_mime: String,
    /// Art bytes.
    pub art: Vec<u8>,
    /// Duration, -1 if unknown.
    pub duration_us: i64,
    /// Has a picture.
    pub has_video: bool,
}

/// What the app told the shell about the transport (`now_playing_playback`).
pub type PlaybackRec = sys::NowPlayingPlaybackRaw;

/// A frame the app handed to `canvas_present`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentRec {
    /// Buffer length.
    pub len: usize,
    /// Dirty rectangle (x, y, w, h).
    pub rect: (i32, i32, i32, i32),
}

/// A frame the app handed to `video_present`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoRec {
    /// The 104-byte struct as sent (pointers are tokens).
    pub raw: sys::VideoFrameRaw,
    /// The Y plane's first row.
    pub y_first_row: Vec<u8>,
}

/// The canvas.
#[derive(Debug, Clone)]
pub struct Canvas {
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Scale.
    pub scale: f32,
    /// Fullscreen.
    pub fullscreen: bool,
    /// Visible.
    pub visible: bool,
    /// Refresh in millihertz.
    pub refresh_mhz: u32,
    /// A resize is pending: presents return `-BUSY`.
    pub stale: bool,
    /// Presents so far.
    pub presents: Vec<PresentRec>,
    /// The last frame, when `keep_frame` is set.
    pub last_frame: Vec<u8>,
    /// Keep a copy of the last frame.
    pub keep_frame: bool,
}

/// An audio stream and its device.
#[derive(Debug, Clone)]
pub struct AudioStream {
    /// Granted rate.
    pub rate: u32,
    /// Granted channels.
    pub channels: u32,
    /// Ring capacity in frames.
    pub capacity: u32,
    /// Hand-off to audible delay.
    pub latency_us: i64,
    /// Frames written since open or flush.
    pub written: u64,
    /// Frames handed to the device since open or flush (fractional: the device runs on virtual time).
    pub consumed: f64,
    /// Paused.
    pub paused: bool,
    /// Volume.
    pub volume: f32,
    /// Underruns since open or flush.
    pub underruns: u32,
    /// Everything written, interleaved (kept across flushes).
    pub samples: Vec<f32>,
    /// `audio_flush` calls.
    pub flushes: u32,
    last_sync_us: i64,
    starved: bool,
}

/// A library root.
#[derive(Debug, Clone)]
pub struct Root {
    /// Stable id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Readable now.
    pub readable: bool,
}

/// A kept listing: the bytes and how much of it the host shows (a partial listing grows).
#[derive(Debug, Clone, Default)]
pub struct KeptListing {
    /// All the record bytes.
    pub blob: Vec<u8>,
    /// What `library_listing` reports as the total so far.
    pub visible: usize,
}

/// A request for the shell to show a picker or a dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRec {
    /// Request id.
    pub request: i32,
    /// `kinds` string.
    pub kinds: String,
    /// Flags.
    pub flags: i32,
}

/// A save request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveRec {
    /// Request id.
    pub request: i32,
    /// Suggested name.
    pub name: String,
    /// Media type.
    pub mime: String,
    /// The bytes.
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Queued {
    /// Library listing: when this event is generated, the kept listing of the root shows this many bytes.
    reveal: Option<(String, usize)>,
    ev: Event,
    /// Text for the payload at this record offset (the handle is made when the event is delivered).
    text: Option<(usize, String)>,
}

/// Everything the mock knows and records. Tests read and change it through [`MockHost::with`].
#[derive(Debug)]
pub struct State {
    /// Virtual time, microseconds.
    pub now_us: i64,
    /// `caps()`.
    pub caps: i64,
    /// `launch_reason()`.
    pub launch_reason: i32,
    /// `api_version()`.
    pub api_version: i32,
    /// `cpu_count()`.
    pub cpu_count: i32,
    /// `limit_get` answers (the documented minimums where absent).
    pub limits: HashMap<i32, i64>,
    /// Log lines (level, text).
    pub logs: Vec<(i32, String)>,
    /// Functions that answer `-UNSUPPORTED`, as on an older host.
    pub unsupported: HashSet<&'static str>,
    /// The `timeout_us` of every `events_wait`.
    pub waits: Vec<i64>,
    /// The canvas.
    pub canvas: Canvas,
    /// `cursor_set` calls.
    pub cursors: Vec<i32>,
    /// `canvas_fullscreen` calls.
    pub fullscreen_calls: Vec<bool>,
    /// `power_inhibit` calls (kind, on).
    pub power: Vec<(i32, bool)>,
    /// The key-value store: key to (value, flags).
    pub kv: BTreeMap<String, (Vec<u8>, i32)>,
    /// Keys whose next `kv_load` is `-BUSY` (an `IO_READY { 0 }` follows after `kv_latency_us`).
    pub kv_cold: HashSet<String>,
    /// Keys whose cold fetch fails: `IO_READY { 0 }` arrives and the retry answers `-IO` (a key must also be in `kv_cold`).
    pub kv_fail: HashSet<String>,
    kv_failed: HashSet<String>,
    kv_age: HashMap<String, u64>,
    kv_clock: u64,
    /// Delay of a cold key.
    pub kv_latency_us: i64,
    /// Total bytes the store accepts (`-NO_SPACE` beyond), if limited.
    pub kv_quota: Option<usize>,
    /// Every `kv_store` (key, length, flags).
    pub kv_stores: Vec<(String, usize, i32)>,
    /// `kv_flush` calls.
    pub kv_flushes: u32,
    /// Ids the host can reopen.
    pub known_files: HashMap<String, FileSpec>,
    /// Ids whose next `file_open_id` is `-BUSY`.
    pub open_cold: HashSet<String>,
    /// Ids whose cold open fails: `IO_READY { 0 }`, then `-IO` (the id must also be in `open_cold`).
    pub open_fail: HashSet<String>,
    open_failed: HashSet<String>,
    /// Files by folder key and name, for `file_open_sibling`.
    pub siblings: HashMap<String, HashMap<String, FileSpec>>,
    open_files: HashMap<i32, OpenFile>,
    /// Handles closed by the app, in order.
    pub closed: Vec<i32>,
    /// `file_prefetch` calls (handle, offset, length).
    pub prefetches: Vec<(i32, i64, i32)>,
    next_handle: i32,
    /// The audio streams.
    pub audio: BTreeMap<i32, AudioStream>,
    /// Device latency for new streams.
    pub audio_latency_us: i64,
    /// Rate granted instead of the requested one, if set (always, whatever was asked).
    pub audio_grant_rate: Option<u32>,
    /// The device's own rate: 44.1, 48, 88.2 and 96 kHz are granted as asked, any other request gets this.
    pub audio_device_rate: u32,
    /// Ring capacity of new streams, in seconds at the granted rate.
    pub audio_capacity_secs: u32,
    /// Streams the app closed.
    pub audio_closed: Vec<i32>,
    /// `now_playing_metadata` calls, decoded.
    pub metadata: Vec<MetaRec>,
    /// `now_playing_playback` calls.
    pub playback: Vec<PlaybackRec>,
    /// `now_playing_clear` calls.
    pub cleared: u32,
    /// `now_playing_playback` calls that repeated the last report (same state, rate and flags, the position where the shell would
    /// extrapolate it): the Simulator warns about them.
    pub needless_playback: u32,
    playback_fresh: bool,
    /// `now_playing_metadata` strings that were cut to the limit.
    pub truncated_strings: u32,
    /// `viz_block` calls (pts, rate, channels, frames).
    pub viz_blocks: Vec<(i64, i32, i32, i32)>,
    /// Summaries received, in order.
    pub viz_summaries: Vec<sys::VizSummaryRaw>,
    /// `viz_summary` calls.
    pub viz_single_calls: u32,
    /// `viz_summary_n` calls.
    pub viz_batch_calls: u32,
    /// `video_present` calls.
    pub videos: Vec<VideoRec>,
    /// Library roots.
    pub roots: Vec<Root>,
    /// Kept listings by root.
    pub listings: HashMap<String, KeptListing>,
    /// `library_listing_release` calls.
    pub releases: Vec<String>,
    /// `library_rescan` calls that started a walk (not the ones that found one running).
    pub rescans: Vec<String>,
    /// Roots whose walk has not delivered its last `LIBRARY_LISTING` yet.
    pub walking: HashSet<String>,
    /// `library_forget` calls.
    pub forgets: Vec<String>,
    /// `library_reconnect` calls.
    pub reconnects: Vec<String>,
    /// Request ids handed out by `library_add_folder`.
    pub folder_requests: Vec<i32>,
    /// `file_pick` calls.
    pub picks: Vec<PickRec>,
    /// `file_save` calls.
    pub saves: Vec<SaveRec>,
    next_request: i32,
    /// Arguments of `thread_spawn` calls.
    pub spawns: Vec<i32>,
    /// The most live threads, **counting the main thread** (default `limit_get(THREADS)`): spawning at the limit is `-NO_SPACE`,
    /// so with 64 the 63rd spawn works and the 64th fails. Thread ids count up from 1 and are never reused.
    pub thread_limit: Option<usize>,
    /// `thread_priority` calls (tid, level).
    pub priorities: Vec<(i32, i32)>,
    /// `restart()` calls that took effect.
    pub restarts: u32,
    /// `restart()` fails with this error instead of ending the app (`-UNSUPPORTED` on an OS that cannot).
    pub restart_error: Option<i32>,
    /// `bucket_save_state`, run in the `events_wait` after the app has read `RELOAD`.
    pub save_hook: Option<SaveHook>,
    /// Size of the buffer the host adds for the save (the real one is up to 16 MiB).
    pub save_buffer: usize,
    /// What each save returned.
    pub saves_done: Vec<i32>,
    /// The bytes of the last successful save.
    pub saved_state: Vec<u8>,
    reload_armed: bool,
    /// `exit(status)` calls.
    pub exits: Vec<i32>,
    /// `events_wake()` calls.
    pub wakes: u32,
    /// `frame_request` and `pointer_capture` calls.
    pub misc: Vec<String>,
    queue: VecDeque<Queued>,
    scheduled: Vec<(i64, Queued)>,
    texts: Vec<String>,
}

impl State {
    fn new() -> Self {
        Self {
            now_us: 1_000_000,
            caps: sys::caps::CANVAS
                | sys::caps::AUDIO_OUT
                | sys::caps::NOW_PLAYING
                | sys::caps::LIBRARY
                | sys::caps::VIDEO_YUV,
            launch_reason: 0,
            api_version: sys::API_MINOR,
            cpu_count: 4,
            limits: HashMap::new(),
            logs: Vec::new(),
            unsupported: HashSet::new(),
            waits: Vec::new(),
            canvas: Canvas {
                width: 640,
                height: 360,
                scale: 1.0,
                fullscreen: false,
                visible: true,
                refresh_mhz: 60_000,
                stale: false,
                presents: Vec::new(),
                last_frame: Vec::new(),
                keep_frame: false,
            },
            cursors: Vec::new(),
            fullscreen_calls: Vec::new(),
            power: Vec::new(),
            kv: BTreeMap::new(),
            kv_cold: HashSet::new(),
            kv_fail: HashSet::new(),
            kv_failed: HashSet::new(),
            kv_age: HashMap::new(),
            kv_clock: 0,
            kv_latency_us: 2_000,
            kv_quota: None,
            kv_stores: Vec::new(),
            kv_flushes: 0,
            known_files: HashMap::new(),
            open_cold: HashSet::new(),
            open_fail: HashSet::new(),
            open_failed: HashSet::new(),
            siblings: HashMap::new(),
            open_files: HashMap::new(),
            closed: Vec::new(),
            prefetches: Vec::new(),
            next_handle: 0,
            audio: BTreeMap::new(),
            audio_latency_us: 40_000,
            audio_grant_rate: None,
            audio_device_rate: 48_000,
            audio_capacity_secs: 1,
            audio_closed: Vec::new(),
            metadata: Vec::new(),
            playback: Vec::new(),
            cleared: 0,
            needless_playback: 0,
            truncated_strings: 0,
            viz_blocks: Vec::new(),
            viz_summaries: Vec::new(),
            viz_single_calls: 0,
            viz_batch_calls: 0,
            videos: Vec::new(),
            roots: Vec::new(),
            listings: HashMap::new(),
            releases: Vec::new(),
            rescans: Vec::new(),
            walking: HashSet::new(),
            forgets: Vec::new(),
            reconnects: Vec::new(),
            folder_requests: Vec::new(),
            picks: Vec::new(),
            saves: Vec::new(),
            next_request: 100,
            spawns: Vec::new(),
            thread_limit: None,
            priorities: Vec::new(),
            restarts: 0,
            restart_error: None,
            save_hook: None,
            save_buffer: 1 << 20,
            playback_fresh: false,
            saves_done: Vec::new(),
            saved_state: Vec::new(),
            reload_armed: false,
            exits: Vec::new(),
            wakes: 0,
            misc: Vec::new(),
            queue: VecDeque::new(),
            scheduled: Vec::new(),
            texts: Vec::new(),
        }
    }

    fn off(&self, name: &str) -> bool {
        self.unsupported.contains(name)
    }

    fn limit(&self, which: i32) -> i64 {
        self.limits.get(&which).copied().unwrap_or(match which {
            sys::limit::KEY_LEN => 255,
            sys::limit::VALUE_SIZE => 64 << 20,
            sys::limit::QUOTA => 512 << 20,
            sys::limit::HANDLES => 256,
            sys::limit::EVENTS_PER_WAIT => 256,
            sys::limit::READ_PER_CALL => 1 << 20,
            sys::limit::ART_BYTES => 8 << 20,
            sys::limit::STRING_BYTES => 4096,
            sys::limit::THREADS => 64,
            _ => err::INVALID as i64,
        })
    }

    /// Queue an event for the next `events_wait` (its time is now).
    pub fn push(&mut self, mut e: Event) {
        e.time_us = self.now_us;
        self.queue.push_back(Queued { ev: e, text: None, reveal: None });
    }

    /// Queue an event whose payload at `offset` is a text handle for `text`.
    pub fn push_text(&mut self, mut e: Event, offset: usize, text: &str) {
        e.time_us = self.now_us;
        self.queue.push_back(Queued { ev: e, text: Some((offset, text.into())), reveal: None });
    }

    /// Deliver an event `delay_us` from now (it arrives during a wait that is long enough).
    pub fn push_after(&mut self, delay_us: i64, mut e: Event) {
        let at = self.now_us + delay_us;
        e.time_us = at;
        self.scheduled.push((at, Queued { ev: e, text: None, reveal: None }));
    }

    /// Events not yet taken by the app.
    pub fn pending_events(&self) -> usize {
        self.queue.len() + self.scheduled.len()
    }

    /// Move virtual time forward; scheduled events that fall due are queued.
    pub fn advance(&mut self, us: i64) {
        self.set_time(self.now_us + us.max(0));
    }

    fn set_time(&mut self, t: i64) {
        self.now_us = t.max(self.now_us);
        let now = self.now_us;
        let mut due: Vec<(i64, Queued)> = Vec::new();
        self.scheduled.retain(|(at, q)| {
            if *at <= now {
                due.push((*at, q.clone()));
                false
            } else {
                true
            }
        });
        due.sort_by_key(|(at, _)| *at);
        for (_, q) in &due {
            if let Some((root, upto)) = &q.reveal {
                let l = self.listings.entry(root.clone()).or_default();
                l.visible = (*upto).min(l.blob.len());
            }
        }
        self.queue.extend(due.into_iter().map(|(_, q)| q));
        self.sync_audio();
    }

    fn sync_audio(&mut self) {
        let now = self.now_us;
        for a in self.audio.values_mut() {
            let dt = (now - a.last_sync_us).max(0) as f64;
            a.last_sync_us = now;
            if a.paused {
                continue;
            }
            let room = a.written as f64 - a.consumed;
            let want = dt * f64::from(a.rate) / 1e6;
            if want >= room && a.written > 0 && room >= 0.0 {
                // The ring ran dry while playing: it is a gap (an underrun) only if the app then writes more.
                if want > room {
                    a.starved = true;
                }
                a.consumed = a.written as f64;
            } else {
                a.consumed += want;
            }
        }
    }

    // ---- scripting helpers ----

    /// Register a file the app can reopen by id (and that [`State::new_handle`] can hand out).
    pub fn add_file(&mut self, spec: FileSpec) {
        if let Some(id) = &spec.id {
            self.known_files.insert(id.clone(), spec);
        }
    }

    /// Open a handle on `spec` as the shell does when it puts a file in a `DROP`, `OPEN` or `FILE_PICKED` event.
    pub fn new_handle(&mut self, spec: FileSpec) -> i32 {
        self.next_handle += 1;
        let h = self.next_handle;
        self.open_files.insert(h, OpenFile { spec, blocks: HashMap::new() });
        h
    }

    /// Handles still open.
    pub fn open_handles(&self) -> Vec<i32> {
        let mut v: Vec<i32> = self.open_files.keys().copied().collect();
        v.sort_unstable();
        v
    }

    /// Register `spec` as a file in the folder `dir` (what `file_open_sibling` finds by name).
    pub fn add_sibling(&mut self, dir: &str, spec: FileSpec) {
        self.siblings.entry(dir.into()).or_default().insert(spec.name.clone(), spec);
    }

    /// The OS changed the canvas: `RESIZE` is queued and presents answer `-BUSY` until the app has been handed it.
    pub fn resize(&mut self, w: u32, h: u32, scale: f32, fullscreen: bool) {
        self.canvas.width = w;
        self.canvas.height = h;
        self.canvas.scale = scale;
        self.canvas.fullscreen = fullscreen;
        self.canvas.stale = true;
        self.push(events::resize(w, h, scale, fullscreen));
    }

    /// Ask for a hot reload: `RELOAD` is queued; `save_hook` runs in the `events_wait` after the one that hands it out.
    pub fn request_reload(&mut self) {
        self.push(events::bare(ev::RELOAD));
    }

    /// A walk of `root` as the OS does it: the first `LIBRARY_LISTING` **replaces** the kept listing, later ones append, 200 files
    /// per event, the last without the partial flag; one event every `gap_us`.
    pub fn walk(&mut self, root: &str, files: &[(&str, &str, u64, i64)], gap_us: i64) {
        let chunks: Vec<&[(&str, &str, u64, i64)]> =
            if files.is_empty() { vec![&[][..]] } else { files.chunks(200).collect() };
        let blob = encode_listing(files);
        // Where each chunk ends in the blob.
        let mut ends = Vec::new();
        let mut at = 0;
        for c in &chunks {
            at += encode_listing(c).len();
            ends.push(at);
        }
        self.listings.insert(root.into(), KeptListing { blob, visible: 0 });
        self.walking.insert(root.into());
        let n = chunks.len();
        for (i, end) in ends.into_iter().enumerate() {
            let mut e = events::library(ev::LIBRARY_LISTING, i + 1 < n);
            let at = self.now_us + gap_us * (i as i64 + 1);
            e.time_us = at;
            self.scheduled.push((
                at,
                Queued { ev: e, text: Some((16, root.into())), reveal: Some((root.into(), end)) },
            ));
        }
    }

    /// The user picked a folder: `FOLDER_ADDED { request, root }` and, as the OS does by itself, the first walk of it.
    pub fn answer_folder(&mut self, request: i32, id: &str, name: &str, files: &[(&str, &str, u64, i64)]) {
        self.add_root(id, name, true);
        self.push_text(events::folder_added(request), 20, id);
        self.walk(id, files, 1_000);
    }

    /// Add a library root.
    pub fn add_root(&mut self, id: &str, name: &str, readable: bool) {
        self.roots.push(Root { id: id.into(), name: name.into(), readable });
    }

    /// Files the app was launched with: `OPEN` events (one group, bit 1 on all but the last) queued only on a normal launch
    /// (`launch_reason` 0); after a hot reload or a restart the OS sends none.
    pub fn launch_opens(&mut self, handles: &[i32]) {
        if self.launch_reason != 0 {
            return;
        }
        for (i, h) in handles.iter().enumerate() {
            self.push(events::open(*h, false, i + 1 < handles.len()));
        }
    }

    /// What the OS does for the library at launch: a `LIBRARY_LISTING` per readable root from the last scan, and for a root whose
    /// first walk never finished (the kept listing is not all visible) a new walk: `LIBRARY_PROGRESS`, then the full listing.
    pub fn launch_library(&mut self, gap_us: i64) {
        let roots: Vec<String> = self.roots.iter().filter(|r| r.readable).map(|r| r.id.clone()).collect();
        for root in roots {
            let l = self.listings.entry(root.clone()).or_default();
            let (unfinished, len) = (l.visible < l.blob.len(), l.blob.len());
            if unfinished && !self.walking.contains(&root) {
                self.walking.insert(root.clone());
                let at = self.now_us + gap_us;
                let mut p = events::library(ev::LIBRARY_PROGRESS, false);
                p.time_us = at;
                self.scheduled.push((at, Queued { ev: p, text: Some((16, root.clone())), reveal: None }));
                let at = at + gap_us;
                let mut e = events::library(ev::LIBRARY_LISTING, false);
                e.time_us = at;
                self.scheduled
                    .push((at, Queued { ev: e, text: Some((16, root.clone())), reveal: Some((root, len)) }));
            } else if !unfinished {
                self.push_text(events::library(ev::LIBRARY_LISTING, false), 16, &root);
            }
        }
    }

    /// Set the kept listing of `root` to these files (id, path, size, mtime_ms), all visible.
    pub fn set_listing(&mut self, root: &str, files: &[(&str, &str, u64, i64)]) {
        let blob = encode_listing(files);
        let visible = blob.len();
        self.listings.insert(root.into(), KeptListing { blob, visible });
    }

    /// Show more of the kept listing (a partial listing growing); returns the visible size.
    pub fn reveal(&mut self, root: &str, upto: usize) -> usize {
        let l = self.listings.entry(root.into()).or_default();
        l.visible = upto.min(l.blob.len());
        l.visible
    }
}

/// The listing records as the API pages lay them out: `size u64`, `mtime_ms i64`, `id_len u32`, `path_len u32`, the id and the
/// path bytes, then zero padding to a multiple of 8.
pub fn encode_listing(files: &[(&str, &str, u64, i64)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (id, path, size, mtime) in files {
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&mtime.to_le_bytes());
        out.extend_from_slice(&(id.len() as u32).to_le_bytes());
        out.extend_from_slice(&(path.len() as u32).to_le_bytes());
        out.extend_from_slice(id.as_bytes());
        out.extend_from_slice(path.as_bytes());
        while out.len() % 8 != 0 {
            out.push(0);
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

/// The mock host. Cheap to share: it is an `Arc` with one lock.
pub struct MockHost {
    state: Mutex<State>,
}

impl MockHost {
    /// A host with a 640x360 canvas, audio, now-playing and a library, at the documented minimum limits.
    pub fn new() -> Arc<Self> {
        Arc::new(Self { state: Mutex::new(State::new()) })
    }

    /// Make this host answer the imports on the current thread, until the guard drops.
    pub fn install(self: &Arc<Self>) -> Guard {
        sys::backend::install(self.clone())
    }

    /// Read or change the state.
    pub fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.lock())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ---- reading app memory ----

/// # Safety
/// `p` must point to `len` readable bytes (or `len <= 0`).
unsafe fn bytes<'a>(p: *const u8, len: i32) -> &'a [u8] {
    if len <= 0 || p.is_null() {
        &[]
    } else {
        // SAFETY: the caller guarantees the range.
        unsafe { std::slice::from_raw_parts(p, len as usize) }
    }
}

/// # Safety
/// `p` must point to `cap` writable bytes (or `cap <= 0`).
unsafe fn out_buf<'a>(p: *mut u8, cap: i32) -> &'a mut [u8] {
    if cap <= 0 || p.is_null() {
        &mut []
    } else {
        // SAFETY: the caller guarantees the range.
        unsafe { std::slice::from_raw_parts_mut(p, cap as usize) }
    }
}

/// # Safety
/// As [`bytes`].
unsafe fn text(p: *const u8, len: i32) -> String {
    // SAFETY: forwarded.
    String::from_utf8_lossy(unsafe { bytes(p, len) }).into_owned()
}

fn copy_out(dst: &mut [u8], src: &[u8]) -> i32 {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    src.len() as i32
}

fn rd32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn rd64(b: &[u8], at: usize) -> i64 {
    i64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]))
}

/// A string field of an app struct: (pointer token, length) at `at`.
fn field_bytes(b: &[u8], at: usize) -> Vec<u8> {
    let (ptr, len) = (rd32(b, at), rd32(b, at + 4));
    if len == 0 {
        return Vec::new();
    }
    // SAFETY: the app wrote a pointer made by `ptr32` and its length for this call.
    unsafe { bytes(deref32(ptr), len as i32) }.to_vec()
}

impl Backend for MockHost {
    unsafe fn api_version(&self) -> i32 {
        self.lock().api_version
    }
    unsafe fn caps(&self) -> i64 {
        self.lock().caps
    }
    unsafe fn launch_reason(&self) -> i32 {
        self.lock().launch_reason
    }
    unsafe fn restart(&self) -> i32 {
        let mut s = self.lock();
        if s.off("restart") {
            return err::UNSUPPORTED;
        }
        if let Some(e) = s.restart_error {
            return e;
        }
        s.restarts += 1;
        drop(s);
        // It does not return: the app ends at once.
        std::panic::resume_unwind(Box::new(AppEnded::Restart))
    }
    unsafe fn exit(&self, status: i32) {
        self.lock().exits.push(status);
        std::panic::resume_unwind(Box::new(AppEnded::Exit(status)))
    }
    unsafe fn log(&self, level: i32, ptr: *const u8, len: i32) {
        // SAFETY: the app's range.
        let t = unsafe { text(ptr, len) };
        self.lock().logs.push((level, t));
    }
    unsafe fn random_fill(&self, ptr: *mut u8, len: i32) -> i32 {
        // SAFETY: the app's range.
        let buf = unsafe { out_buf(ptr, len) };
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(31).wrapping_add(7);
        }
        0
    }
    unsafe fn time_now_us(&self) -> i64 {
        self.lock().now_us
    }
    unsafe fn cpu_count(&self) -> i32 {
        self.lock().cpu_count
    }
    unsafe fn limit_get(&self, which: i32) -> i64 {
        self.lock().limit(which)
    }
    unsafe fn power_inhibit(&self, kind: i32, on: i32) -> i32 {
        self.lock().power.push((kind, on != 0));
        0
    }

    unsafe fn thread_spawn(&self, arg: i32) -> i32 {
        let mut s = self.lock();
        // The limit counts the main thread.
        let limit = s.thread_limit.unwrap_or(s.limit(sys::limit::THREADS).max(1) as usize);
        if 1 + s.spawns.len() >= limit {
            return err::NO_SPACE;
        }
        s.spawns.push(arg);
        s.spawns.len() as i32
    }
    unsafe fn thread_yield(&self) {}
    unsafe fn thread_priority(&self, tid: i32, level: i32) -> i32 {
        let mut s = self.lock();
        if !(0..=1).contains(&level) {
            return err::INVALID;
        }
        if tid < 0 || tid as usize > s.spawns.len() {
            return err::NOT_FOUND;
        }
        s.priorities.push((tid, level));
        0
    }

    unsafe fn events_wait(&self, buf: *mut u8, max: i32, timeout_us: i64) -> i32 {
        if max < 1 || timeout_us < -1 {
            return err::INVALID;
        }
        let mut s = self.lock();
        // A hot reload: the app has read `RELOAD`, so its loop is parked here and the state is saved now (`bucket_save_state`).
        if s.reload_armed {
            s.reload_armed = false;
            if let Some(hook) = s.save_hook.clone() {
                let cap = s.save_buffer;
                drop(s);
                let mut buf = vec![0u8; cap];
                let n = (hook.0)(&mut buf);
                s = self.lock();
                s.saves_done.push(n);
                // 0 is "nothing to save" and is not logged; a negative value is.
                if n < 0 {
                    s.logs.push((2, format!("bucket_save_state failed ({n}); the app starts without state")));
                }
                if n > 0 {
                    buf.truncate((n as usize).min(cap));
                    s.saved_state = buf;
                }
            }
        }
        s.waits.push(timeout_us);
        s.texts.clear();
        // Bring due events in; if there is nothing and the caller may sleep, sleep until the next scheduled event or the timeout.
        let now = s.now_us;
        s.set_time(now);
        if s.queue.is_empty() && timeout_us != 0 {
            let limit = if timeout_us < 0 { i64::MAX } else { now + timeout_us };
            let next = s.scheduled.iter().map(|(at, _)| *at).min();
            match next {
                Some(at) if at <= limit => s.set_time(at),
                _ => s.set_time(if timeout_us < 0 { now + 1_000_000 } else { limit }),
            }
        }
        // SAFETY: the app's range: `max` records of 64 bytes.
        let out = unsafe { out_buf(buf, max.saturating_mul(64)) };
        let cap =
            (max.max(0) as usize).min(out.len() / 64).min(s.limit(sys::limit::EVENTS_PER_WAIT) as usize);
        let mut n = 0;
        while n < cap {
            let Some(mut q) = s.queue.pop_front() else { break };
            if let Some((off, t)) = q.text.take() {
                s.texts.push(t);
                let handle = s.texts.len() as i32;
                q.ev.put_i32(off, handle);
            }
            out[n * 64..n * 64 + 64].copy_from_slice(&q.ev.to_bytes());
            n += 1;
            match q.ev.kind {
                // The size is no longer stale once the app has been handed the `RESIZE`.
                ev::RESIZE => s.canvas.stale = false,
                ev::RELOAD => s.reload_armed = true,
                ev::LIBRARY_LISTING if q.ev.flags & ev::FLAG_MORE == 0 => {
                    if let Some((root, _)) = &q.reveal {
                        s.walking.remove(root);
                    }
                }
                _ => {}
            }
        }
        n as i32
    }
    unsafe fn events_wake(&self) -> i32 {
        let mut s = self.lock();
        s.wakes += 1;
        if !s.queue.iter().any(|q| q.ev.kind == ev::WAKE) {
            s.push(Event::new(ev::WAKE, 0, 0));
        }
        0
    }
    unsafe fn event_text(&self, handle: i32, buf: *mut u8, cap: i32) -> i32 {
        let s = self.lock();
        let Some(t) = handle.checked_sub(1).and_then(|i| s.texts.get(i as usize)) else {
            return err::NOT_FOUND;
        };
        // SAFETY: the app's range.
        copy_out(unsafe { out_buf(buf, cap) }, t.as_bytes())
    }

    unsafe fn canvas_info(&self, out: *mut u8) -> i32 {
        let s = self.lock();
        let c = &s.canvas;
        let info = sys::CanvasInfo {
            struct_size: 32,
            width: c.width,
            height: c.height,
            scale: c.scale,
            format: 1,
            refresh_mhz: c.refresh_mhz,
            flags: (c.fullscreen as u32) | ((c.visible as u32) << 1),
            reserved: 0,
        };
        // SAFETY: the app passes room for the struct.
        unsafe { std::ptr::copy_nonoverlapping(&info as *const sys::CanvasInfo as *const u8, out, 32) };
        0
    }
    unsafe fn canvas_present(&self, rgba: *const u8, len: i32, x: i32, y: i32, w: i32, h: i32) -> i32 {
        let mut s = self.lock();
        let c = &s.canvas;
        // A pending `RESIZE` is checked first; then the call is strict about its arguments.
        if c.stale {
            return err::BUSY;
        }
        if len as i64 != i64::from(c.width) * i64::from(c.height) * 4 {
            return err::INVALID;
        }
        if x < 0
            || y < 0
            || w < 0
            || h < 0
            || i64::from(x) + i64::from(w) > i64::from(c.width)
            || i64::from(y) + i64::from(h) > i64::from(c.height)
        {
            return err::INVALID;
        }
        if s.canvas.keep_frame {
            // SAFETY: the app's range.
            s.canvas.last_frame = unsafe { bytes(rgba, len) }.to_vec();
        }
        s.canvas.presents.push(PresentRec { len: len as usize, rect: (x, y, w, h) });
        0
    }
    unsafe fn canvas_fullscreen(&self, on: i32) -> i32 {
        self.lock().fullscreen_calls.push(on != 0);
        0
    }
    unsafe fn frame_request(&self, on: i32) -> i32 {
        self.lock().misc.push(format!("frame_request({on})"));
        err::UNSUPPORTED
    }
    unsafe fn cursor_set(&self, kind: i32) -> i32 {
        self.lock().cursors.push(kind);
        0
    }
    unsafe fn pointer_capture(&self, on: i32) -> i32 {
        self.lock().misc.push(format!("pointer_capture({on})"));
        0
    }

    unsafe fn video_present(&self, frame: *const u8) -> i32 {
        let mut s = self.lock();
        if s.caps & sys::caps::VIDEO_YUV == 0 {
            return err::UNSUPPORTED;
        }
        // SAFETY: the app passes the 104-byte struct.
        let raw = unsafe { std::ptr::read_unaligned(frame as *const sys::VideoFrameRaw) };
        let y = deref32(raw.planes[0]);
        let first = if y.is_null() || raw.dest == [0; 4] {
            Vec::new()
        } else {
            // SAFETY: the plane is valid for this call; the first row is within it.
            unsafe { bytes(y, raw.width as i32) }.to_vec()
        };
        s.videos.push(VideoRec { raw, y_first_row: first });
        0
    }

    unsafe fn audio_open(&self, rate: i32, channels: i32, out: *mut u8) -> i32 {
        let mut s = self.lock();
        if s.off("audio_open") || s.caps & sys::caps::AUDIO_OUT == 0 {
            return err::UNSUPPORTED;
        }
        if !(1..=2).contains(&channels) || rate <= 0 {
            return err::INVALID;
        }
        let rate = s.audio_grant_rate.unwrap_or(match rate {
            44_100 | 48_000 | 88_200 | 96_000 => rate as u32,
            _ => s.audio_device_rate,
        });
        let capacity = rate * s.audio_capacity_secs;
        s.next_handle += 1;
        let h = s.next_handle;
        let (latency_us, now) = (s.audio_latency_us, s.now_us);
        s.audio.insert(
            h,
            AudioStream {
                rate,
                channels: channels as u32,
                capacity,
                latency_us,
                written: 0,
                consumed: 0.0,
                paused: false,
                volume: 1.0,
                underruns: 0,
                samples: Vec::new(),
                flushes: 0,
                last_sync_us: now,
                starved: false,
            },
        );
        let info = sys::AudioOpenInfo {
            struct_size: 16,
            rate,
            channels: channels as u32,
            capacity_frames: capacity,
        };
        // SAFETY: the app passes room for the struct.
        unsafe { std::ptr::copy_nonoverlapping(&info as *const sys::AudioOpenInfo as *const u8, out, 16) };
        h
    }
    unsafe fn audio_write(&self, h: i32, ptr: *const f32, frames: i32) -> i32 {
        let mut s = self.lock();
        s.sync_audio();
        let Some(a) = s.audio.get_mut(&h) else { return err::NOT_FOUND };
        let queued = a.written - a.consumed as u64;
        let room = u64::from(a.capacity).saturating_sub(queued);
        let n = (frames.max(0) as u64).min(room) as usize;
        // SAFETY: the app's range: `frames` frames of `channels` samples.
        let samples = unsafe { std::slice::from_raw_parts(ptr, n * a.channels as usize) };
        a.samples.extend_from_slice(samples);
        a.written += n as u64;
        if a.starved && n > 0 {
            a.underruns += 1;
            a.starved = false;
        }
        n as i32
    }
    unsafe fn audio_clock(&self, h: i32, out: *mut u8) -> i32 {
        let mut s = self.lock();
        if s.off("audio_clock") {
            return err::UNSUPPORTED;
        }
        s.sync_audio();
        let now = s.now_us;
        let Some(a) = s.audio.get(&h) else { return err::NOT_FOUND };
        let latency_frames = a.latency_us as f64 * f64::from(a.rate) / 1e6;
        let info = sys::AudioClockInfo {
            struct_size: 32,
            underruns: a.underruns,
            frames_played: (a.consumed - latency_frames).max(0.0) as u64,
            host_time_us: now,
            latency_us: a.latency_us,
        };
        // SAFETY: the app passes room for the struct.
        unsafe { std::ptr::copy_nonoverlapping(&info as *const sys::AudioClockInfo as *const u8, out, 32) };
        0
    }
    unsafe fn audio_queued(&self, h: i32) -> i32 {
        let mut s = self.lock();
        if s.off("audio_queued") {
            return err::UNSUPPORTED;
        }
        s.sync_audio();
        s.audio.get(&h).map_or(err::NOT_FOUND, |a| (a.written - a.consumed as u64) as i32)
    }
    unsafe fn audio_latency_us(&self, h: i32) -> i64 {
        let s = self.lock();
        if s.off("audio_latency_us") {
            return err::UNSUPPORTED as i64;
        }
        s.audio.get(&h).map_or(err::NOT_FOUND as i64, |a| a.latency_us)
    }
    unsafe fn audio_flush(&self, h: i32) -> i32 {
        let mut s = self.lock();
        s.sync_audio();
        let Some(a) = s.audio.get_mut(&h) else { return err::NOT_FOUND };
        a.written = 0;
        a.consumed = 0.0;
        a.underruns = 0;
        a.starved = false;
        a.flushes += 1;
        0
    }
    unsafe fn audio_pause(&self, h: i32, paused: i32) -> i32 {
        let mut s = self.lock();
        s.sync_audio();
        let Some(a) = s.audio.get_mut(&h) else { return err::NOT_FOUND };
        a.paused = paused != 0;
        if a.paused {
            a.starved = false;
        }
        0
    }
    unsafe fn audio_volume(&self, h: i32, volume: f32) -> i32 {
        let mut s = self.lock();
        if volume.is_nan() {
            return err::INVALID;
        }
        let Some(a) = s.audio.get_mut(&h) else { return err::NOT_FOUND };
        a.volume = volume.clamp(0.0, 1.0);
        0
    }
    unsafe fn audio_close(&self, h: i32) -> i32 {
        let mut s = self.lock();
        if s.audio.remove(&h).is_none() {
            return err::NOT_FOUND;
        }
        s.audio_closed.push(h);
        0
    }

    unsafe fn now_playing_metadata(&self, meta: *const u8) -> i32 {
        let mut s = self.lock();
        if s.caps & sys::caps::NOW_PLAYING == 0 {
            return err::UNSUPPORTED;
        }
        // SAFETY: the app passes the 64-byte struct.
        let b = unsafe { bytes(meta, 64) }.to_vec();
        let art = field_bytes(&b, 40);
        if art.len() as i64 > s.limit(sys::limit::ART_BYTES) {
            return err::TOO_LARGE;
        }
        // Cover art must start like a PNG or a JPEG.
        if !art.is_empty()
            && !art.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
            && !art.starts_with(&[0xFF, 0xD8, 0xFF])
        {
            return err::INVALID;
        }
        let max = s.limit(sys::limit::STRING_BYTES).max(0) as usize;
        let mut strings = Vec::new();
        for at in [8, 16, 24, 32] {
            match String::from_utf8(field_bytes(&b, at)) {
                Ok(mut t) => {
                    // Too long: cut at a character boundary.
                    if t.len() > max {
                        let mut end = max;
                        while !t.is_char_boundary(end) {
                            end -= 1;
                        }
                        t.truncate(end);
                        s.truncated_strings += 1;
                    }
                    strings.push(t);
                }
                Err(_) => return err::INVALID,
            }
        }
        let art_mime = strings.pop().unwrap_or_default();
        let album = strings.pop().unwrap_or_default();
        let artist = strings.pop().unwrap_or_default();
        let title = strings.pop().unwrap_or_default();
        s.metadata.push(MetaRec {
            struct_size: rd32(&b, 0),
            title,
            artist,
            album,
            art_mime,
            art,
            duration_us: rd64(&b, 48),
            has_video: rd32(&b, 56) != 0,
        });
        // The first playback report after metadata always counts as a change.
        s.playback_fresh = true;
        0
    }
    unsafe fn now_playing_playback(&self, pb: *const u8) -> i32 {
        let mut s = self.lock();
        if s.caps & sys::caps::NOW_PLAYING == 0 {
            return err::UNSUPPORTED;
        }
        // SAFETY: the app passes the 40-byte struct.
        let raw = unsafe { std::ptr::read_unaligned(pb as *const sys::NowPlayingPlaybackRaw) };
        if raw.state > 2 || !raw.rate.is_finite() || raw.position_us < 0 {
            return err::INVALID;
        }
        let fresh = std::mem::take(&mut s.playback_fresh);
        if let Some(last) = s.playback.last().filter(|_| !fresh) {
            // Where the shell would put the position now: it extrapolates a playing item.
            let elapsed =
                if last.state == sys::play_state::PLAYING { raw.host_time_us - last.host_time_us } else { 0 };
            let expected = last.position_us + (elapsed as f64 * f64::from(last.rate)) as i64;
            if (last.state, last.flags, last.rate.to_bits()) == (raw.state, raw.flags, raw.rate.to_bits())
                && (raw.position_us - expected).abs() <= 500_000
            {
                s.needless_playback += 1;
                s.logs.push((1, "now_playing_playback sent again with nothing changed".into()));
            }
        }
        s.playback.push(raw);
        0
    }
    unsafe fn now_playing_clear(&self) -> i32 {
        self.lock().cleared += 1;
        0
    }

    unsafe fn viz_block(
        &self,
        pts_us: i64,
        rate: i32,
        channels: i32,
        _samples: *const f32,
        frames: i32,
    ) -> i32 {
        self.lock().viz_blocks.push((pts_us, rate, channels, frames));
        0
    }
    unsafe fn viz_summary(&self, sm: *const u8) -> i32 {
        let mut s = self.lock();
        if s.off("viz_summary") {
            return err::UNSUPPORTED;
        }
        // SAFETY: the app passes one summary.
        s.viz_summaries.push(unsafe { std::ptr::read_unaligned(sm as *const sys::VizSummaryRaw) });
        s.viz_single_calls += 1;
        0
    }
    unsafe fn viz_summary_n(&self, sm: *const u8, count: i32) -> i32 {
        let mut s = self.lock();
        if s.off("viz_summary_n") {
            return err::UNSUPPORTED;
        }
        // SAFETY: the app passes `count` summaries back to back; the stride is element 0's `struct_size`.
        let stride = unsafe { std::ptr::read_unaligned(sm as *const u32) } as usize;
        for i in 0..count.max(0) as usize {
            // SAFETY: as above.
            let one = unsafe { std::ptr::read_unaligned(sm.add(i * stride) as *const sys::VizSummaryRaw) };
            s.viz_summaries.push(one);
        }
        s.viz_batch_calls += 1;
        0
    }

    unsafe fn library_add_folder(&self) -> i32 {
        let mut s = self.lock();
        if s.caps & sys::caps::LIBRARY == 0 {
            return err::UNSUPPORTED;
        }
        s.next_request += 1;
        let r = s.next_request;
        s.folder_requests.push(r);
        r
    }
    unsafe fn library_roots(&self, buf: *mut u8, cap: i32) -> i32 {
        let s = self.lock();
        if s.caps & sys::caps::LIBRARY == 0 {
            return err::UNSUPPORTED;
        }
        let t: String = s
            .roots
            .iter()
            .map(|r| format!("{}\t{}\t{}\n", escape(&r.id), escape(&r.name), r.readable as u8))
            .collect();
        // SAFETY: the app's range.
        copy_out(unsafe { out_buf(buf, cap) }, t.as_bytes())
    }
    unsafe fn library_reconnect(&self, root: *const u8, len: i32) -> i32 {
        // SAFETY: the app's range.
        let t = unsafe { text(root, len) };
        self.lock().reconnects.push(t);
        0
    }
    unsafe fn library_rescan(&self, root: *const u8, len: i32) -> i32 {
        // SAFETY: the app's range.
        let t = unsafe { text(root, len) };
        let mut s = self.lock();
        let Some(root) = s.roots.iter().find(|r| r.id == t) else { return err::NOT_FOUND };
        if !root.readable {
            return err::IO;
        }
        // A walk of that root is already running: nothing starts.
        if !s.walking.contains(&t) {
            s.rescans.push(t);
        }
        0
    }
    unsafe fn library_forget(&self, root: *const u8, len: i32) -> i32 {
        // SAFETY: the app's range.
        let t = unsafe { text(root, len) };
        let mut s = self.lock();
        s.roots.retain(|r| r.id != t);
        s.forgets.push(t);
        0
    }
    unsafe fn library_listing(&self, root: *const u8, len: i32, offset: i64, buf: *mut u8, cap: i32) -> i64 {
        // SAFETY: the app's range.
        let t = unsafe { text(root, len) };
        let s = self.lock();
        let Some(l) = s.listings.get(&t) else { return err::NOT_FOUND as i64 };
        let shown = &l.blob[..l.visible.min(l.blob.len())];
        // SAFETY: the app's range.
        let out = unsafe { out_buf(buf, cap) };
        if offset >= 0 && (offset as usize) < shown.len() {
            copy_out(out, &shown[offset as usize..]);
        }
        shown.len() as i64
    }
    unsafe fn library_listing_release(&self, root: *const u8, len: i32) -> i32 {
        // SAFETY: the app's range.
        let t = unsafe { text(root, len) };
        let mut s = self.lock();
        s.listings.remove(&t);
        s.releases.push(t);
        0
    }

    unsafe fn kv_load(&self, key: *const u8, key_len: i32, buf: *mut u8, cap: i32) -> i32 {
        // SAFETY: the app's range.
        let k = unsafe { text(key, key_len) };
        let mut s = self.lock();
        if k.is_empty() {
            return err::INVALID;
        }
        if k.len() as i64 > s.limit(sys::limit::KEY_LEN) {
            return err::TOO_LARGE;
        }
        if s.kv_failed.remove(&k) {
            return err::IO;
        }
        if s.kv_cold.remove(&k) {
            let d = s.kv_latency_us;
            s.push_after(d, events::io_ready(0));
            if s.kv_fail.contains(&k) {
                s.kv_failed.insert(k);
            }
            return err::BUSY;
        }
        let Some((v, _)) = s.kv.get(&k) else { return err::NOT_FOUND };
        // SAFETY: the app's range.
        copy_out(unsafe { out_buf(buf, cap) }, v)
    }
    unsafe fn kv_store(&self, key: *const u8, key_len: i32, val: *const u8, val_len: i32, flags: i32) -> i32 {
        // SAFETY: the app's range.
        let (k, v) = unsafe { (text(key, key_len), bytes(val, val_len).to_vec()) };
        let mut s = self.lock();
        // 1 to 255 bytes: an empty key is `-INVALID`, a longer one `-TOO_LARGE` (a size, not a count).
        if k.is_empty() {
            return err::INVALID;
        }
        if k.len() as i64 > s.limit(sys::limit::KEY_LEN) {
            return err::TOO_LARGE;
        }
        if v.len() as i64 > s.limit(sys::limit::VALUE_SIZE) {
            return err::TOO_LARGE;
        }
        s.kv_stores.push((k.clone(), v.len(), flags));
        if v.is_empty() {
            s.kv.remove(&k);
            s.kv_age.remove(&k);
            return 0;
        }
        if let Some(q) = s.kv_quota {
            // Cache-class entries go first, oldest first, until the value fits; if it still does not, the call fails and what was
            // evicted for it stays evicted.
            loop {
                let used: usize = s.kv.iter().filter(|(kk, _)| **kk != k).map(|(_, (vv, _))| vv.len()).sum();
                if used + v.len() <= q {
                    break;
                }
                let oldest =
                    s.kv.iter()
                        .filter(|(kk, (_, fl))| **kk != k && fl & sys::kv::CACHE != 0)
                        .min_by_key(|(kk, _)| s.kv_age.get(*kk).copied().unwrap_or(0))
                        .map(|(kk, _)| kk.clone());
                let Some(victim) = oldest else { return err::NO_SPACE };
                s.kv.remove(&victim);
                s.kv_age.remove(&victim);
            }
        }
        s.kv_clock += 1;
        let t = s.kv_clock;
        s.kv_age.insert(k.clone(), t);
        s.kv.insert(k, (v, flags));
        0
    }
    unsafe fn kv_flush(&self) -> i32 {
        self.lock().kv_flushes += 1;
        0
    }

    unsafe fn file_pick(&self, kinds: *const u8, kinds_len: i32, flags: i32) -> i32 {
        // SAFETY: the app's range.
        let kinds = unsafe { text(kinds, kinds_len) };
        let mut s = self.lock();
        s.next_request += 1;
        let request = s.next_request;
        s.picks.push(PickRec { request, kinds, flags });
        request
    }
    unsafe fn file_save(
        &self,
        name: *const u8,
        name_len: i32,
        mime: *const u8,
        mime_len: i32,
        data: *const u8,
        len: i32,
    ) -> i32 {
        // SAFETY: the app's ranges.
        let (name, mime, data) =
            unsafe { (text(name, name_len), text(mime, mime_len), bytes(data, len).to_vec()) };
        let mut s = self.lock();
        // Only the last component of the name is used.
        let name = name.rsplit(['/', '\\']).next().unwrap_or("").to_string();
        if name.is_empty() {
            return err::INVALID;
        }
        s.next_request += 1;
        let request = s.next_request;
        s.saves.push(SaveRec { request, name, mime, data });
        request
    }
    unsafe fn file_open_id(&self, id: *const u8, len: i32) -> i32 {
        // SAFETY: the app's range.
        let id = unsafe { text(id, len) };
        let mut s = self.lock();
        if s.open_failed.remove(&id) {
            return err::IO;
        }
        if s.open_cold.remove(&id) {
            let d = s.kv_latency_us;
            s.push_after(d, events::io_ready(0));
            if s.open_fail.contains(&id) {
                s.open_failed.insert(id);
            }
            return err::BUSY;
        }
        let Some(spec) = s.known_files.get(&id).cloned() else { return err::NOT_FOUND };
        if s.open_files.len() as i64 >= s.limit(sys::limit::HANDLES) {
            return err::NO_SPACE;
        }
        s.new_handle(spec)
    }
    unsafe fn file_open_sibling(&self, h: i32, name: *const u8, name_len: i32) -> i32 {
        // SAFETY: the app's range.
        let name = unsafe { text(name, name_len) };
        let mut s = self.lock();
        let Some(f) = s.open_files.get(&h) else { return err::NOT_FOUND };
        let Some(dir) = f.spec.dir.clone() else { return err::UNSUPPORTED };
        let Some(spec) = s.siblings.get(&dir).and_then(|m| m.get(&name)).cloned() else {
            return err::NOT_FOUND;
        };
        if s.open_files.len() as i64 >= s.limit(sys::limit::HANDLES) {
            return err::NO_SPACE;
        }
        s.new_handle(spec)
    }
    unsafe fn file_size(&self, h: i32) -> i64 {
        let s = self.lock();
        match s.open_files.get(&h) {
            None => err::NOT_FOUND as i64,
            Some(f) if f.spec.folder => err::INVALID as i64,
            Some(f) if f.spec.unknown_size => err::UNSUPPORTED as i64,
            Some(f) => f.spec.data.len() as i64,
        }
    }
    unsafe fn file_read_at(&self, h: i32, offset: i64, buf: *mut u8, cap: i32) -> i32 {
        let mut s = self.lock();
        let (now, max) = (s.now_us, s.limit(sys::limit::READ_PER_CALL) as usize);
        let Some(f) = s.open_files.get_mut(&h) else { return err::NOT_FOUND };
        if f.spec.folder {
            return err::INVALID;
        }
        let len = f.spec.data.len() as u64;
        if offset < 0 {
            return err::INVALID;
        }
        // Nothing to read: 0, which is not the end of the file.
        if cap == 0 {
            return 0;
        }
        let offset = offset as u64;
        if offset >= len {
            return 0;
        }
        let block = offset / BLOCK;
        if f.spec.latency_us > 0 {
            match f.blocks.get(&block).copied() {
                Some(ready) if ready <= now => {
                    if f.spec.fail_io {
                        return err::IO;
                    }
                }
                Some(_) => return err::BUSY,
                None => {
                    let ready = now + f.spec.latency_us;
                    f.blocks.insert(block, ready);
                    s.push_after(ready - now, events::io_ready(h));
                    return err::BUSY;
                }
            }
        }
        let Some(f) = s.open_files.get(&h) else { return err::NOT_FOUND };
        // Only the rest of the block is served per call, like a host that fetches in blocks.
        let block_end = if f.spec.latency_us > 0 { ((block + 1) * BLOCK).min(len) } else { len };
        let n = (block_end - offset).min(cap.max(0) as u64).min(max as u64) as usize;
        // SAFETY: the app's range.
        let out = unsafe { out_buf(buf, cap) };
        out[..n].copy_from_slice(&f.spec.data[offset as usize..offset as usize + n]);
        n as i32
    }
    unsafe fn file_prefetch(&self, h: i32, offset: i64, len: i32) -> i32 {
        self.lock().prefetches.push((h, offset, len));
        0
    }
    unsafe fn file_name(&self, h: i32, buf: *mut u8, cap: i32) -> i32 {
        let s = self.lock();
        let Some(f) = s.open_files.get(&h) else { return err::NOT_FOUND };
        // SAFETY: the app's range.
        copy_out(unsafe { out_buf(buf, cap) }, f.spec.name.as_bytes())
    }
    unsafe fn file_id(&self, h: i32, buf: *mut u8, cap: i32) -> i32 {
        let s = self.lock();
        let Some(f) = s.open_files.get(&h) else { return err::NOT_FOUND };
        let Some(id) = &f.spec.id else { return err::UNSUPPORTED };
        // SAFETY: the app's range.
        copy_out(unsafe { out_buf(buf, cap) }, id.as_bytes())
    }
    unsafe fn file_close(&self, h: i32) -> i32 {
        let mut s = self.lock();
        if s.open_files.remove(&h).is_none() {
            return err::NOT_FOUND;
        }
        s.closed.push(h);
        0
    }
}
