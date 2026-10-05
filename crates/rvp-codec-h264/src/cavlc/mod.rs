//! CAVLC residual coding (clause 9.2): reading and writing `residual_block_cavlc()`.
//!
//! The VLC tables in [`tables`] are generated from the specification text. [`read_residual_block`] and
//! [`write_residual_block`] are exact inverses of each other, which the unit tests exercise exhaustively.

pub mod tables;

use crate::bitstream::{BitReader, BitWriter};
use crate::error::{Error, Result};
use tables::*;

const LUT_BITS: u32 = 8;

/// A prefix-code table: a lookup table for codes of up to 8 bits plus the full entry list for longer codes.
pub struct Vlc {
    lut: [u16; 1 << LUT_BITS],
    entries: &'static [(u8, u16, u8)],
}

impl Vlc {
    /// Build the lookup table at compile time from `(length, code, value)` entries.
    pub const fn new(entries: &'static [(u8, u16, u8)]) -> Self {
        let mut lut = [0u16; 1 << LUT_BITS];
        let mut i = 0;
        while i < entries.len() {
            let (len, code, val) = entries[i];
            if len as u32 <= LUT_BITS {
                let shift = LUT_BITS - len as u32;
                let base = (code as usize) << shift;
                let mut k = 0;
                while k < (1usize << shift) {
                    lut[base + k] = ((len as u16) << 8) | val as u16;
                    k += 1;
                }
            }
            i += 1;
        }
        Self { lut, entries }
    }

    /// Decode one symbol.
    #[inline]
    pub fn decode(&self, r: &mut BitReader) -> Result<u8> {
        let w = r.peek32();
        let e = self.lut[(w >> (32 - LUT_BITS)) as usize];
        if e != 0 {
            r.skip((e >> 8) as u32);
            return if r.overrun() { Err(Error::Truncated) } else { Ok(e as u8) };
        }
        for &(len, code, val) in self.entries {
            if len as u32 > LUT_BITS && (w >> (32 - len as u32)) == code as u32 {
                r.skip(len as u32);
                return if r.overrun() { Err(Error::Truncated) } else { Ok(val) };
            }
        }
        if r.bits_left() < 16 { Err(Error::Truncated) } else { Err(Error::Invalid("bad VLC code")) }
    }

    /// The `(length, code)` for a value, if it has one.
    pub fn code_for(&self, value: u8) -> Option<(u8, u16)> {
        self.entries.iter().find(|e| e.2 == value).map(|e| (e.0, e.1))
    }

    /// Write the code for `value`. Panics if the value has no code (an encoder bug, never input driven).
    pub fn encode(&self, w: &mut BitWriter, value: u8) {
        let (len, code) = self.code_for(value).expect("value has no VLC code");
        w.put_bits(code as u32, len as u32);
    }
}

static COEFF_TOKEN: [Vlc; 6] = [
    Vlc::new(&COEFF_TOKEN_0),
    Vlc::new(&COEFF_TOKEN_1),
    Vlc::new(&COEFF_TOKEN_2),
    Vlc::new(&COEFF_TOKEN_3),
    Vlc::new(&COEFF_TOKEN_CHROMA_DC),
    Vlc::new(&COEFF_TOKEN_CHROMA_DC_422),
];

static TOTAL_ZEROS: [Vlc; 15] = [
    Vlc::new(&TOTAL_ZEROS_1),
    Vlc::new(&TOTAL_ZEROS_2),
    Vlc::new(&TOTAL_ZEROS_3),
    Vlc::new(&TOTAL_ZEROS_4),
    Vlc::new(&TOTAL_ZEROS_5),
    Vlc::new(&TOTAL_ZEROS_6),
    Vlc::new(&TOTAL_ZEROS_7),
    Vlc::new(&TOTAL_ZEROS_8),
    Vlc::new(&TOTAL_ZEROS_9),
    Vlc::new(&TOTAL_ZEROS_10),
    Vlc::new(&TOTAL_ZEROS_11),
    Vlc::new(&TOTAL_ZEROS_12),
    Vlc::new(&TOTAL_ZEROS_13),
    Vlc::new(&TOTAL_ZEROS_14),
    Vlc::new(&TOTAL_ZEROS_15),
];

