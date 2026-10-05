//! Optional data parallelism for the pixel and codec kernels.
//!
//! A host that has threads (native, or a browser page built with shared memory) installs a [`Parallel`] with
//! [`install`]; kernels then call [`for_each`] or [`for_each_chunk_mut`] and the work is spread over the
//! pool. Without one, or for a single task, everything runs inline on the calling thread, so the single-threaded
//! build behaves exactly as before.
//!
//! Callers never block in the OS sense: [`Parallel::run`] returns when every task has finished and the calling
//! thread takes part in the work, which makes it safe to call from a browser's main thread (where waiting on an
//! atomic is not allowed) for the short tasks the kernels use.
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A thread pool the kernels can run tasks on.
pub trait Parallel: Send + Sync {
    /// Number of threads that take part in [`run`](Parallel::run), the caller included.
    fn threads(&self) -> usize;
    /// Run `f(0)`, `f(1)`, ... `f(n - 1)` on the pool and return when all have finished.
    fn run(&self, n: usize, f: &(dyn Fn(usize) + Sync));
}

struct Slot {
    ready: AtomicBool,
    pool: UnsafeCell<Option<&'static dyn Parallel>>,
}

// SAFETY: `pool` is written once, before `ready` is set with Release, and only read after `ready` is seen with Acquire.
unsafe impl Sync for Slot {}

static SLOT: Slot = Slot { ready: AtomicBool::new(false), pool: UnsafeCell::new(None) };

/// Install the process-wide pool. Only the first call has an effect; returns whether this one did.
pub fn install(pool: &'static dyn Parallel) -> bool {
    static CLAIM: AtomicBool = AtomicBool::new(false);
    if CLAIM.swap(true, Ordering::AcqRel) {
        return false;
    }
    // SAFETY: the claim above makes this the only writer, and no reader looks before `ready` is set.
    unsafe { *SLOT.pool.get() = Some(pool) };
    SLOT.ready.store(true, Ordering::Release);
    true
}

fn pool() -> Option<&'static dyn Parallel> {
    if SLOT.ready.load(Ordering::Acquire) {
        // SAFETY: written before `ready`, never changed afterwards.
        unsafe { *SLOT.pool.get() }
    } else {
        None
    }
}

/// Threads available to kernels (1 when no pool is installed).
pub fn threads() -> usize {
    pool().map_or(1, |p| p.threads().max(1))
}

/// Run `f(0..n)`, in parallel when a pool is installed. Returns when all tasks have finished.
pub fn for_each(n: usize, f: &(dyn Fn(usize) + Sync)) {
    match pool() {
        Some(p) if n > 1 && p.threads() > 1 => p.run(n, f),
        _ => (0..n).for_each(f),
    }
}

/// A spin lock: for the few instructions of handing a value between threads. It never puts a thread to sleep, so it
/// is safe on a browser's main thread.
pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the lock serialises all access to `value`.
unsafe impl<T: Send> Sync for SpinLock<T> {}
// SAFETY: moving the lock moves the value.
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T: Default> Default for SpinLock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> SpinLock<T> {
    /// A new unlocked lock.
    pub const fn new(value: T) -> Self {
        Self { locked: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    /// Take the lock, spinning until it is free.
    pub fn lock(&self) -> SpinGuard<'_, T> {
        while self.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinGuard { lock: self }
    }
}

/// Holds a [`SpinLock`]; releases it on drop.
pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard holds the lock.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard holds the lock exclusively.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

/// Split `data` into chunks of `chunk_len` elements (the last may be shorter) and run `f(chunk index, chunk)` on
/// each, in parallel when a pool is installed.
pub fn for_each_chunk_mut<T: Send>(data: &mut [T], chunk_len: usize, f: &(dyn Fn(usize, &mut [T]) + Sync)) {
    let chunk_len = chunk_len.max(1);
    if data.len() <= chunk_len || threads() == 1 {
        for (i, c) in data.chunks_mut(chunk_len).enumerate() {
            f(i, c);
        }
        return;
    }
    let slots: Vec<SpinLock<Option<&mut [T]>>> =
        data.chunks_mut(chunk_len).map(|c| SpinLock::new(Some(c))).collect();
    for_each(slots.len(), &|i| {
        let chunk = slots[i].lock().take();
        if let Some(c) = chunk {
            f(i, c);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicUsize;

    #[test]
    fn serial_fallback_runs_everything() {
        let n = AtomicUsize::new(0);
        for_each(10, &|i| {
            n.fetch_add(i + 1, Ordering::Relaxed);
        });
        assert_eq!(n.load(Ordering::Relaxed), 55);
        let mut v = alloc::vec![0u32; 10];
        for_each_chunk_mut(&mut v, 3, &|i, c| c.iter_mut().for_each(|x| *x = i as u32));
        assert_eq!(v, [0, 0, 0, 1, 1, 1, 2, 2, 2, 3]);
    }

    #[test]
    fn spin_lock_guards_a_value() {
        let l = SpinLock::new(5);
        *l.lock() += 1;
        assert_eq!(*l.lock(), 6);
    }
}
