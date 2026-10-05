//! MSB-first bit writer with Exp-Golomb codes: the encoder-side counterpart of [`super::BitReader`].
use alloc::vec::Vec;

/// Accumulates bits into bytes.
#[derive(Debug, Default, Clone)]
pub struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    /// An empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of bits written so far.
    pub fn bit_len(&self) -> usize {
        self.buf.len() * 8 + self.nbits as usize
    }

    /// True if the next bit is at a byte boundary.
    pub fn is_byte_aligned(&self) -> bool {
        self.nbits % 8 == 0
    }

    /// Write the low `n` bits of `value` (`n` from 0 to 32).
    pub fn put_bits(&mut self, value: u32, n: u32) {
        debug_assert!(n <= 32);
        if n == 0 {
            return;
        }
        let v = if n == 32 { value as u64 } else { (value as u64) & ((1u64 << n) - 1) };
        self.acc = (self.acc << n) | v;
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.buf.push((self.acc >> self.nbits) as u8);
        }
        self.acc &= (1u64 << self.nbits) - 1;
    }

    /// Write one bit.
    pub fn put_bit(&mut self, b: bool) {
        self.put_bits(b as u32, 1);
    }

    /// `ue(v)`.
    pub fn put_ue(&mut self, v: u32) {
        let x = v as u64 + 1;
        let len = 64 - x.leading_zeros(); // number of significant bits of v+1
        let zeros = len - 1;
        if zeros > 0 {
            self.put_bits(0, zeros.min(32));
            if zeros > 32 {
                self.put_bits(0, zeros - 32);
            }
        }
        // `len` bits of x (at most 33): split.
        if len > 32 {
            self.put_bits((x >> 32) as u32, len - 32);
            self.put_bits(x as u32, 32);
        } else {
            self.put_bits(x as u32, len);
        }
    }

    /// `se(v)`.
    pub fn put_se(&mut self, v: i32) {
        let k =
            if v > 0 { (v as u32).wrapping_mul(2).wrapping_sub(1) } else { v.unsigned_abs().wrapping_mul(2) };
        self.put_ue(k);
    }

    /// `te(v)` with the given maximum value.
    pub fn put_te(&mut self, v: u32, max: u32) {
        if max > 1 {
            self.put_ue(v);
        } else {
            self.put_bit(v == 0);
        }
    }

    /// Pad with zero bits to a byte boundary.
    pub fn align_zero(&mut self) {
        if self.nbits % 8 != 0 {
            self.put_bits(0, 8 - self.nbits % 8);
        }
    }

    /// `rbsp_trailing_bits()`: a one bit, then zeros to the byte boundary.
    pub fn put_trailing_bits(&mut self) {
        self.put_bit(true);
        self.align_zero();
    }

    /// Append whole bytes (the writer must be byte aligned).
    pub fn put_bytes(&mut self, bytes: &[u8]) {
        debug_assert!(self.is_byte_aligned());
        for &b in bytes {
            self.put_bits(b as u32, 8);
        }
    }

    /// Finish and return the bytes (a partial last byte is zero padded).
    pub fn into_bytes(mut self) -> Vec<u8> {
        self.align_zero();
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitstream::BitReader;

    #[test]
    fn roundtrip_mixed() {
        let mut w = BitWriter::new();
        w.put_bits(0b101, 3);
        for v in [0u32, 1, 2, 7, 100, 65535, 1 << 20] {
            w.put_ue(v);
        }
        for v in [0i32, 1, -1, 5, -5, 1000, -1000, i32::MAX, i32::MIN + 1] {
            w.put_se(v);
        }
        w.put_te(1, 1);
        w.put_te(0, 1);
        w.put_te(9, 12);
        w.put_trailing_bits();
        let b = w.into_bytes();
        let mut r = BitReader::new(&b);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        for v in [0u32, 1, 2, 7, 100, 65535, 1 << 20] {
            assert_eq!(r.read_ue().unwrap(), v);
        }
        for v in [0i32, 1, -1, 5, -5, 1000, -1000, i32::MAX, i32::MIN + 1] {
            assert_eq!(r.read_se().unwrap(), v);
        }
        assert_eq!(r.read_te(1).unwrap(), 1);
        assert_eq!(r.read_te(1).unwrap(), 0);
        assert_eq!(r.read_te(12).unwrap(), 9);
        assert!(!r.more_rbsp_data());
    }
}