static TOTAL_ZEROS_DC: [Vlc; 3] =
    [Vlc::new(&TOTAL_ZEROS_DC_1), Vlc::new(&TOTAL_ZEROS_DC_2), Vlc::new(&TOTAL_ZEROS_DC_3)];

static RUN_BEFORE: [Vlc; 7] = [
    Vlc::new(&RUN_BEFORE_1),
    Vlc::new(&RUN_BEFORE_2),
    Vlc::new(&RUN_BEFORE_3),
    Vlc::new(&RUN_BEFORE_4),
    Vlc::new(&RUN_BEFORE_5),
    Vlc::new(&RUN_BEFORE_6),
    Vlc::new(&RUN_BEFORE_7),
];

/// Which coeff_token table applies for a predicted `nC` (Table 9-5 columns): 0 to 3 by range, 4 for chroma
/// DC (`nC == -1`).
#[inline]
fn token_table(nc: i32) -> usize {
    match nc {
        -1 => 4,
        0..=1 => 0,
        2..=3 => 1,
        4..=7 => 2,
        _ => 3,
    }
}

/// `nC` from the total coefficient counts of the left (A) and above (B) neighbouring blocks, `None` if the
/// neighbour is not available (9.2.1).
#[inline]
pub fn predict_nc(na: Option<u8>, nb: Option<u8>) -> i32 {
    match (na, nb) {
        (Some(a), Some(b)) => (a as i32 + b as i32 + 1) >> 1,
        (Some(a), None) => a as i32,
        (None, Some(b)) => b as i32,
        (None, None) => 0,
    }
}

/// Parse one `residual_block_cavlc()`.
///
/// `nc` is the predicted coefficient count (`-1` for chroma DC), `max_num_coeff` is 4 (chroma DC), 15 or 16.
/// The coefficient levels are written to `out[0..max_num_coeff]` in scan order (zero where there is none) and the
/// total number of non-zero coefficients is returned.
pub fn read_residual_block(
    r: &mut BitReader,
    nc: i32,
    max_num_coeff: usize,
    out: &mut [i32],
) -> Result<usize> {
    debug_assert!(max_num_coeff <= 16 && out.len() >= max_num_coeff);
    for v in out[..max_num_coeff].iter_mut() {
        *v = 0;
    }
    let tok = COEFF_TOKEN[token_table(nc)].decode(r)?;
    let (total, t1) = ((tok >> 2) as usize, (tok & 3) as usize);
    if total == 0 {
        return Ok(0);
    }
    if total > max_num_coeff {
        return Err(Error::Invalid("TotalCoeff exceeds maxNumCoeff"));
    }
    let mut level = [0i32; 16];
    for l in level.iter_mut().take(t1) {
        *l = 1 - 2 * r.read_bit()? as i32;
    }
    let mut suffix_len: u32 = if total > 10 && t1 < 3 { 1 } else { 0 };
    for i in t1..total {
        // level_prefix: number of leading zero bits.
        let w = r.peek32();
        let prefix = if w != 0 {
            w.leading_zeros()
        } else {
            return Err(if r.bits_left() < 32 {
                Error::Truncated
            } else {
                Error::Invalid("level_prefix too long")
            });
        };
        r.skip(prefix + 1);
        if prefix > 28 {
            return Err(Error::Invalid("level_prefix too long"));
        }
        let suffix_size = if prefix == 14 && suffix_len == 0 {
            4
        } else if prefix >= 15 {
            prefix - 3
        } else {
            suffix_len
        };
        let suffix = if suffix_size > 0 { r.read_bits(suffix_size)? } else { 0 };
        let mut code = ((prefix.min(15) << suffix_len) + suffix) as i32;
        if prefix >= 15 && suffix_len == 0 {
            code += 15;
        }
        if prefix >= 16 {
            code += (1 << (prefix - 3)) - 4096;
        }
        if i == t1 && t1 < 3 {
            code += 2;
        }
        let lv = if code & 1 == 0 { (code + 2) >> 1 } else { (-code - 1) >> 1 };
        level[i] = lv;
        if suffix_len == 0 {
            suffix_len = 1;
        }
        if lv.abs() > (3 << (suffix_len - 1)) && suffix_len < 6 {
            suffix_len += 1;
        }
    }
    let mut zeros_left = if total < max_num_coeff {
        let v = match max_num_coeff {
            4 => TOTAL_ZEROS_DC[total - 1].decode(r)?,
            _ => TOTAL_ZEROS[total - 1].decode(r)?,
        } as usize;
        if v + total > max_num_coeff {
            return Err(Error::Invalid("total_zeros too large"));
        }
        v
    } else {
        0
    };
    // Place levels from the highest frequency down.
    let mut pos = total + zeros_left; // one past the index of the last coefficient
    for (i, &lv) in level.iter().enumerate().take(total) {
        pos -= 1;
        out[pos] = lv;
        if i + 1 < total {
            let run = if zeros_left > 0 { RUN_BEFORE[zeros_left.min(7) - 1].decode(r)? as usize } else { 0 };
            if run > zeros_left {
                return Err(Error::Invalid("run_before exceeds zerosLeft"));
            }
            zeros_left -= run;
            pos -= run;
        }
    }
    Ok(total)
}

