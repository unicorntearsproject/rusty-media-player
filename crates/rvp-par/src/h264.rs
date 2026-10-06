//! H.264 with the reconstruction on a thread of its own: while it reconstructs and deblocks picture N, the decoder
//! thread already parses picture N + 1.
use rvp_codec_h264::decoder::Frame;
use rvp_codec_h264::decoder::parse::ParseRunner;
use rvp_codec_h264::decoder::recon::{ReconEvent, ReconExecutor, Reconstructor};
use rvp_core::par::SpinLock;
use rvp_core::{StreamInfo, VideoDecoder};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::Thread;
use std::time::Duration;

/// What waiting threads do between looks at what they wait for: sleep for a little, longer the longer they have waited.
fn sleep(micros: u32) {
    std::thread::park_timeout(Duration::from_micros(micros as u64));
}

fn install_relax() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        rvp_core::par::set_relax(|| sleep(60));
        rvp_core::par::set_sleep(sleep);
    });
}

/// Pictures that may wait for reconstruction before the parsing side is made to wait.
const MAX_PICTURES_QUEUED: usize = 3;

#[derive(Default)]
struct Shared {
    inbox: SpinLock<VecDeque<ReconEvent>>,
    frames: SpinLock<VecDeque<Frame>>,
    /// Events submitted and not yet finished.
    queued: AtomicUsize,
    /// Picture events among them.
    pictures: AtomicUsize,
    stop: AtomicBool,
    thread: SpinLock<Option<Thread>>,
    /// The thread waiting for the worker (to catch up, or to go idle): woken when the worker finishes an event.
    waiter: SpinLock<Option<Thread>>,
}

impl Shared {
    fn wake_waiter(&self) {
        if let Some(t) = self.waiter.lock().as_ref() {
            t.unpark();
        }
    }

    /// Sleep until `done()` (the worker wakes this thread after each event; the timeout is only a safety net).
    fn wait_until(&self, done: impl Fn() -> bool) {
        while !done() {
            *self.waiter.lock() = Some(std::thread::current());
            // Look again after registering, so a wake between the check and the registration is not lost.
            if done() {
                break;
            }
            std::thread::park_timeout(Duration::from_millis(5));
        }
    }
}

/// A [`ReconExecutor`] running a [`Reconstructor`] on its own thread. Only for threads that may wait (the decoder
/// thread, never a browser's main thread).
pub struct ThreadedRecon {
    sh: Arc<Shared>,
}

impl ThreadedRecon {
    /// Start the reconstruction thread.
    pub fn new() -> Self {
        install_relax();
        let sh = Arc::new(Shared::default());
        let worker = sh.clone();
        crate::spawn(move || run(worker));
        Self { sh }
    }

    fn wake(&self) {
        if let Some(t) = self.sh.thread.lock().as_ref() {
            t.unpark();
        }
    }
}

impl Default for ThreadedRecon {
    fn default() -> Self {
        Self::new()
    }
}

fn run(sh: Arc<Shared>) {
    *sh.thread.lock() = Some(std::thread::current());
    let mut rec = Reconstructor::new();
    let mut out = VecDeque::new();
    while !sh.stop.load(Ordering::Acquire) {
        let ev = sh.inbox.lock().pop_front();
        let Some(ev) = ev else {
            std::thread::park();
            continue;
        };
        let is_picture = matches!(ev, ReconEvent::Picture(_));
        rec.handle(ev, &mut out);
        if !out.is_empty() {
            sh.frames.lock().append(&mut out);
        }
        if is_picture {
            sh.pictures.fetch_sub(1, Ordering::AcqRel);
        }
        // Last, so that a zero count means every frame is already visible.
        sh.queued.fetch_sub(1, Ordering::AcqRel);
        sh.wake_waiter();
    }
}

impl ReconExecutor for ThreadedRecon {
    fn submit(&mut self, ev: ReconEvent) {
        let is_picture = matches!(ev, ReconEvent::Picture(_));
        if is_picture {
            // Do not run arbitrarily far ahead of the reconstruction (each picture holds its macroblock data).
            self.sh.wait_until(|| self.sh.pictures.load(Ordering::Acquire) < MAX_PICTURES_QUEUED);
            self.sh.pictures.fetch_add(1, Ordering::AcqRel);
        }
        self.sh.queued.fetch_add(1, Ordering::AcqRel);
        self.sh.inbox.lock().push_back(ev);
        self.wake();
    }

