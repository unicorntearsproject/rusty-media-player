//! The scan driver: turns a host [`Listing`] into index updates a little at a time, so it can run inside the application's
//! tick (browser file reads are asynchronous; nothing here blocks or spawns threads).
//!
//! A listing is first compared with the index ([`Library::begin_scan`]); the files that are new or changed are read a few at
//! a time through `rvp-demux` ([`read_tags`]), then folder pictures are read for albums that need them, and the views are
//! rebuilt ([`Library::finish_scan`]). More listings can be queued while one is running.
//!
//! After the scans there is a second, slower job when the automatic level is on: tracks whose tags say nothing about their
//! loudness are decoded and measured ([`Scanner::start_analysis`]), a track at a time, within a small time budget per tick, and
//! the result is filed in the index.
//!
//! Videos are read in the same scan (their headers, see [`read_video_info`]); afterwards a poster job decodes one frame of each video
//! that has none ([`Scanner::start_posters`]), a video at a time within a small time budget per tick.
use crate::art::Thumb;
use crate::index::{Library, ScanReport};
use crate::model::TrackId;
use crate::scan::{TrackTags, read_all, read_tags};
use crate::video::{VideoId, VideoInfo, make_poster, read_video_info};
use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use rvp_core::{CodecFactory, Error, Timestamp};
use rvp_host::{FileEntry, Host, Listing, OpenRequest};
use rvp_player::exec::Executor;

/// Files read at the same time.
const IN_FLIGHT: usize = 4;
/// Time one tick may spend filing results, microseconds.
const BUDGET_US: Timestamp = 4_000;
/// The views are rebuilt this often while a scan runs, microseconds.
const REBUILD_EVERY_US: Timestamp = 400_000;
/// Folder pictures bigger than this are not read.
const MAX_PICTURE: usize = 16 << 20;
/// Time one tick may spend decoding files to measure them, microseconds (a tenth or less of a frame).
const ANALYSIS_BUDGET_US: Timestamp = 4_000;
/// The index is saved after this many tracks have been measured.
const ANALYSIS_SAVE_EVERY: usize = 25;
/// Time one tick may spend decoding a poster frame, microseconds.
const POSTER_BUDGET_US: Timestamp = 4_000;
/// The videos are saved after this many posters have been made.
const POSTER_SAVE_EVERY: usize = 10;

type TagResult = (u16, FileEntry, Result<TrackTags, Error>);
type VideoResult = (u16, FileEntry, Result<VideoInfo, Error>);
type ArtResult = (u16, String, Option<Vec<u8>>);
/// A measured track: its id and its integrated loudness (`None`: nothing to measure).
type Measured = (TrackId, Option<f32>);
/// A made poster: the video's id and its picture (`None`: no frame could be made).
type Made = (VideoId, Option<Thumb>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Tags,
    Pictures,
}

/// What a scan is doing, for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanStatus {
    /// Name of the folder being scanned (empty while measuring loudness).
    pub root: String,
    /// Files read so far.
    pub done: usize,
    /// Files to read in all.
    pub total: usize,
    /// Measuring the loudness of tracks (after the scans) rather than reading tags.
    pub analysing: bool,
    /// Making poster pictures of videos (after the scans) rather than reading tags.
    pub posters: bool,
}

/// What a call to [`Scanner::tick`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanEvent {
    /// Nothing to report.
    Quiet,
    /// The index changed (progress, or the views were rebuilt).
    Progress,
    /// A scan finished; the index is complete and should be saved.
    Finished(ScanReport),
    /// Loudness measurements were filed (a batch, or the last ones); the index should be saved.
    Analysed,
    /// Posters were filed (a batch, or the last ones); the videos (and the thumbnails) should be saved.
    Posters,
}

/// Tracks being measured.
struct Analysis {
    queue: VecDeque<(TrackId, String)>,
    total: usize,
    done: usize,
    since_save: usize,
}

/// Videos getting their posters.
struct PosterJob {
    queue: VecDeque<(VideoId, String)>,
    total: usize,
    done: usize,
    since_save: usize,
}