/// Write one `residual_block_cavlc()` for the coefficients `coeffs[0..max_num_coeff]` (scan order). Returns the
/// number of non-zero coefficients (`TotalCoeff`). The inverse of [`read_residual_block`].
pub fn write_residual_block(w: &mut BitWriter, nc: i32, max_num_coeff: usize, coeffs: &[i32]) -> usize {
    // Non-zero levels, highest frequency first, and their positions.
    let mut levels = [0i32; 16];
    let mut positions = [0usize; 16];
    let mut total = 0;
    for i in (0..max_num_coeff).rev() {
        if coeffs[i] != 0 {
            levels[total] = coeffs[i];
            positions[total] = i;
            total += 1;
        }
    }
    let mut t1 = 0;
    while t1 < total.min(3) && levels[t1].abs() == 1 {
        t1 += 1;
    }
    let tok = ((total << 2) | t1) as u8;
    COEFF_TOKEN[token_table(nc)].encode(w, tok);
    if total == 0 {
        return 0;
    }
    for &l in levels.iter().take(t1) {
        w.put_bit(l < 0);
    }
    let mut suffix_len: u32 = if total > 10 && t1 < 3 { 1 } else { 0 };
    for i in t1..total {
        let lv = levels[i];
        let mut code = if lv > 0 { 2 * lv - 2 } else { -2 * lv - 1 };
        if i == t1 && t1 < 3 {
            code -= 2;
        }
        // Choose the prefix.
        let (prefix, suffix, size) = if suffix_len == 0 && code < 14 {
            (code as u32, 0, 0)
        } else if suffix_len == 0 && code < 30 {
            (14, (code - 14) as u32, 4)
        } else if suffix_len > 0 && code < (15 << suffix_len) {
            ((code >> suffix_len) as u32, (code & ((1 << suffix_len) - 1)) as u32, suffix_len)
        } else {
            let mut p = 15u32;
            loop {
                let mut base = 15 << suffix_len;
                if suffix_len == 0 {
                    base += 15;
                }
                if p >= 16 {
                    base += (1 << (p - 3)) - 4096;
                }
                let size = p - 3;
                let rem = code - base;
                if rem >= 0 && (rem as i64) < (1i64 << size) {
                    break (p, rem as u32, size);
                }
                p += 1;
                debug_assert!(p < 29, "level too large to code");
                if p >= 29 {
                    break (28, 0, 25);
                }
            }
        };
        w.put_bits(0, prefix.min(31));
        if prefix > 31 {
            w.put_bits(0, prefix - 31);
        }
        w.put_bit(true);
        if size > 0 {
            w.put_bits(suffix, size);
        }
        if suffix_len == 0 {
            suffix_len = 1;
        }
        if lv.abs() > (3 << (suffix_len - 1)) && suffix_len < 6 {
            suffix_len += 1;
        }
    }
    let total_zeros = positions[0] + 1 - total; // zeros before the highest-frequency coefficient
    if total < max_num_coeff {
        match max_num_coeff {
            4 => TOTAL_ZEROS_DC[total - 1].encode(w, total_zeros as u8),
            _ => TOTAL_ZEROS[total - 1].encode(w, total_zeros as u8),
        }
    }
    let mut zeros_left = total_zeros;
    for i in 0..total - 1 {
        if zeros_left == 0 {
            break;
        }
        let run = positions[i] - positions[i + 1] - 1;
        RUN_BEFORE[zeros_left.min(7) - 1].encode(w, run as u8);
        zeros_left -= run;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn tables_are_prefix_free_and_decode_back() {
        let all: [&'static [(u8, u16, u8)]; 3] = [&COEFF_TOKEN_0, &COEFF_TOKEN_1, &COEFF_TOKEN_2];
        for (t, table) in all.iter().enumerate() {
            for &(len, code, val) in table.iter() {
                let mut w = BitWriter::new();
                w.put_bits(code as u32, len as u32);
                w.put_bits(0xFFFF, 16);
                let b = w.into_bytes();
                let mut r = BitReader::new(&b);
                assert_eq!(COEFF_TOKEN[t].decode(&mut r).unwrap(), val);
                assert_eq!(r.pos(), len as usize);
            }
        }
    }

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn random_blocks_roundtrip() {
        let mut seed = 99u32;
        for iter in 0..4000 {
            let max = [4usize, 15, 16][iter % 3];
            let nc = if max == 4 { -1 } else { [0, 1, 2, 3, 4, 7, 8, 12][lcg(&mut seed) as usize % 8] };
            let density = lcg(&mut seed) % 17;
            let mut c = [0i32; 16];
            for v in c.iter_mut().take(max) {
                if lcg(&mut seed) % 16 < density {
                    let mag = match lcg(&mut seed) % 8 {
                        0..=4 => 1,
                        5 => 2 + lcg(&mut seed) % 5,
                        6 => 7 + lcg(&mut seed) % 60,
                        _ => 70 + lcg(&mut seed) % 3000,
                    } as i32;
                    *v = if lcg(&mut seed) & 1 == 1 { mag } else { -mag };
                }
            }
            let mut w = BitWriter::new();
            let total = write_residual_block(&mut w, nc, max, &c);
            w.put_trailing_bits();
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            let mut out = [0i32; 16];
            let got = read_residual_block(&mut r, nc, max, &mut out).unwrap();
            assert_eq!(got, total);
            assert_eq!(&out[..max], &c[..max], "iter {iter} max {max} nc {nc}");
            assert!(!r.more_rbsp_data());
        }
    }

    #[test]
    fn garbage_never_panics() {
        let mut seed = 5u32;
        for _ in 0..3000 {
            let n = 1 + lcg(&mut seed) as usize % 20;
            let bytes: Vec<u8> = (0..n).map(|_| lcg(&mut seed) as u8).collect();
            for (max, nc) in [(16, 0), (16, 5), (15, 9), (4, -1)] {
                let mut r = BitReader::new(&bytes);
                let mut out = [0i32; 16];
                let _ = read_residual_block(&mut r, nc, max, &mut out);
            }
        }
    }
}