    fn take_frames(&mut self, out: &mut VecDeque<Frame>) {
        out.append(&mut self.sh.frames.lock());
    }

    fn pending(&self) -> usize {
        self.sh.queued.load(Ordering::Acquire)
    }

    fn wait_idle(&mut self) {
        self.sh.wait_until(|| self.sh.queued.load(Ordering::Acquire) == 0);
    }
}

impl Drop for ThreadedRecon {
    fn drop(&mut self) {
        self.sh.stop.store(true, Ordering::Release);
        self.wake();
    }
}

type Task = Box<dyn FnOnce() + Send + 'static>;

struct ParseShared {
    queue: SpinLock<VecDeque<Task>>,
    /// Tasks queued or running.
    active: AtomicUsize,
    stop: AtomicBool,
    threads: SpinLock<Vec<Thread>>,
    /// The thread that is waiting for the workers to catch up: woken when a task finishes.
    waiter: SpinLock<Option<Thread>>,
}

/// Parses pictures on a few threads. Tasks start in the order they are submitted, which is what keeps a picture's wait
/// for the motion of an earlier one from ever deadlocking.
pub struct ParseWorkers {
    sh: Arc<ParseShared>,
    limit: usize,
}

impl ParseWorkers {
    /// Start `n` parse threads.
    pub fn new(n: usize) -> Self {
        install_relax();
        let sh = Arc::new(ParseShared {
            queue: SpinLock::new(VecDeque::new()),
            active: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            threads: SpinLock::new(Vec::new()),
            waiter: SpinLock::new(None),
        });
        for _ in 0..n.max(1) {
            let w = sh.clone();
            crate::spawn(move || {
                w.threads.lock().push(std::thread::current());
                while !w.stop.load(Ordering::Acquire) {
                    let task = w.queue.lock().pop_front();
                    match task {
                        Some(t) => {
                            t();
                            w.active.fetch_sub(1, Ordering::AcqRel);
                            if let Some(t) = w.waiter.lock().as_ref() {
                                t.unpark();
                            }
                        }
                        None => std::thread::park(),
                    }
                }
            });
        }
        Self { sh, limit: n.max(1) + 1 }
    }
}

impl ParseRunner for ParseWorkers {
    fn spawn(&mut self, job: Task) {
        // Do not run far ahead: every unfinished picture holds its slice data and macroblock arrays.
        while self.sh.active.load(Ordering::Acquire) >= self.limit {
            *self.sh.waiter.lock() = Some(std::thread::current());
            if self.sh.active.load(Ordering::Acquire) < self.limit {
                break;
            }
            std::thread::park_timeout(Duration::from_millis(5));
        }
        self.sh.active.fetch_add(1, Ordering::AcqRel);
        self.sh.queue.lock().push_back(job);
        for t in self.sh.threads.lock().iter() {
            t.unpark();
        }
    }
}

impl Drop for ParseWorkers {
    fn drop(&mut self) {
        self.sh.stop.store(true, Ordering::Release);
        for t in self.sh.threads.lock().iter() {
            t.unpark();
        }
    }
}

/// An H.264 decoder with its reconstruction on a second thread, and pictures parsed on `parse_threads` threads. Build it on the thread that will drive it (a
/// [`crate::ThreadedVideoDecoder`] worker, or a native thread that may wait).
pub fn h264_pipelined(info: &StreamInfo) -> rvp_core::Result<Box<dyn VideoDecoder>> {
    h264_pipelined_with(info, 2)
}

/// [`h264_pipelined`] with `parse_threads` threads parsing pictures in parallel (0: parse on the decoder thread).
pub fn h264_pipelined_with(
    info: &StreamInfo,
    parse_threads: usize,
) -> rvp_core::Result<Box<dyn VideoDecoder>> {
    let runner: Option<Box<dyn ParseRunner>> =
        (parse_threads > 0).then(|| Box::new(ParseWorkers::new(parse_threads)) as Box<dyn ParseRunner>);
    rvp_codec_h264::h264_decoder_with(info, Some(Box::new(ThreadedRecon::new())), runner)
}
