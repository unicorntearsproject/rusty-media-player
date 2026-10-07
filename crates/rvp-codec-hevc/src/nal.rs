//! NAL units: the two-byte header, splitting a sample into units, and emulation prevention.
use alloc::vec::Vec;

/// `nal_unit_type` values this crate acts on.
pub mod kind {
    /// Coded slice of a non-TSA, non-STSA trailing picture (reference).
    pub const TRAIL_N: u8 = 0;
    /// Trailing picture, reference.
    pub const TRAIL_R: u8 = 1;
    /// RADL pictures.
    pub const RADL_N: u8 = 6;
    /// RADL pictures, reference.
    pub const RADL_R: u8 = 7;
    /// RASL pictures.
    pub const RASL_N: u8 = 8;
    /// RASL pictures, reference.
    pub const RASL_R: u8 = 9;
    /// First of the IRAP types: broken link access with leading pictures.
    pub const BLA_W_LP: u8 = 16;
    /// Instantaneous decoder refresh with leading pictures.
    pub const IDR_W_RADL: u8 = 19;
    /// IDR without leading pictures.
    pub const IDR_N_LP: u8 = 20;
    /// Clean random access.
    pub const CRA: u8 = 21;
    /// Last reserved IRAP type.
    pub const RSV_IRAP_VCL23: u8 = 23;
    /// Video parameter set.
    pub const VPS: u8 = 32;
    /// Sequence parameter set.
    pub const SPS: u8 = 33;
    /// Picture parameter set.
    pub const PPS: u8 = 34;
    /// End of sequence.
    pub const EOS: u8 = 36;
    /// End of bitstream.
    pub const EOB: u8 = 37;
}

/// The header of a NAL unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NalHeader {
    /// `nal_unit_type`.
    pub kind: u8,
    /// `nuh_layer_id`.
    pub layer: u8,
    /// `TemporalId` (`nuh_temporal_id_plus1 - 1`).
    pub tid: u8,
}

impl NalHeader {
    /// Parse the first two bytes of a unit; `None` for a short unit or `forbidden_zero_bit` set.
    pub fn parse(nal: &[u8]) -> Option<Self> {
        let (a, b) = (*nal.first()?, *nal.get(1)?);
        if a & 0x80 != 0 || b & 7 == 0 {
            return None;
        }
        Some(Self { kind: (a >> 1) & 0x3f, layer: ((a & 1) << 5) | (b >> 3), tid: (b & 7) - 1 })
    }

    /// A coded slice segment (types 0 to 9 and 16 to 21).
    pub fn is_slice(&self) -> bool {
        self.kind <= 9 || (16..=21).contains(&self.kind)
    }

    /// An intra random access point (16 to 23).
    pub fn is_irap(&self) -> bool {
        (kind::BLA_W_LP..=kind::RSV_IRAP_VCL23).contains(&self.kind)
    }

    /// IDR (19, 20).
    pub fn is_idr(&self) -> bool {
        matches!(self.kind, kind::IDR_W_RADL | kind::IDR_N_LP)
    }

    /// BLA (16, 17, 18).
    pub fn is_bla(&self) -> bool {
        (16..=18).contains(&self.kind)
    }

    /// RASL (8, 9).
    pub fn is_rasl(&self) -> bool {
        matches!(self.kind, kind::RASL_N | kind::RASL_R)
    }

    /// RADL (6, 7).
    pub fn is_radl(&self) -> bool {
        matches!(self.kind, kind::RADL_N | kind::RADL_R)
    }

    /// A sub-layer non-reference picture (even types below 16: the `_N` ones).
    pub fn is_sub_layer_non_ref(&self) -> bool {
        self.kind < 16 && self.kind % 2 == 0
    }
}

/// Split a length-prefixed (`hvcC` / MP4) sample into units; a truncated tail is dropped.
pub fn split_length_prefixed(data: &[u8], length_size: usize) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let ls = length_size.clamp(1, 4);
    let mut i = 0;
    while i + ls <= data.len() {
        let len = data[i..i + ls].iter().fold(0usize, |a, b| (a << 8) | *b as usize);
        i += ls;
        let end = i.saturating_add(len).min(data.len());
        if end > i {
            out.push(&data[i..end]);
        }
        i = end;
    }
    out
}

/// Split an Annex B byte stream into units (without start codes and trailing zero bytes).
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut i = 0;
    fn push<'a>(out: &mut Vec<&'a [u8]>, mut s: &'a [u8]) {
        while let Some((&0, rest)) = s.split_last() {
            s = rest;
        }
        if !s.is_empty() {
            out.push(s);
        }
    }
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if let Some(s) = start {
                push(&mut out, &data[s..i]);
            }
            i += 3;
            start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        push(&mut out, &data[s..]);
    }
    out
}

/// Remove emulation prevention bytes (`00 00 03` becomes `00 00`) from `payload` into `out`, and note where each removed byte sat in
/// `payload` (so a position found in the unescaped bytes can be mapped back).
pub fn unescape(payload: &[u8], out: &mut Vec<u8>, removed_at: &mut Vec<usize>) {
    out.clear();
    removed_at.clear();
    out.reserve(payload.len());
    let mut zeros = 0u32;
    for (i, &b) in payload.iter().enumerate() {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            removed_at.push(i);
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
}

/// The position in the escaped bytes of byte `pos` of the unescaped ones.
pub fn escaped_pos(pos: usize, removed_at: &[usize]) -> usize {
    // Every removed byte at or before the mapped position pushes it one further.
    let mut p = pos;
    for &r in removed_at {
        if r <= p {
            p += 1;
        } else {
            break;
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers() {
        // VPS: type 32, layer 0, tid_plus1 1.
        let h = NalHeader::parse(&[0x40, 0x01]).unwrap();
        assert_eq!((h.kind, h.layer, h.tid), (32, 0, 0));
        // IDR_W_RADL with tid 2.
        let h = NalHeader::parse(&[0x26, 0x03]).unwrap();
        assert_eq!((h.kind, h.tid), (19, 2));
        assert!(h.is_idr() && h.is_irap() && h.is_slice() && !h.is_rasl());
        assert!(NalHeader::parse(&[0x80, 0x01]).is_none());
        assert!(NalHeader::parse(&[0x40, 0x00]).is_none());
        assert!(NalHeader { kind: 0, layer: 0, tid: 0 }.is_sub_layer_non_ref());
        assert!(!NalHeader { kind: 1, layer: 0, tid: 0 }.is_sub_layer_non_ref());
    }

    #[test]
    fn splitting_and_escaping() {
        let s = [0, 0, 0, 3, 1, 2, 3, 0, 0, 0, 2, 9, 9, 0, 0];
        assert_eq!(split_length_prefixed(&s, 4), [&[1u8, 2, 3][..], &[9, 9][..]]);
        let a = [0, 0, 0, 1, 0x40, 1, 5, 0, 0, 1, 0x42, 1, 6, 0, 0, 0];
        assert_eq!(split_annexb(&a), [&[0x40u8, 1, 5][..], &[0x42, 1, 6][..]]);
        let mut out = Vec::new();
        let mut rem = Vec::new();
        unescape(&[1, 0, 0, 3, 0, 0, 3, 1], &mut out, &mut rem);
        assert_eq!(out, [1, 0, 0, 0, 0, 1]);
        assert_eq!(rem, [3, 6]);
        assert_eq!(escaped_pos(0, &rem), 0);
        assert_eq!(escaped_pos(3, &rem), 4);
        assert_eq!(escaped_pos(5, &rem), 7);
    }
}
