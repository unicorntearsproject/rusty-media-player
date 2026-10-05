//! MSB-first bit reader over an RBSP (emulation prevention already removed) with Exp-Golomb codes.
use crate::error::{Error, Result};

/// Reads bits from a byte slice. Reads past the end fail with [`Error::Truncated`]; the `peek` family
/// returns zero bits past the end so table-driven decoders need no bounds checks, and callers verify with
/// [`BitReader::overrun`] afterwards.
#[derive(Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    stop_bit: usize,
}

impl<'a> BitReader<'a> {
    /// Start reading `data` at bit 0.
    pub fn new(data: &'a [u8]) -> Self {
        // The rbsp_stop_one_bit is the last 1 bit of the data.
        let mut stop_bit = 0;
        for (i, &b) in data.iter().enumerate().rev() {
            if b != 0 {
                stop_bit = i * 8 + 7 - b.trailing_zeros() as usize;
                break;
            }
        }
        Self { data, pos: 0, stop_bit }
    }

    /// Current position in bits.
    #[inline]
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Total size in bits.
    #[inline]
    pub fn len_bits(&self) -> usize {
        self.data.len() * 8
    }

    /// Bits left (zero if overrun).
    #[inline]
    pub fn bits_left(&self) -> usize {
        self.len_bits().saturating_sub(self.pos)
    }

    /// True if a read or skip went past the end of the data.
    #[inline]
    pub fn overrun(&self) -> bool {
        self.pos > self.len_bits()
    }

    /// The underlying bytes.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Next 32 bits without consuming them (zeros past the end).
    #[inline]
    pub fn peek32(&self) -> u32 {
        let byte = self.pos >> 3;
        let sh = (self.pos & 7) as u32;
        if byte + 8 <= self.data.len() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&self.data[byte..byte + 8]);
            ((u64::from_be_bytes(b) << sh) >> 32) as u32
        } else {
            let mut v = 0u64;
            for i in 0..8 {
                v = (v << 8) | self.data.get(byte + i).copied().unwrap_or(0) as u64;
            }
            ((v << sh) >> 32) as u32
        }
    }

    /// Next `n` bits (0 to 32) without consuming them.
    #[inline]
    pub fn peek_bits(&self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.peek32() >> (32 - n) }
    }

    /// Consume `n` bits without checking the bound (see [`BitReader::overrun`]).
    #[inline]
    pub fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    /// Read `n` bits (0 to 32).
    #[inline]
    pub fn read_bits(&mut self, n: u32) -> Result<u32> {
        if self.pos + n as usize > self.len_bits() {
            return Err(Error::Truncated);
        }
        let v = self.peek_bits(n);
        self.pos += n as usize;
        Ok(v)
    }

    /// Read one bit.
    #[inline]
    pub fn read_bit(&mut self) -> Result<u32> {
        self.read_bits(1)
    }

    /// Read one bit as a flag.
    #[inline]
    pub fn read_flag(&mut self) -> Result<bool> {
        Ok(self.read_bits(1)? != 0)
    }

    /// `ue(v)`: unsigned Exp-Golomb.
    #[inline]
    pub fn read_ue(&mut self) -> Result<u32> {
        let w = self.peek32();
        if w != 0 {
            let lz = w.leading_zeros();
            if lz <= 15 {
                // Whole code fits in the 32-bit window.
                let len = 2 * lz + 1;
                if self.pos + len as usize > self.len_bits() {
                    return Err(Error::Truncated);
                }
                self.pos += len as usize;
                return Ok((w >> (32 - len)) - 1);
            }
        }
        // Long code: count the zeros explicitly.
        let mut lz = 0u32;
        while self.read_bits(1)? == 0 {
            lz += 1;
            if lz > 31 {
                return Err(Error::Invalid("exp-golomb code too long"));
            }
        }
        let rest = self.read_bits(lz)?;
        Ok(((1u64 << lz) - 1 + rest as u64) as u32)
    }

    /// `se(v)`: signed Exp-Golomb.
    #[inline]
    pub fn read_se(&mut self) -> Result<i32> {
        let k = self.read_ue()?;
        let m = ((k as u64 + 1) >> 1) as i32;
        Ok(if k & 1 == 1 { m } else { m.wrapping_neg() })
    }

    /// `te(v)`: truncated Exp-Golomb with the given maximum value.
    #[inline]
    pub fn read_te(&mut self, max: u32) -> Result<u32> {
        if max > 1 { self.read_ue() } else { Ok(1 - self.read_bits(1)?) }
    }

    /// True if the position is on a byte boundary.
    #[inline]
    pub fn is_byte_aligned(&self) -> bool {
        self.pos & 7 == 0
    }

    /// Advance to the next byte boundary.
    pub fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    /// `more_rbsp_data()`: true if there is data before the `rbsp_stop_one_bit`.
    #[inline]
    pub fn more_rbsp_data(&self) -> bool {
        self.pos < self.stop_bit
    }

    /// The bytes from the current (byte-aligned) position to the end.
    pub fn remaining_bytes(&self) -> &'a [u8] {
        self.data.get(self.pos.div_ceil(8)..).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_basics() {
        // 1 | 010 | 011 | 00100 | 00101 | 00110 | 00111  -> ue 0,1,2,3,4,5,6
        let d = [0b1010_0110, 0b0100_0010, 0b1001_1000, 0b1110_0000];
        let mut r = BitReader::new(&d);
        for want in 0..7 {
            assert_eq!(r.read_ue().unwrap(), want);
        }
    }

    #[test]
    fn se_mapping() {
        // ue 1,2,3,4 -> se 1,-1,2,-2
        let d = [0b0100_1100, 0b1000_0101, 0b0000_0000];
        let mut r = BitReader::new(&d);
        assert_eq!(r.read_se().unwrap(), 1);
        assert_eq!(r.read_se().unwrap(), -1);
        assert_eq!(r.read_se().unwrap(), 2);
        assert_eq!(r.read_se().unwrap(), -2);
    }

    #[test]
    fn long_codes_and_truncation() {
        // 20 zeros, a one, 20 bits of payload: ue = 2^20 - 1 + payload
        let mut w = crate::bitstream::BitWriter::new();
        w.put_ue(1_234_567);
        w.put_ue(u32::MAX - 1);
        w.put_bits(1, 1);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_ue().unwrap(), 1_234_567);
        assert_eq!(r.read_ue().unwrap(), u32::MAX - 1);
        let mut r = BitReader::new(&[0, 0]);
        assert!(r.read_ue().is_err());
        let mut r = BitReader::new(&[0xFF]);
        assert!(r.read_bits(9).is_err());
    }

    #[test]
    fn more_rbsp_data_finds_the_stop_bit() {
        let d = [0b1011_0000];
        let mut r = BitReader::new(&d);
        assert!(r.more_rbsp_data());
        r.read_bits(3).unwrap();
        assert!(!r.more_rbsp_data());
    }
}
