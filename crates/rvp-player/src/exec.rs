//! The cooperative executor: a list of tasks polled in spawn order on every tick.
//!
//! No wakers, no threads, no timers of its own. A task waiting for time uses [`sleep_until`], which asks the
//! host for a wake-up and returns `Pending` until the host clock has reached the deadline.
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;
use core::task::Poll;
use rvp_core::{Timestamp, task::poll_once};
use rvp_host::HostClock;

type BoxFuture = Pin<Box<dyn Future<Output = ()>>>;

/// Polls its tasks in spawn order, once each per [`Executor::poll_all`].
#[derive(Default)]
pub struct Executor {
    tasks: Vec<Option<BoxFuture>>,
}

impl Executor {
    /// An executor with no tasks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a task. It first runs on the next [`Executor::poll_all`].
    pub fn spawn(&mut self, fut: impl Future<Output = ()> + 'static) {
        self.tasks.push(Some(Box::pin(fut)));
    }

    /// Poll every live task once, in spawn order. Returns how many tasks are still alive.
    pub fn poll_all(&mut self) -> usize {
        let mut alive = 0;
        for slot in &mut self.tasks {
            if let Some(task) = slot {
                match poll_once(task.as_mut()) {
                    Poll::Ready(()) => *slot = None,
                    Poll::Pending => alive += 1,
                }
            }
        }
        if alive == 0 {
            self.tasks.clear();
        }
        alive
    }

    /// True when no tasks are alive.
    pub fn is_idle(&self) -> bool {
        self.tasks.iter().all(Option::is_none)
    }
}

/// Wait until the host clock reaches `until_us`.
pub async fn sleep_until(clock: &dyn HostClock, until_us: Timestamp) {
    core::future::poll_fn(|_| {
        if clock.now_us() >= until_us {
            Poll::Ready(())
        } else {
            clock.request_wake(until_us);
            Poll::Pending
        }
    })
    .await
}

/// Wait `us` microseconds of host time.
pub async fn sleep(clock: &dyn HostClock, us: Timestamp) {
    sleep_until(clock, clock.now_us() + us).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use core::cell::RefCell;
    use rvp_host::mock::FakeClock;

    type Log = Rc<RefCell<Vec<(Timestamp, &'static str)>>>;

    async fn ticker(clock: Rc<FakeClock>, log: Log, name: &'static str, period: Timestamp, end: Timestamp) {
        let mut next = period;
        while next <= end {
            sleep_until(&*clock, next).await;
            log.borrow_mut().push((clock.now_us(), name));
            next += period;
        }
    }

    #[test]
    fn tasks_wake_in_deadline_then_spawn_order_without_real_sleeping() {
        let started = std::time::Instant::now();
        let clock = Rc::new(FakeClock::new());
        let log: Log = Rc::default();
        let mut exec = Executor::new();
        let end = 10_000_000;
        exec.spawn(ticker(clock.clone(), log.clone(), "a", 100_000, end));
        exec.spawn(ticker(clock.clone(), log.clone(), "b", 250_000, end));
        exec.spawn(ticker(clock.clone(), log.clone(), "c", 1_000_000, end));

        let mut polls = 0;
        while exec.poll_all() > 0 {
            polls += 1;
            clock.advance_to_wake().expect("pending tasks must have asked for a wake-up");
        }

        // Expected: every (time, task) pair sorted by time, ties in spawn order.
        let mut want: Vec<(Timestamp, &str)> = Vec::new();
        for (name, period) in [("a", 100_000), ("b", 250_000), ("c", 1_000_000)] {
            let mut t = period;
            while t <= end {
                want.push((t, name));
                t += period;
            }
        }
        want.sort_by_key(|&(t, n)| (t, n)); // names a < b < c equal spawn order
        assert_eq!(*log.borrow(), want);
        assert_eq!(clock.now_us(), end);
        // One poll per distinct deadline (+ the final poll that lets tasks finish): far fewer than a busy loop.
        assert!(polls <= 160, "{polls} polls");
        assert!(started.elapsed().as_millis() < 100, "took {:?}", started.elapsed());
    }

    #[test]
    fn yield_lets_other_tasks_run() {
        let log: Rc<RefCell<Vec<u8>>> = Rc::default();
        let mut exec = Executor::new();
        for id in 0..2u8 {
            let log = log.clone();
            exec.spawn(async move {
                for step in 0..2u8 {
                    log.borrow_mut().push(id * 10 + step);
                    rvp_core::task::yield_now().await;
                }
            });
        }
        while exec.poll_all() > 0 {}
        assert_eq!(*log.borrow(), [0, 10, 1, 11]);
        assert!(exec.is_idle());
    }
}