/// Drives scans.
pub struct Scanner {
    exec: Executor,
    queue: VecDeque<Listing>,
    phase: Phase,
    root: u16,
    root_name: String,
    reads: VecDeque<FileEntry>,
    video_reads: VecDeque<FileEntry>,
    video_results: Rc<RefCell<Vec<VideoResult>>>,
    posters: Option<PosterJob>,
    poster_results: Rc<RefCell<Vec<Made>>>,
    poster_alive: usize,
    tag_results: Rc<RefCell<Vec<TagResult>>>,
    art_results: Rc<RefCell<Vec<ArtResult>>>,
    alive: usize,
    done: usize,
    total: usize,
    art_total: usize,
    art_done: usize,
    last_rebuild: Timestamp,
    dirty_view: bool,
    codecs: Option<Rc<dyn CodecFactory>>,
    analysis: Option<Analysis>,
    analysis_results: Rc<RefCell<Vec<Measured>>>,
    analysis_alive: usize,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    /// An idle scanner.
    pub fn new() -> Self {
        Self {
            exec: Executor::new(),
            queue: VecDeque::new(),
            phase: Phase::Idle,
            root: 0,
            root_name: String::new(),
            reads: VecDeque::new(),
            video_reads: VecDeque::new(),
            video_results: Rc::default(),
            posters: None,
            poster_results: Rc::default(),
            poster_alive: 0,
            tag_results: Rc::default(),
            art_results: Rc::default(),
            alive: 0,
            done: 0,
            total: 0,
            art_total: 0,
            art_done: 0,
            last_rebuild: 0,
            dirty_view: false,
            codecs: None,
            analysis: None,
            analysis_results: Rc::default(),
            analysis_alive: 0,
        }
    }

    /// The decoders loudness is measured with (without them [`Scanner::start_analysis`] does nothing).
    pub fn set_codecs(&mut self, codecs: Rc<dyn CodecFactory>) {
        self.codecs = Some(codecs);
    }

    /// Measure the loudness of `tracks` (`(track id, what the host opens it with)`, as [`Library::pending_loudness`] lists them)
    /// in the background, after any scan that is running. Tracks that are already queued are not queued twice.
    pub fn start_analysis(&mut self, tracks: Vec<(TrackId, String)>) {
        if self.codecs.is_none() || tracks.is_empty() {
            return;
        }
        match &mut self.analysis {
            Some(a) => {
                for t in tracks {
                    if !a.queue.iter().any(|q| q.0 == t.0) {
                        a.queue.push_back(t);
                        a.total += 1;
                    }
                }
            }
            None => {
                let total = tracks.len();
                self.analysis = Some(Analysis { queue: tracks.into(), total, done: 0, since_save: 0 });
            }
        }
    }

    /// Make posters for `videos` (`(video id, what the host opens it with)`, as [`Library::pending_posters`] lists them) in the
    /// background, after any scan that is running. Videos that are already queued are not queued twice.
    pub fn start_posters(&mut self, videos: Vec<(VideoId, String)>) {
        if self.codecs.is_none() || videos.is_empty() {
            return;
        }
        match &mut self.posters {
            Some(p) => {
                for v in videos {
                    if !p.queue.iter().any(|q| q.0 == v.0) {
                        p.queue.push_back(v);
                        p.total += 1;
                    }
                }
            }
            None => {
                let total = videos.len();
                self.posters = Some(PosterJob { queue: videos.into(), total, done: 0, since_save: 0 });
            }
        }
    }

    /// Stop making posters; the ones made stay filed.
    pub fn stop_posters(&mut self) {
        self.posters = None;
    }

    /// Stop measuring (the setting that wanted it was turned off); what is measured stays filed.
    pub fn stop_analysis(&mut self) {
        self.analysis = None;
    }

    /// Queue a listing; it starts on the next tick (right away if nothing is running).
    pub fn push(&mut self, listing: Listing) {
        self.queue.push_back(listing);
    }

