//! Minimal task helpers for the cooperative, single-threaded model (see `docs/planning/PLAN.md` section 5).
//!
//! Futures in this project never rely on wakers: a task that cannot progress returns `Pending` and is
//! simply polled again on the next tick. So a no-op waker is enough.
use core::future::Future;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, Waker};

/// Poll `fut` once with a no-op waker.
pub fn poll_once<F: Future + ?Sized>(fut: Pin<&mut F>) -> Poll<F::Output> {
    fut.poll(&mut Context::from_waker(Waker::noop()))
}

/// Run a future to completion by polling it in a loop. Only for tests and native hosts whose futures
/// make progress on re-poll (they never wait on an external wake-up).
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    loop {
        if let Poll::Ready(v) = poll_once(fut.as_mut()) {
            return v;
        }
    }
}

/// A future that returns `Pending` once, then completes. Use it to give other tasks a turn.
pub fn yield_now() -> YieldNow {
    YieldNow(false)
}

/// See [`yield_now`].
#[derive(Debug)]
pub struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_spins_through_yields() {
        let v = block_on(async {
            for _ in 0..3 {
                yield_now().await;
            }
            7
        });
        assert_eq!(v, 7);
    }
}
