//! MSB-first bit reader over an RBSP (emulation prevention already removed) with Exp-Golomb codes.
use crate::{Error, Result};

/// Reads bits from a byte slice; a read past the end fails with [`Error::Truncated`].
#[derive(Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    /// Start reading `data` at bit 0.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Position in bits.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Bits left.
    pub fn bits_left(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }

    /// Read `n` bits (0 to 32).
    pub fn bits(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        if self.pos + n as usize > self.data.len() * 8 {
            return Err(Error::Truncated);
        }
        let mut v = 0u64;
        for _ in 0..n {
            let byte = self.data[self.pos >> 3];
            v = (v << 1) | ((byte >> (7 - (self.pos & 7))) & 1) as u64;
            self.pos += 1;
        }
        Ok(v as u32)
    }

    /// One bit as a flag.
    pub fn flag(&mut self) -> Result<bool> {
        Ok(self.bits(1)? != 0)
    }

    /// Skip `n` bits.
    pub fn skip(&mut self, n: usize) -> Result<()> {
        if self.pos + n > self.data.len() * 8 {
            return Err(Error::Truncated);
        }
        self.pos += n;
        Ok(())
    }

    /// `ue(v)`.
    pub fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0u32;
        while !self.flag()? {
            zeros += 1;
            if zeros > 31 {
                return Err(Error::Invalid("exp-golomb code too long"));
            }
        }
        let rest = self.bits(zeros)? as u64;
        Ok(((1u64 << zeros) - 1 + rest) as u32)
    }

    /// `se(v)`.
    pub fn se(&mut self) -> Result<i32> {
        let k = self.ue()? as u64;
        let m = k.div_ceil(2) as i32;
        Ok(if k & 1 == 1 { m } else { -m })
    }

    /// Advance to the next byte boundary.
    pub fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    /// `byte_alignment()`: a one bit, then zero bits to the byte boundary.
    pub fn byte_alignment(&mut self) -> Result<()> {
        if !self.flag()? {
            return Err(Error::Invalid("byte_alignment starts with a zero bit"));
        }
        self.align();
        Ok(())
    }

    /// Byte offset of the current position (rounded up).
    pub fn byte_pos(&self) -> usize {
        self.pos.div_ceil(8)
    }
}

/// `Ceil(Log2(n))` for n >= 1.
pub fn ceil_log2(n: u32) -> u32 {
    if n <= 1 { 0 } else { 32 - (n - 1).leading_zeros() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_and_flags() {
        // 1 | 010 | 011 | 00100 | 00101: ue 0 1 2 3 4
        let d = [0b1010_0110, 0b0100_0010, 0b1000_0000];
        let mut r = BitReader::new(&d);
        for want in 0..5 {
            assert_eq!(r.ue().unwrap(), want);
        }
        // ue 1,2,3,4 -> se 1,-1,2,-2
        let d = [0b0100_1100, 0b1000_0101, 0b0000_0000];
        let mut r = BitReader::new(&d);
        assert_eq!([r.se().unwrap(), r.se().unwrap(), r.se().unwrap(), r.se().unwrap()], [1, -1, 2, -2]);
        let mut r = BitReader::new(&[0xff]);
        assert!(r.flag().unwrap());
        assert_eq!(r.bits(7).unwrap(), 0x7f);
        assert_eq!(r.bits(1), Err(Error::Truncated));
    }

    #[test]
    fn logs() {
        assert_eq!([ceil_log2(1), ceil_log2(2), ceil_log2(3), ceil_log2(4), ceil_log2(5), ceil_log2(510)], [0, 1, 2, 2, 3, 9]);
    }
}
