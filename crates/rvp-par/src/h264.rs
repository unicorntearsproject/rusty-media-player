//! H.264 with the reconstruction on a thread of its own: while it reconstructs and deblocks picture N, the decoder
//! thread already parses picture N + 1.
use rvp_codec_h264::decoder::Frame;
use rvp_codec_h264::decoder::recon::{ReconEvent, ReconExecutor, Reconstructor};
use rvp_core::par::SpinLock;
use rvp_core::{StreamInfo, VideoDecoder};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::Thread;
use std::time::Duration;

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
}

/// A [`ReconExecutor`] running a [`Reconstructor`] on its own thread. Only for threads that may wait (the decoder
/// thread, never a browser's main thread).
pub struct ThreadedRecon {
    sh: Arc<Shared>,
}

impl ThreadedRecon {
    /// Start the reconstruction thread.
    pub fn new() -> Self {
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
    }
}

impl ReconExecutor for ThreadedRecon {
    fn submit(&mut self, ev: ReconEvent) {
        let is_picture = matches!(ev, ReconEvent::Picture(_));
        if is_picture {
            // Do not run arbitrarily far ahead of the reconstruction (each picture holds its macroblock data).
            while self.sh.pictures.load(Ordering::Acquire) >= MAX_PICTURES_QUEUED {
                std::thread::park_timeout(Duration::from_micros(200));
            }
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
        while self.sh.queued.load(Ordering::Acquire) > 0 {
            std::thread::park_timeout(Duration::from_micros(200));
        }
    }
}

impl Drop for ThreadedRecon {
    fn drop(&mut self) {
        self.sh.stop.store(true, Ordering::Release);
        self.wake();
    }
}

/// An H.264 decoder with its reconstruction on a second thread. Build it on the thread that will drive it (a
/// [`crate::ThreadedVideoDecoder`] worker, or a native thread that may wait).
pub fn h264_pipelined(info: &StreamInfo) -> rvp_core::Result<Box<dyn VideoDecoder>> {
    rvp_codec_h264::h264_decoder_with(info, Some(Box::new(ThreadedRecon::new())))
}
