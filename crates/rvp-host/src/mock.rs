//! Mock host pieces for tests.
use crate::{HostClock, HostError, InputEvent, InputEvents, Source};
use alloc::{collections::VecDeque, string::String, vec::Vec};
use core::cell::Cell;
use rvp_core::Timestamp;

/// A clock that only moves when the test says so. Wake requests are remembered (earliest wins) so a
/// driver can jump virtual time straight to the next deadline.
#[derive(Debug, Default)]
pub struct FakeClock {
    now: Cell<Timestamp>,
    wake: Cell<Option<Timestamp>>,
}

impl FakeClock {
    /// Start at time 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance by `us` microseconds.
    pub fn advance(&self, us: Timestamp) {
        self.set_now(self.now.get() + us);
    }

    /// Jump to the earliest pending wake request (if any, and in the future). Returns the new time.
    pub fn advance_to_wake(&self) -> Option<Timestamp> {
        let t = self.wake.get()?;
        self.set_now(t.max(self.now.get()));
        Some(self.now.get())
    }

    /// The earliest pending wake request, if any.
    pub fn requested_wake(&self) -> Option<Timestamp> {
        self.wake.get()
    }

    fn set_now(&self, t: Timestamp) {
        self.now.set(t);
        if self.wake.get().is_some_and(|w| w <= t) {
            self.wake.set(None);
        }
    }
}

impl HostClock for FakeClock {
    fn now_us(&self) -> Timestamp {
        self.now.get()
    }

    fn request_wake(&self, at_us: Timestamp) {
        if at_us > self.now.get() {
            self.wake.set(Some(self.wake.get().map_or(at_us, |w| w.min(at_us))));
        }
    }
}

/// An in-memory [`Source`] that can misbehave on purpose: short reads and `Pending` polls.
#[derive(Debug, Clone)]
pub struct MemSource {
    data: Vec<u8>,
    name: String,
    /// Longest read to return per call (1 = byte at a time).
    pub max_read: usize,
    /// Number of times each read returns `Pending` before completing.
    pub pending_polls: u32,
}

impl MemSource {
    /// A well-behaved source over `data`.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data, name: String::from("mem"), max_read: usize::MAX, pending_polls: 0 }
    }

    /// Return at most `n` bytes per read.
    pub fn with_max_read(mut self, n: usize) -> Self {
        self.max_read = n.max(1);
        self
    }

    /// Make every read yield `Pending` `n` times first.
    pub fn with_pending_polls(mut self, n: u32) -> Self {
        self.pending_polls = n;
        self
    }
}

impl Source for MemSource {
    async fn size(&self) -> Option<u64> {
        Some(self.data.len() as u64)
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, HostError> {
        for _ in 0..self.pending_polls {
            rvp_core::task::yield_now().await;
        }
        let start = (offset as usize).min(self.data.len());
        let n = buf.len().min(self.max_read).min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(n)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// A scripted input queue.
#[derive(Debug, Default)]
pub struct ScriptedInput(pub VecDeque<InputEvent>);

impl InputEvents for ScriptedInput {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_only_moves_on_advance() {
        let c = FakeClock::new();
        assert_eq!(c.now_us(), 0);
        c.advance(1500);
        c.request_wake(4000);
        c.request_wake(3000);
        c.request_wake(100); // in the past: ignored
        assert_eq!((c.now_us(), c.requested_wake()), (1500, Some(3000)));
        assert_eq!(c.advance_to_wake(), Some(3000));
        assert_eq!(c.requested_wake(), None);
    }

    #[test]
    fn mem_source_short_reads_and_pending() {
        let mut s = MemSource::new((0..10).collect()).with_max_read(3).with_pending_polls(2);
        let mut buf = [0u8; 8];
        let n = rvp_core::task::block_on(s.read_at(4, &mut buf)).unwrap();
        assert_eq!((n, &buf[..n]), (3, &[4u8, 5, 6][..]));
        assert_eq!(rvp_core::task::block_on(s.read_at(10, &mut buf)).unwrap(), 0);
    }

    #[test]
    fn scripted_input_drains_in_order() {
        let mut i = ScriptedInput::default();
        i.0.push_back(InputEvent::Focus(true));
        i.0.push_back(InputEvent::Focus(false));
        assert_eq!(i.poll(), Some(InputEvent::Focus(true)));
        assert_eq!(i.poll(), Some(InputEvent::Focus(false)));
        assert_eq!(i.poll(), None);
    }
}
