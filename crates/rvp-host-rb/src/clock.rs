//! The clock: `time_now_us`, and `request_wake` collected for the driver, which turns it into the timeout of `events_wait`.
use crate::api;
use rvp_core::Timestamp;
use rvp_host::HostClock;
use std::cell::Cell;

/// `HostClock` over the OS clock.
pub struct RbClock {
    wake: Cell<Timestamp>,
}

impl RbClock {
    /// A clock with no wake request.
    pub fn new() -> Self {
        Self { wake: Cell::new(i64::MAX) }
    }

    /// The earliest wake request since the last call (and forget it).
    pub fn take_wake(&self) -> Option<Timestamp> {
        let w = self.wake.replace(i64::MAX);
        (w != i64::MAX).then_some(w)
    }
}

impl Default for RbClock {
    fn default() -> Self {
        Self::new()
    }
}

impl HostClock for RbClock {
    fn now_us(&self) -> Timestamp {
        api::now_us()
    }

    fn request_wake(&self, at_us: Timestamp) {
        self.wake.set(self.wake.get().min(at_us));
    }
}