    /// True while a scan is running or queued.
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
            || !self.queue.is_empty()
            || self.analysis.is_some()
            || self.posters.is_some()
    }

    /// True while a folder is being scanned or one is queued (not counting the loudness measurement and the posters that follow).
    pub fn scanning(&self) -> bool {
        self.phase != Phase::Idle || !self.queue.is_empty()
    }

    /// Progress of the running scan.
    pub fn status(&self) -> Option<ScanStatus> {
        if self.phase != Phase::Idle {
            return Some(ScanStatus {
                root: self.root_name.clone(),
                done: self.done,
                total: self.total,
                analysing: false,
                posters: false,
            });
        }
        if let Some(p) = &self.posters {
            return Some(ScanStatus {
                root: String::new(),
                done: p.done,
                total: p.total,
                analysing: false,
                posters: true,
            });
        }
        self.analysis.as_ref().map(|a| ScanStatus {
            root: String::new(),
            done: a.done,
            total: a.total,
            analysing: true,
            posters: false,
        })
    }

    /// Run one step against `lib` through `host`.
    pub fn tick<H>(&mut self, lib: &mut Library, host: &mut H) -> ScanEvent
    where
        H: Host,
        H::Source: 'static,
    {
        let t0 = host.clock().now_us();
        if self.phase == Phase::Idle {
            let Some(listing) = self.queue.pop_front() else {
                // Posters come first (they are what the person sees); a loudness measurement under way is finished first.
                if self.posters.is_some() && self.analysis_alive == 0 {
                    return self.poster_tick(lib, host);
                }
                return self.analysis_tick(lib, host);
            };
            let plan = lib.begin_scan(&listing.root, &listing.name, &listing.files);
            self.root = plan.root;
            self.root_name = listing.name;
            self.reads = plan.read.into();
            self.video_reads = plan.read_videos.into();
            self.total = self.reads.len() + self.video_reads.len();
            self.done = 0;
            self.art_total = 0;
            self.art_done = 0;
            self.phase = Phase::Tags;
            self.last_rebuild = t0;
            self.dirty_view = true; // the removals
        }
        let mut event = ScanEvent::Quiet;
        match self.phase {
            Phase::Tags => {
                // Start reads (opening a file is immediate in every host: the bytes are read inside the task).
                while self.alive < IN_FLIGHT && !self.reads.is_empty() {
                    let Some(entry) = self.reads.pop_front() else { break };
                    match rvp_core::task::block_on(host.open(OpenRequest::Id(entry.id.clone()))) {
                        Ok(src) => {
                            let out = self.tag_results.clone();
                            let root = self.root;
                            self.exec.spawn(async move {
                                let r = read_tags(src).await;
                                out.borrow_mut().push((root, entry, r));
                            });
                            self.alive += 1;
                        }
                        Err(e) => self.tag_results.borrow_mut().push((self.root, entry, Err(e.into()))),
                    }
                }
                while self.alive < IN_FLIGHT && !self.video_reads.is_empty() {
                    let Some(entry) = self.video_reads.pop_front() else { break };
                    match rvp_core::task::block_on(host.open(OpenRequest::Id(entry.id.clone()))) {
                        Ok(src) => {
                            let out = self.video_results.clone();
                            let root = self.root;
                            self.exec.spawn(async move {
                                let r = read_video_info(src).await;
                                out.borrow_mut().push((root, entry, r));
                            });
                            self.alive += 1;
                        }
                        Err(e) => self.video_results.borrow_mut().push((self.root, entry, Err(e.into()))),
                    }
                }
                self.alive = self.exec.poll_all();
                // File the results, within the time budget (at least one, so progress never stalls).
                let results = core::mem::take(&mut *self.tag_results.borrow_mut());
                let mut rest = results.into_iter();
                let mut first = true;
                for (root, entry, r) in rest.by_ref() {
                    lib.apply_tags(root, &entry, r);
                    self.done += 1;
                    self.dirty_view = true;
                    if !first && host.clock().now_us() - t0 > BUDGET_US {
                        break;
                    }
                    first = false;
                }
                self.tag_results.borrow_mut().extend(rest);
                let vresults = core::mem::take(&mut *self.video_results.borrow_mut());
                let mut vrest = vresults.into_iter();
                for (root, entry, r) in vrest.by_ref() {
                    lib.apply_video(root, &entry, r);
                    self.done += 1;
                    self.dirty_view = true;
                    if host.clock().now_us() - t0 > BUDGET_US {
                        break;
                    }
                }
                self.video_results.borrow_mut().extend(vrest);
                if self.reads.is_empty()
                    && self.video_reads.is_empty()
                    && self.alive == 0
                    && self.tag_results.borrow().is_empty()
                    && self.video_results.borrow().is_empty()
                {
                    // Tags are in: fetch the folder pictures some albums need.
                    let pending = lib.pending_folder_art();
                    self.art_total = pending.len();
                    for (root, dir, id, _size) in pending {
                        match rvp_core::task::block_on(host.open(OpenRequest::Id(id))) {
                            Ok(src) => {
                                let out = self.art_results.clone();
                                self.exec.spawn(async move {
                                    let bytes = read_all(src, MAX_PICTURE).await.ok();
                                    out.borrow_mut().push((root, dir, bytes));
                                });
                            }
                            Err(_) => self.art_results.borrow_mut().push((root, dir, None)),
                        }
                    }
                    self.alive = self.exec.poll_all();
                    self.phase = Phase::Pictures;
                }
                event = ScanEvent::Progress;
            }
            Phase::Pictures => {
                self.alive = self.exec.poll_all();
                let results = core::mem::take(&mut *self.art_results.borrow_mut());
                for (root, dir, bytes) in results {
                    lib.set_folder_art(root, &dir, bytes.as_deref());
                    self.art_done += 1;
                }
                if self.alive == 0 && self.art_done >= self.art_total && self.art_results.borrow().is_empty()
                {
                    lib.finish_scan();
                    self.phase = Phase::Idle;
                    self.dirty_view = false;
                    return ScanEvent::Finished(lib.report);
                }
            }
            Phase::Idle => {}
        }
        if self.dirty_view && host.clock().now_us() - self.last_rebuild >= REBUILD_EVERY_US {
            lib.rebuild();
            self.last_rebuild = host.clock().now_us();
            self.dirty_view = false;
            event = ScanEvent::Progress;
        }
        event
    }

    /// One step of the poster job: start the next video when none is being decoded, give the decoding a few milliseconds, file what
    /// is finished.
    fn poster_tick<H>(&mut self, lib: &mut Library, host: &mut H) -> ScanEvent
    where
        H: Host,
        H::Source: 'static,
    {
        let (Some(job), Some(codecs)) = (&mut self.posters, &self.codecs) else {
            self.posters = None;
            return ScanEvent::Quiet;
        };
        let t0 = host.clock().now_us();
        while self.poster_alive == 0 {
            let Some((id, src)) = job.queue.pop_front() else { break };
            match rvp_core::task::block_on(host.open(OpenRequest::Id(src))) {
                Ok(source) => {
                    let (out, codecs) = (self.poster_results.clone(), codecs.clone());
                    self.exec.spawn(async move {
                        let r = make_poster(source, &*codecs).await;
                        out.borrow_mut().push((id, r.ok().flatten()));
                    });
                    self.poster_alive = 1;
                }
                // A file that cannot be opened gets no poster (and is not asked for again until it changes).
                Err(_) => self.poster_results.borrow_mut().push((id, None)),
            }
        }
        while self.poster_alive > 0 {
            self.poster_alive = self.exec.poll_all();
            if host.clock().now_us() - t0 >= POSTER_BUDGET_US {
                break;
            }
        }
        let results = core::mem::take(&mut *self.poster_results.borrow_mut());
        let any = !results.is_empty();
        for (id, thumb) in results {
            lib.set_poster(id, thumb);
            job.done += 1;
            job.since_save += 1;
        }
        if job.queue.is_empty() && self.poster_alive == 0 {
            self.posters = None;
            return ScanEvent::Posters;
        }
        if job.since_save >= POSTER_SAVE_EVERY {
            job.since_save = 0;
            return ScanEvent::Posters;
        }
        if any { ScanEvent::Progress } else { ScanEvent::Quiet }
    }

    /// One step of the loudness measurement: start the next track when none is being measured, give the decoding a few
    /// milliseconds, file what is finished.
    fn analysis_tick<H>(&mut self, lib: &mut Library, host: &mut H) -> ScanEvent
    where
        H: Host,
        H::Source: 'static,
    {
        let (Some(an), Some(codecs)) = (&mut self.analysis, &self.codecs) else {
            self.analysis = None;
            return ScanEvent::Quiet;
        };
        let t0 = host.clock().now_us();
        while self.analysis_alive == 0 {
            let Some((id, src)) = an.queue.pop_front() else { break };
            match rvp_core::task::block_on(host.open(OpenRequest::Id(src))) {
                Ok(source) => {
                    let (out, codecs) = (self.analysis_results.clone(), codecs.clone());
                    self.exec.spawn(async move {
                        let r = rvp_player::measure_source(source, &*codecs).await;
                        out.borrow_mut().push((id, r.ok().flatten().map(|m| m.integrated_lufs)));
                    });
                    self.analysis_alive = 1;
                }
                // A file that cannot be opened is not measured (and not asked for again until it changes).
                Err(_) => self.analysis_results.borrow_mut().push((id, None)),
            }
        }
        // Decode for a few milliseconds (the task gives the executor a turn every few dozen packets).
        while self.analysis_alive > 0 {
            self.analysis_alive = self.exec.poll_all();
            if host.clock().now_us() - t0 >= ANALYSIS_BUDGET_US {
                break;
            }
        }
        let results = core::mem::take(&mut *self.analysis_results.borrow_mut());
        let any = !results.is_empty();
        for (id, lufs) in results {
            lib.set_measured(id, lufs);
            an.done += 1;
            an.since_save += 1;
        }
        let finished = an.queue.is_empty() && self.analysis_alive == 0;
        if finished {
            self.analysis = None;
            return ScanEvent::Analysed;
        }
        if an.since_save >= ANALYSIS_SAVE_EVERY {
            an.since_save = 0;
            return ScanEvent::Analysed;
        }
        if any { ScanEvent::Progress } else { ScanEvent::Quiet }
    }
}
