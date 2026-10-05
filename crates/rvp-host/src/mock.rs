//! Mock host pieces for tests.
use crate::{HostClock, InputEvent, InputEvents};
use alloc::collections::VecDeque;
use core::cell::Cell;
use rvp_core::Timestamp;

/// A clock that only moves when the test says so.
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
        self.now.set(self.now.get() + us);
    }

    /// The most recent wake request, if any.
    pub fn requested_wake(&self) -> Option<Timestamp> {
        self.wake.get()
    }
}

impl HostClock for FakeClock {
    fn now_us(&self) -> Timestamp {
        self.now.get()
    }

    fn request_wake(&self, at_us: Timestamp) {
        self.wake.set(Some(at_us));
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
        assert_eq!((c.now_us(), c.requested_wake()), (1500, Some(4000)));
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
