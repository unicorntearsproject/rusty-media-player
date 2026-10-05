//! NAL unit framing (Annex B byte streams and length-prefixed AVCC), RBSP escaping and the avcC record.
use alloc::vec::Vec;

/// NAL unit types (Table 7-1) that matter to a decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalUnitType {
    /// Coded slice of a non-IDR picture (1).
    Slice,
    /// Coded slice data partition A/B/C (2 to 4); not supported.
    Partition(u8),
    /// Coded slice of an IDR picture (5).
    IdrSlice,
    /// Supplemental enhancement information (6).
    Sei,
    /// Sequence parameter set (7).
    Sps,
    /// Picture parameter set (8).
    Pps,
    /// Access unit delimiter (9).
    Aud,
    /// End of sequence (10).
    EndOfSequence,
    /// End of stream (11).
    EndOfStream,
    /// Filler data (12).
    Filler,
    /// Sequence parameter set extension (13).
    SpsExt,
    /// Anything else (extensions, reserved, unspecified), with its numeric value.
    Other(u8),
}

impl NalUnitType {
    /// Map the 5-bit `nal_unit_type`.
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Slice,
            2..=4 => Self::Partition(v),
            5 => Self::IdrSlice,
            6 => Self::Sei,
            7 => Self::Sps,
            8 => Self::Pps,
            9 => Self::Aud,
            10 => Self::EndOfSequence,
            11 => Self::EndOfStream,
            12 => Self::Filler,
            13 => Self::SpsExt,
            o => Self::Other(o),
        }
    }

    /// The numeric `nal_unit_type`.
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Slice => 1,
            Self::Partition(v) => v,
            Self::IdrSlice => 5,
            Self::Sei => 6,
            Self::Sps => 7,
            Self::Pps => 8,
            Self::Aud => 9,
            Self::EndOfSequence => 10,
            Self::EndOfStream => 11,
            Self::Filler => 12,
            Self::SpsExt => 13,
            Self::Other(v) => v,
        }
    }
}

/// The one-byte NAL unit header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NalHeader {
    /// `nal_ref_idc` (0 means the picture is not used for reference).
    pub ref_idc: u8,
    /// `nal_unit_type`.
    pub unit_type: NalUnitType,
}

impl NalHeader {
    /// Parse the first byte of a NAL unit. `None` if the forbidden bit is set or the unit is empty.
    pub fn parse(nal: &[u8]) -> Option<Self> {
        let b = *nal.first()?;
        if b & 0x80 != 0 {
            return None;
        }
        Some(Self { ref_idc: (b >> 5) & 3, unit_type: NalUnitType::from_u8(b & 31) })
    }

    /// The header byte.
    pub fn to_byte(self) -> u8 {
        (self.ref_idc << 5) | self.unit_type.to_u8()
    }
}

/// Split an Annex B byte stream into NAL units (without start codes and without trailing zero bytes).
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let n = data.len();
    // Find the first start code.
    let mut i = 0;
    let mut start: Option<usize> = None;
    while i + 2 < n {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if let Some(s) = start {
                push_trimmed(&mut out, &data[s..i]);
            }
            i += 3;
            start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        push_trimmed(&mut out, &data[s..]);
    }
    out
}

fn push_trimmed<'a>(out: &mut Vec<&'a [u8]>, mut s: &'a [u8]) {
    while let Some((&0, rest)) = s.split_last() {
        s = rest;
    }
    if !s.is_empty() {
        out.push(s);
    }
}

/// Split a length-prefixed (AVCC / MP4) sample into NAL units. A truncated tail is dropped.
/// `length_size` is 1 to 4.
pub fn split_length_prefixed(data: &[u8], length_size: usize) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let ls = length_size.clamp(1, 4);
    let mut i = 0;
    while i + ls <= data.len() {
        let mut len = 0usize;
        for &b in &data[i..i + ls] {
            len = (len << 8) | b as usize;
        }
        i += ls;
        let end = i.saturating_add(len).min(data.len());
        if end > i {
            out.push(&data[i..end]);
        }
        i = end;
    }
    out
}

/// Remove emulation prevention bytes (`00 00 03` becomes `00 00`) from a NAL unit payload (the bytes after
/// the one-byte header, or the whole unit if the caller wants the header kept) into `out`.
pub fn unescape(payload: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(payload.len());
    let mut zeros = 0u32;
    for &b in payload {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
        out.push(b);
    }
}

