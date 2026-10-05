//! The scan driver: turns a host [`Listing`] into index updates a little at a time, so it can run inside the application's
//! tick (browser file reads are asynchronous; nothing here blocks or spawns threads).
//!
//! A listing is first compared with the index ([`Library::begin_scan`]); the files that are new or changed are read a few at
//! a time through `rvp-demux` ([`read_tags`]), then folder pictures are read for albums that need them, and the views are
//! rebuilt ([`Library::finish_scan`]). More listings can be queued while one is running.
use crate::index::{Library, ScanReport};
use crate::scan::{TrackTags, read_all, read_tags};
use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use rvp_core::{Error, Timestamp};
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

type TagResult = (u16, FileEntry, Result<TrackTags, Error>);
type ArtResult = (u16, String, Option<Vec<u8>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Tags,
    Pictures,
}

/// What a scan is doing, for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanStatus {
    /// Name of the folder being scanned.
    pub root: String,
    /// Files read so far.
    pub done: usize,
    /// Files to read in all.
    pub total: usize,
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
}

/// Drives scans.
pub struct Scanner {
    exec: Executor,
    queue: VecDeque<Listing>,
    phase: Phase,
    root: u16,
    root_name: String,
    reads: VecDeque<FileEntry>,
    tag_results: Rc<RefCell<Vec<TagResult>>>,
    art_results: Rc<RefCell<Vec<ArtResult>>>,
    alive: usize,
    done: usize,
    total: usize,
    art_total: usize,
    art_done: usize,
    last_rebuild: Timestamp,
    dirty_view: bool,
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
            tag_results: Rc::default(),
            art_results: Rc::default(),
            alive: 0,
            done: 0,
            total: 0,
            art_total: 0,
            art_done: 0,
            last_rebuild: 0,
            dirty_view: false,
        }
    }

    /// Queue a listing; it starts on the next tick (right away if nothing is running).
    pub fn push(&mut self, listing: Listing) {
        self.queue.push_back(listing);
    }

    /// True while a scan is running or queued.
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle || !self.queue.is_empty()
    }

    /// Progress of the running scan.
    pub fn status(&self) -> Option<ScanStatus> {
        (self.phase != Phase::Idle).then(|| ScanStatus {
            root: self.root_name.clone(),
            done: self.done,
            total: self.total,
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
            let Some(listing) = self.queue.pop_front() else { return ScanEvent::Quiet };
            let plan = lib.begin_scan(&listing.root, &listing.name, &listing.files);
            self.root = plan.root;
            self.root_name = listing.name;
            self.reads = plan.read.into();
            self.total = self.reads.len();
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
                if self.reads.is_empty() && self.alive == 0 && self.tag_results.borrow().is_empty() {
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
}
