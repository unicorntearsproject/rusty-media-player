//! State the parts of the host share: the capability bits, events read while waiting for something else, and the limits.
use crate::api;
use crate::events::{self, Ev};
use bucket_v0_sys::{self as sys, caps};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

/// The limits of the OS that the adapter respects, read once at the start (`limit_get`).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Largest cover art `now_playing_metadata` takes, bytes.
    pub art: usize,
    /// Longest title, artist or album, bytes.
    pub string: usize,
    /// Most bytes one `file_read_at` returns.
    pub read: usize,
    /// Most events one `events_wait` returns.
    pub events: usize,
    /// Most threads the app may have.
    pub threads: usize,
}

impl Limits {
    /// Read the limits from the OS (the documented minimums where it does not answer).
    pub fn read() -> Self {
        Self {
            art: api::limit(sys::limit::ART_BYTES, 8 << 20) as usize,
            string: api::limit(sys::limit::STRING_BYTES, 4096) as usize,
            read: api::limit(sys::limit::READ_PER_CALL, 1 << 20) as usize,
            events: api::limit(sys::limit::EVENTS_PER_WAIT, 256) as usize,
            threads: api::limit(sys::limit::THREADS, 64) as usize,
        }
    }
}

/// Shared between the host's parts and the driver.
pub struct Shared {
    /// The capability bits (updated by `CAPS_CHANGED`).
    pub caps: Cell<i64>,
    /// Events that arrived while the adapter waited for something else; the driver handles them first.
    pub backlog: RefCell<VecDeque<Ev>>,
    /// The limits.
    pub limits: Limits,
    /// Open handles of files that have no stable id, by the id the player was given for them.
    pub stash: RefCell<HashMap<String, i32>>,
    next_stash: Cell<u32>,
}

impl Shared {
    /// Read the capabilities and limits.
    pub fn new() -> Rc<Self> {
        Rc::new(Self::with(api::caps(), Limits::read()))
    }

    /// With given values.
    pub fn with(caps: i64, limits: Limits) -> Self {
        Self {
            caps: Cell::new(caps),
            backlog: RefCell::new(VecDeque::new()),
            limits,
            stash: RefCell::new(HashMap::new()),
            next_stash: Cell::new(0),
        }
    }

    /// True when the OS offers every bit of `bits` right now.
    pub fn has(&self, bits: i64) -> bool {
        self.caps.get() & bits == bits
    }

    /// True when this build has threads and the OS granted them.
    pub fn threads(&self) -> bool {
        self.has(caps::THREADS)
    }

    /// A new id for a stashed handle.
    pub fn stash_handle(&self, handle: i32) -> String {
        let n = self.next_stash.get() + 1;
        self.next_stash.set(n);
        let id = format!("rb-handle:{n}");
        self.stash.borrow_mut().insert(id.clone(), handle);
        id
    }

    /// Wait up to `max_us` for `IO_READY { 0 }` (a pending `kv_load` or `file_open_id` can be retried). Other events that arrive
    /// meanwhile are kept for the driver. Returns false if it timed out.
    pub fn wait_io(&self, max_us: i64) -> bool {
        let deadline = api::now_us() + max_us;
        loop {
            let now = api::now_us();
            if now >= deadline {
                return false;
            }
            let evs = events::wait((deadline - now).min(20_000), self.limits.events);
            let mut ready = false;
            let mut backlog = self.backlog.borrow_mut();
            for e in evs {
                ready |= matches!(e, Ev::IoReady { file: 0 });
                backlog.push_back(e);
            }
            if ready {
                return true;
            }
        }
    }
}
