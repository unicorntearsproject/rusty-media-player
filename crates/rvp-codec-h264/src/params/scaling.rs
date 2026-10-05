//! Scaling lists (7.3.2.1.1.1, 7.4.2.1.1): syntax, default lists, fall-back rules and resolution.
use crate::bitstream::{BitReader, BitWriter};
use crate::error::{Error, Result};
use alloc::vec::Vec;

/// Default_4x4_Intra, in zig-zag order (Table 7-3).
pub const DEFAULT_4X4_INTRA: [u8; 16] = [6, 13, 13, 20, 20, 20, 28, 28, 28, 28, 32, 32, 32, 37, 37, 42];
/// Default_4x4_Inter, in zig-zag order (Table 7-3).
pub const DEFAULT_4X4_INTER: [u8; 16] = [10, 14, 14, 20, 20, 20, 24, 24, 24, 24, 27, 27, 27, 30, 30, 34];
/// Default_8x8_Intra, in 8x8 zig-zag order (Table 7-4).
pub const DEFAULT_8X8_INTRA: [u8; 64] = [
    6, 10, 10, 13, 11, 13, 16, 16, 16, 16, 18, 18, 18, 18, 18, 23, 23, 23, 23, 23, 23, 25, 25, 25, 25, 25,
    25, 25, 27, 27, 27, 27, 27, 27, 27, 27, 29, 29, 29, 29, 29, 29, 29, 31, 31, 31, 31, 31, 31, 33, 33, 33,
    33, 33, 36, 36, 36, 36, 38, 38, 38, 40, 40, 42,
];
/// Default_8x8_Inter, in 8x8 zig-zag order (Table 7-4).
pub const DEFAULT_8X8_INTER: [u8; 64] = [
    9, 13, 13, 15, 13, 15, 17, 17, 17, 17, 19, 19, 19, 19, 19, 21, 21, 21, 21, 21, 21, 22, 22, 22, 22, 22,
    22, 22, 24, 24, 24, 24, 24, 24, 24, 24, 25, 25, 25, 25, 25, 25, 25, 27, 27, 27, 27, 27, 27, 28, 28, 28,
    28, 28, 30, 30, 30, 30, 32, 32, 32, 33, 33, 35,
];

/// How one scaling list is signalled.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ListSpec {
    /// `scaling_list_present_flag` is 0: use the fall-back rule.
    #[default]
    NotPresent,
    /// Present, and `useDefaultScalingMatrixFlag` is set.
    UseDefault,
    /// Present with explicit values (16 or 64, in zig-zag order).
    Explicit(Vec<u8>),
}

/// The scaling-matrix syntax of an SPS or PPS: 6 lists of 4x4 then up to 6 of 8x8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalingSyntax {
    /// One entry per list (index 0 to 5: 4x4 Intra Y/Cb/Cr, Inter Y/Cb/Cr; 6 to 11: 8x8 Intra Y, Inter Y, ...).
    pub lists: Vec<ListSpec>,
}

fn parse_list(r: &mut BitReader, size: usize) -> Result<ListSpec> {
    let mut list = Vec::with_capacity(size);
    let mut last = 8i32;
    let mut next = 8i32;
    for j in 0..size {
        if next != 0 {
            let delta = r.read_se()?;
            if !(-128..=127).contains(&delta) {
                return Err(Error::Invalid("delta_scale out of range"));
            }
            next = (last + delta + 256).rem_euclid(256);
            if j == 0 && next == 0 {
                return Ok(ListSpec::UseDefault);
            }
        }
        let v = if next == 0 { last } else { next };
        list.push(v as u8);
        last = v;
    }
    Ok(ListSpec::Explicit(list))
}

fn write_list(w: &mut BitWriter, l: &[u8]) {
    let mut last = 8i32;
    for &v in l {
        let mut d = v as i32 - last;
        if d > 127 {
            d -= 256;
        }
        if d < -128 {
            d += 256;
        }
        w.put_se(d);
        last = v as i32;
    }
}

impl ScalingSyntax {
    /// Parse the `*_scaling_list_present_flag` loop for `count` lists (8, or 12 for 4:4:4 SPS; 6 plus
    /// 2 or 6 times `transform_8x8_mode_flag` for a PPS).
    pub fn parse(r: &mut BitReader, count: usize) -> Result<Self> {
        let mut lists = Vec::with_capacity(count);
        for i in 0..count {
            if r.read_flag()? {
                lists.push(parse_list(r, if i < 6 { 16 } else { 64 })?);
            } else {
                lists.push(ListSpec::NotPresent);
            }
        }
        Ok(Self { lists })
    }

    /// Write the flags and lists.
    pub fn write(&self, w: &mut BitWriter) {
        for l in &self.lists {
            match l {
                ListSpec::NotPresent => w.put_bit(false),
                ListSpec::UseDefault => {
                    w.put_bit(true);
                    w.put_se(-8); // first delta makes nextScale 0
                }
                ListSpec::Explicit(v) => {
                    w.put_bit(true);
                    write_list(w, v);
                }
            }
        }
    }
}

