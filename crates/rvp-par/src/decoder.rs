//! A video decoder that runs on its own thread.
use rvp_core::par::SpinLock;
use rvp_core::{Error, Packet, Result, VideoDecoder, VideoFrame};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::Thread;

/// Builds the real decoder on the worker thread (decoders are not `Send`, so they cannot be built elsewhere).
pub type Factory = Box<dyn FnOnce() -> Result<Box<dyn VideoDecoder>> + Send>;

enum Cmd {
    Packet(Packet, u64),
    Flush(u64),
    Drain(u64),
}

#[derive(Default)]
struct Shared {
    inbox: SpinLock<VecDeque<Cmd>>,
    /// Decoded frames with the epoch they belong to.
    outbox: SpinLock<VecDeque<(u64, VideoFrame)>>,
    /// Bumped by every flush; output of older epochs is discarded.
    epoch: AtomicU64,
    /// Commands sent and not yet finished by the worker.
    pending: AtomicUsize,
    /// The first error the worker met and the caller has not seen.
    error: SpinLock<Option<String>>,
    /// The worker's thread handle, for `unpark`.
    thread: SpinLock<Option<Thread>>,
    stop: AtomicBool,
    /// Set by the worker when the decoder could not be built.
    failed: SpinLock<Option<String>>,
}

impl Shared {
    fn wake(&self) {
        if let Some(t) = self.thread.lock().as_ref() {
            t.unpark();
        }
    }
}

/// Decodes on a worker thread. `send_packet` queues the packet and returns at once; frames show up in
/// `receive_frame` as the worker finishes them. [`VideoDecoder::pending`] says how many commands are still in flight.
pub struct ThreadedVideoDecoder {
    sh: Arc<Shared>,
}

impl ThreadedVideoDecoder {
    /// Start the worker, which builds the decoder with `factory`. A failure to build shows up as an error from the
    /// first `send_packet`. Needs [`crate::available`].
    pub fn new(factory: Factory) -> Self {
        let sh = Arc::new(Shared::default());
        let worker = sh.clone();
        crate::spawn(move || run(worker, factory));
        Self { sh }
    }
}

fn run(sh: Arc<Shared>, factory: Factory) {
    *sh.thread.lock() = Some(std::thread::current());
    let mut dec = match factory() {
        Ok(d) => d,
        Err(e) => {
            *sh.failed.lock() = Some(e.to_string());
            // Drain commands so `pending` reaches zero and the caller is never stuck.
            while !sh.stop.load(Ordering::Acquire) {
                if sh.inbox.lock().pop_front().is_some() {
                    sh.pending.fetch_sub(1, Ordering::AcqRel);
                } else {
                    std::thread::park();
                }
            }
            return;
        }
    };
    // The epoch of the stream state inside the decoder: frames it finishes on its own (a decoder with a second thread)
    // belong to it, even if the caller has flushed since and the flush has not reached us yet.
    let mut state_epoch = sh.epoch.load(Ordering::Acquire);
    while !sh.stop.load(Ordering::Acquire) {
        let cmd = sh.inbox.lock().pop_front();
        let Some(cmd) = cmd else {
            // Read the count first: a decoder lowers it after publishing its frames.
            let busy = dec.pending() > 0;
            collect(&sh, &mut *dec, state_epoch);
            if busy {
                std::thread::park_timeout(std::time::Duration::from_micros(300));
            } else {
                std::thread::park();
            }
            continue;
        };
        match cmd {
            Cmd::Packet(p, epoch) => {
                if epoch == sh.epoch.load(Ordering::Acquire) {
                    state_epoch = epoch;
                    match dec.send_packet(&p) {
                        Ok(()) => {}
                        Err(e) => {
                            sh.error.lock().get_or_insert_with(|| e.to_string());
                        }
                    }
                    collect(&sh, &mut *dec, epoch);
                }
            }
            Cmd::Flush(epoch) => {
                dec.flush();
                state_epoch = epoch;
            }
            Cmd::Drain(epoch) => {
                if epoch == sh.epoch.load(Ordering::Acquire) {
                    state_epoch = epoch;
                    let _ = dec.drain();
                    collect(&sh, &mut *dec, epoch);
                }
            }
        }
        sh.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

fn collect(sh: &Shared, dec: &mut dyn VideoDecoder, epoch: u64) {
    while let Ok(Some(f)) = dec.receive_frame() {
        // A flush that happened meanwhile makes this frame stale; it is dropped when the caller looks at it.
        sh.outbox.lock().push_back((epoch, f));
    }
}

impl VideoDecoder for ThreadedVideoDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if let Some(e) = self.sh.failed.lock().clone() {
            return Err(Error::Unsupported(e));
        }
        let epoch = self.sh.epoch.load(Ordering::Acquire);
        self.sh.pending.fetch_add(1, Ordering::AcqRel);
        self.sh.inbox.lock().push_back(Cmd::Packet(packet.clone(), epoch));
        self.sh.wake();
        // Errors of earlier packets are reported on a later call; the packet itself is not at fault.
        match self.sh.error.lock().take() {
            Some(e) => Err(Error::Invalid(e)),
            None => Ok(()),
        }
    }

    fn receive_frame(&mut self) -> Result<Option<VideoFrame>> {
        let epoch = self.sh.epoch.load(Ordering::Acquire);
        let mut out = self.sh.outbox.lock();
        while let Some((e, f)) = out.pop_front() {
            if e == epoch {
                return Ok(Some(f));
            }
        }
        Ok(None)
    }

    fn flush(&mut self) {
        let epoch = self.sh.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        // Commands not yet started are dropped; the one running (if any) finishes and its output is discarded.
        let dropped = {
            let mut inbox = self.sh.inbox.lock();
            let n = inbox.len();
            inbox.clear();
            n
        };
        if dropped > 0 {
            self.sh.pending.fetch_sub(dropped, Ordering::AcqRel);
        }
        self.sh.outbox.lock().clear();
        self.sh.pending.fetch_add(1, Ordering::AcqRel);
        self.sh.inbox.lock().push_back(Cmd::Flush(epoch));
        self.sh.wake();
    }

    fn drain(&mut self) -> Result<()> {
        let epoch = self.sh.epoch.load(Ordering::Acquire);
        self.sh.pending.fetch_add(1, Ordering::AcqRel);
        self.sh.inbox.lock().push_back(Cmd::Drain(epoch));
        self.sh.wake();
        Ok(())
    }

    fn pending(&self) -> usize {
        self.sh.pending.load(Ordering::Acquire)
    }
}

impl Drop for ThreadedVideoDecoder {
    fn drop(&mut self) {
        self.sh.stop.store(true, Ordering::Release);
        self.sh.wake();
    }
}