/// Insert emulation prevention bytes into an RBSP, appending to `out`.
pub fn escape(rbsp: &[u8], out: &mut Vec<u8>) {
    let mut zeros = 0u32;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
}

/// Append one NAL unit (header byte, escaped RBSP) to `out`, preceded by a 4-byte Annex B start code if
/// `start_code` is true.
pub fn write_nal(header: NalHeader, rbsp: &[u8], start_code: bool, out: &mut Vec<u8>) {
    if start_code {
        out.extend_from_slice(&[0, 0, 0, 1]);
    }
    out.push(header.to_byte());
    escape(rbsp, out);
}

/// A parsed `AVCDecoderConfigurationRecord` (the contents of an `avcC` box).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvcConfig {
    /// `AVCProfileIndication`.
    pub profile: u8,
    /// `profile_compatibility`.
    pub compat: u8,
    /// `AVCLevelIndication`.
    pub level: u8,
    /// Size of the NAL length field in samples: 1 to 4.
    pub length_size: usize,
    /// Sequence parameter set NAL units (header byte included).
    pub sps: Vec<Vec<u8>>,
    /// Picture parameter set NAL units (header byte included).
    pub pps: Vec<Vec<u8>>,
}

impl AvcConfig {
    /// Parse an avcC record. `None` if it is malformed.
    pub fn parse(d: &[u8]) -> Option<Self> {
        if d.len() < 7 || d[0] != 1 {
            return None;
        }
        let length_size = (d[4] & 3) as usize + 1;
        let mut i = 5;
        let read_sets = |count: usize, i: &mut usize| -> Option<Vec<Vec<u8>>> {
            let mut v = Vec::new();
            for _ in 0..count {
                let len = u16::from_be_bytes([*d.get(*i)?, *d.get(*i + 1)?]) as usize;
                *i += 2;
                v.push(d.get(*i..*i + len)?.to_vec());
                *i += len;
            }
            Some(v)
        };
        let n_sps = (d[i] & 31) as usize;
        i += 1;
        let sps = read_sets(n_sps, &mut i)?;
        let n_pps = *d.get(i)? as usize;
        i += 1;
        let pps = read_sets(n_pps, &mut i)?;
        Some(Self { profile: d[1], compat: d[2], level: d[3], length_size, sps, pps })
    }

    /// Serialise as an avcC record (for tests and muxers).
    pub fn write(&self) -> Vec<u8> {
        let mut o =
            alloc::vec![1, self.profile, self.compat, self.level, 0xFC | (self.length_size as u8 - 1).min(3)];
        o.push(0xE0 | self.sps.len() as u8);
        for s in &self.sps {
            o.extend_from_slice(&(s.len() as u16).to_be_bytes());
            o.extend_from_slice(s);
        }
        o.push(self.pps.len() as u8);
        for p in &self.pps {
            o.extend_from_slice(&(p.len() as u16).to_be_bytes());
            o.extend_from_slice(p);
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annexb_split_handles_three_and_four_byte_codes() {
        let d = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 0];
        let n = split_annexb(&d);
        assert_eq!(n, alloc::vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9][..]]);
    }

    #[test]
    fn escape_roundtrip() {
        let rbsp = [0u8, 0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 0, 0];
        let mut e = Vec::new();
        escape(&rbsp, &mut e);
        assert_eq!(e, [0, 0, 3, 0, 0, 3, 1, 0, 0, 3, 2, 0, 0, 3, 3, 0, 0]);
        let mut u = Vec::new();
        unescape(&e, &mut u);
        assert_eq!(u, rbsp);
    }

    #[test]
    fn length_prefixed() {
        let d = [0, 0, 0, 2, 0x65, 1, 0, 0, 0, 1, 0x41, 0, 0, 0, 9, 1];
        let n = split_length_prefixed(&d, 4);
        assert_eq!(n, alloc::vec![&[0x65, 1][..], &[0x41][..], &[1][..]]);
    }

    #[test]
    fn avcc_roundtrip() {
        let c = AvcConfig {
            profile: 100,
            compat: 0,
            level: 31,
            length_size: 4,
            sps: alloc::vec![alloc::vec![0x67, 1, 2]],
            pps: alloc::vec![alloc::vec![0x68, 3]],
        };
        assert_eq!(AvcConfig::parse(&c.write()), Some(c));
    }
}
