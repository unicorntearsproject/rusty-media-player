//! The worker pool.
use rvp_core::par::{Parallel, SpinLock};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::Thread;

thread_local! {
    /// Set on a thread that must never wait for others (a browser's main thread): `run` then does all the work itself.
    static NO_WAIT: Cell<bool> = const { Cell::new(false) };
}

/// Mark the calling thread as one that must not wait for pool threads (the UI thread of a page). Its `run` calls
/// execute serially, so a descheduled worker can never hold up a frame.
pub fn mark_ui_thread() {
    NO_WAIT.with(|f| f.set(true));
}

/// One `run` call: tasks `0..n` handed out through `next`, counted in `done`.
struct Job {
    /// The caller's closure with its lifetime erased; valid until `done == n` (the caller waits for that).
    f: *const (dyn Fn(usize) + Sync),
    n: usize,
    next: AtomicUsize,
    done: AtomicUsize,
}

// SAFETY: `f` is `Sync` and only dereferenced while the owning `run` call is still waiting for the job.
unsafe impl Send for Job {}
// SAFETY: as above.
unsafe impl Sync for Job {}

impl Job {
    /// Run tasks until none are left to hand out.
    fn work(&self) {
        loop {
            let i = self.next.fetch_add(1, Ordering::AcqRel);
            if i >= self.n {
                return;
            }
            // SAFETY: tasks are only handed out while `run` waits for `done == n`, so the closure is alive.
            unsafe { (*self.f)(i) };
            self.done.fetch_add(1, Ordering::AcqRel);
        }
    }
}

struct Shared {
    jobs: SpinLock<Vec<Arc<Job>>>,
    threads: SpinLock<Vec<Thread>>,
    stop: AtomicBool,
}

/// A fixed set of worker threads. `Pool::new(n)` plans `n - 1` workers (started on first use); the thread calling `run`
/// is the `n`-th.
pub struct Pool {
    shared: Arc<Shared>,
    size: usize,
    /// The workers are started by the first `run` that has something to share, so a page that never needs them (or has
    /// not got to it yet) pays nothing at start-up.
    started: AtomicBool,
}

impl Pool {
    /// Start a pool of `size` threads in total (the caller of `run` counts as one). Needs [`crate::available`].
    pub fn new(size: usize) -> Self {
        let size = size.max(1);
        let shared = Arc::new(Shared {
            jobs: SpinLock::new(Vec::new()),
            threads: SpinLock::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        Self { shared, size, started: AtomicBool::new(false) }
    }

    fn start(&self) {
        // An atomic flag rather than `Once`: a second caller must never wait (it may be the browser's main thread).
        if self.started.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            for _ in 1..self.size {
                let sh = self.shared.clone();
                crate::spawn(move || worker(sh));
            }
        }
    }

    /// Make this pool the one the kernels use (`rvp_core::par`). Leaks the pool: it lives as long as the process.
    pub fn install(self) -> &'static Pool {
        let leaked: &'static Pool = Box::leak(Box::new(self));
        rvp_core::par::install(leaked);
        leaked
    }

    fn wake_all(&self) {
        for t in self.shared.threads.lock().iter() {
            t.unpark();
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.wake_all();
    }
}

fn worker(sh: Arc<Shared>) {
    sh.threads.lock().push(std::thread::current());
    while !sh.stop.load(Ordering::Acquire) {
        let job = sh.jobs.lock().iter().find(|j| j.next.load(Ordering::Acquire) < j.n).cloned();
        match job {
            Some(j) => j.work(),
            None => std::thread::park(),
        }
    }
}

impl Parallel for Pool {
    fn threads(&self) -> usize {
        self.size
    }

    fn run(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        if n == 0 {
            return;
        }
        if NO_WAIT.with(Cell::get) {
            (0..n).for_each(f);
            return;
        }
        self.start();
        // SAFETY: the closure outlives this call, and `run` does not return before `done == n`, after which no
        // thread starts another task of this job.
        let f: *const (dyn Fn(usize) + Sync) = unsafe { std::mem::transmute(f) };
        let job = Arc::new(Job { f, n, next: AtomicUsize::new(0), done: AtomicUsize::new(0) });
        self.shared.jobs.lock().push(job.clone());
        self.wake_all();
        job.work();
        while job.done.load(Ordering::Acquire) < n {
            std::hint::spin_loop();
        }
        self.shared.jobs.lock().retain(|j| !Arc::ptr_eq(j, &job));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn runs_every_task_once_and_nests() {
        let pool = Arc::new(Pool::new(4));
        let sum = AtomicU64::new(0);
        pool.run(100, &|i| {
            sum.fetch_add(i as u64, Ordering::Relaxed);
        });
        assert_eq!(sum.load(Ordering::Relaxed), 4950);
        // Two callers at once (as the decode thread and the UI thread are).
        let p2 = pool.clone();
        let t = std::thread::spawn(move || {
            let s = AtomicU64::new(0);
            for _ in 0..200 {
                p2.run(16, &|i| {
                    s.fetch_add(i as u64, Ordering::Relaxed);
                });
            }
            s.load(Ordering::Relaxed)
        });
        let s = AtomicU64::new(0);
        for _ in 0..200 {
            pool.run(16, &|i| {
                s.fetch_add(i as u64, Ordering::Relaxed);
            });
        }
        assert_eq!(s.load(Ordering::Relaxed), 200 * 120);
        assert_eq!(t.join().unwrap(), 200 * 120);
    }
}