/// The resolved scaling lists used for dequantisation, in zig-zag order as transmitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalingMatrices {
    /// 4x4 lists: Intra Y, Cb, Cr, Inter Y, Cb, Cr.
    pub l4: [[u8; 16]; 6],
    /// 8x8 lists: Intra Y, Inter Y, Intra Cb, Inter Cb, Intra Cr, Inter Cr.
    pub l8: [[u8; 64]; 6],
}

impl ScalingMatrices {
    /// Flat_4x4_16 and Flat_8x8_16.
    pub fn flat() -> Self {
        Self { l4: [[16; 16]; 6], l8: [[16; 64]; 6] }
    }

    fn default_for(i: usize) -> ListRef {
        match i {
            0..=2 => ListRef::L4(DEFAULT_4X4_INTRA),
            3..=5 => ListRef::L4(DEFAULT_4X4_INTER),
            _ => ListRef::L8(if (i - 6) % 2 == 0 { DEFAULT_8X8_INTRA } else { DEFAULT_8X8_INTER }),
        }
    }

    /// Resolve the matrices for a sequence: flat if the SPS carries none, else the lists with fall-back rule A.
    pub fn from_sps(syntax: Option<&ScalingSyntax>) -> Self {
        match syntax {
            None => Self::flat(),
            Some(s) => Self::resolve(s, None),
        }
    }

    /// Resolve the matrices for a picture: the PPS lists, falling back per 7.4.2.2 (rule A when the SPS had no
    /// matrix, rule B, i.e. the sequence-level lists, when it did), or the sequence matrices if the PPS has none.
    pub fn from_pps(sps_syntax: Option<&ScalingSyntax>, pps_syntax: Option<&ScalingSyntax>) -> Self {
        let seq = Self::from_sps(sps_syntax);
        match pps_syntax {
            None => seq,
            Some(p) => Self::resolve(p, sps_syntax.map(|_| &seq)),
        }
    }

    fn resolve(s: &ScalingSyntax, fallback_b: Option<&ScalingMatrices>) -> Self {
        let mut m = Self::flat();
        for i in 0..12 {
            let spec = s.lists.get(i).cloned().unwrap_or(ListSpec::NotPresent);
            let resolved = match spec {
                ListSpec::Explicit(v) => {
                    if i < 6 {
                        let mut a = [0u8; 16];
                        a.copy_from_slice(&v);
                        ListRef::L4(a)
                    } else {
                        let mut a = [0u8; 64];
                        a.copy_from_slice(&v);
                        ListRef::L8(a)
                    }
                }
                ListSpec::UseDefault => Self::default_for(i),
                ListSpec::NotPresent => match i {
                    // First list of each kind: rule A is the default list, rule B the sequence-level list.
                    0 | 3 | 6 | 7 => match fallback_b {
                        None => Self::default_for(i),
                        Some(seq) => {
                            if i < 6 {
                                ListRef::L4(seq.l4[i])
                            } else {
                                ListRef::L8(seq.l8[i - 6])
                            }
                        }
                    },
                    // The others copy the preceding list of the same kind.
                    1 | 2 | 4 | 5 => ListRef::L4(m.l4[i - 1]),
                    _ => ListRef::L8(m.l8[i - 8]), // 8x8 lists 2..5 copy the one two before (same intra/inter kind)
                },
            };
            match resolved {
                ListRef::L4(a) => m.l4[i] = a,
                ListRef::L8(a) => m.l8[i - 6] = a,
            }
        }
        m
    }
}

enum ListRef {
    L4([u8; 16]),
    L8([u8; 64]),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_roundtrip_and_resolution() {
        let mut lists = alloc::vec![ListSpec::NotPresent; 8];
        lists[0] = ListSpec::UseDefault;
        lists[1] = ListSpec::Explicit((0..16).map(|i| 8 + i as u8 * 3).collect());
        lists[6] = ListSpec::Explicit((0..64).map(|i| 200 - i as u8).collect());
        let s = ScalingSyntax { lists };
        let mut w = BitWriter::new();
        s.write(&mut w);
        w.put_trailing_bits();
        let b = w.into_bytes();
        let mut r = BitReader::new(&b);
        assert_eq!(ScalingSyntax::parse(&mut r, 8).unwrap(), s);
        let m = ScalingMatrices::from_sps(Some(&s));
        assert_eq!(m.l4[0], DEFAULT_4X4_INTRA);
        assert_eq!(m.l4[1][3], 17);
        assert_eq!(m.l4[2], m.l4[1]);
        assert_eq!(m.l4[3], DEFAULT_4X4_INTER);
        assert_eq!(m.l4[4], DEFAULT_4X4_INTER);
        assert_eq!(m.l8[0][1], 199);
        assert_eq!(m.l8[1], DEFAULT_8X8_INTER);
    }
}
