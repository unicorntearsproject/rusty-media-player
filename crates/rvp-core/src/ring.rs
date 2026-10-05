//! A fixed-capacity single-owner ring buffer for sample or byte queues.
use alloc::vec::Vec;

/// FIFO ring over `Copy` items with a fixed capacity.
#[derive(Debug, Clone)]
pub struct RingBuffer<T: Copy + Default> {
    buf: Vec<T>,
    head: usize,
    len: usize,
}

impl<T: Copy + Default> RingBuffer<T> {
    /// Create a ring that holds up to `capacity` items.
    pub fn new(capacity: usize) -> Self {
        Self { buf: alloc::vec![T::default(); capacity], head: 0, len: 0 }
    }

    /// Total capacity.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Items currently stored.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True if empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Free space.
    pub fn free(&self) -> usize {
        self.capacity() - self.len
    }

    /// Append as many items from `src` as fit; returns how many were taken.
    pub fn push_slice(&mut self, src: &[T]) -> usize {
        let n = src.len().min(self.free());
        let cap = self.capacity();
        for (i, item) in src[..n].iter().enumerate() {
            let idx = (self.head + self.len + i) % cap;
            self.buf[idx] = *item;
        }
        self.len += n;
        n
    }

    /// Remove up to `dst.len()` items into `dst`; returns how many were copied.
    pub fn pop_slice(&mut self, dst: &mut [T]) -> usize {
        let n = dst.len().min(self.len);
        let cap = self.capacity();
        for (i, slot) in dst[..n].iter_mut().enumerate() {
            *slot = self.buf[(self.head + i) % cap];
        }
        self.head = (self.head + n) % cap.max(1);
        self.len -= n;
        n
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_bounds() {
        let mut r = RingBuffer::<u8>::new(4);
        assert_eq!(r.push_slice(&[1, 2, 3]), 3);
        let mut out = [0u8; 2];
        assert_eq!(r.pop_slice(&mut out), 2);
        assert_eq!(out, [1, 2]);
        assert_eq!(r.push_slice(&[4, 5, 6, 7]), 3); // only 3 free
        let mut all = [0u8; 8];
        assert_eq!(r.pop_slice(&mut all), 4);
        assert_eq!(&all[..4], &[3, 4, 5, 6]);
        assert!(r.is_empty());
    }
}
