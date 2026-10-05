//! Native FLAC (`fLaC`): STREAMINFO, SEEKTABLE, Vorbis comments and pictures, then one packet per audio frame.
//!
//! A FLAC frame does not say how long it is, so its end is found the way the format intends: the next frame header (sync
//! pattern, valid header CRC-8) that follows bytes whose CRC-16 matches the one stored at the end of the frame.
use crate::Demuxer;
use crate::framed::{audio_stream, read_id3v2};
use crate::io::{Reader, Win};
use crate::tags;
use alloc::string::ToString;
use alloc::vec::Vec;
use rvp_core::{Chapter, Error, Metadata, Packet, Result, StreamInfo, Timestamp};
use rvp_host::Source;

/// CRC-8 (polynomial 0x07) of a frame header, table-driven.
const fn crc8_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

/// CRC-16 (polynomial 0x8005) of a whole frame.
const fn crc16_table() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC8: [u8; 256] = crc8_table();
static CRC16: [u16; 256] = crc16_table();

pub(crate) fn crc8(b: &[u8]) -> u8 {
    b.iter().fold(0u8, |c, &x| CRC8[(c ^ x) as usize])
}

fn crc16_step(c: u16, x: u8) -> u16 {
    (c << 8) ^ CRC16[((c >> 8) as u8 ^ x) as usize]
}

/// What a frame header says: where the frame is in the stream and how long it is, and where the header ends.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameInfo {
    /// First sample of the frame (a frame number times the block size for fixed-size streams).
    pub sample: u64,
    pub block: u32,
    pub header_len: usize,
}

/// Parse and validate (sync, reserved bits, CRC-8) the frame header at the start of `h`. `fixed_block` is the stream's
/// block size when all blocks are the same (then the header carries a frame number, not a sample number).
pub(crate) fn parse_frame_header(h: &[u8], fixed_block: Option<u32>) -> Option<FrameInfo> {
    if h.len() < 6 || h[0] != 0xFF || h[1] & 0xFE != 0xF8 || h[3] & 1 != 0 {
        return None;
    }
    let variable = h[1] & 1 == 1;
    let (bs_code, sr_code) = (h[2] >> 4, h[2] & 15);
    let (chan, size_code) = (h[3] >> 4, (h[3] >> 1) & 7);
    if bs_code == 0 || sr_code == 15 || size_code == 3 || size_code == 7 || chan > 10 {
        return None;
    }
    // The coded frame or sample number: UTF-8 style, up to 7 bytes.
    let b0 = h[4];
    let n = match b0 {
        0x00..=0x7F => 0,
        0xC0..=0xDF => 1,
        0xE0..=0xEF => 2,
        0xF0..=0xF7 => 3,
        0xF8..=0xFB => 4,
        0xFC..=0xFD => 5,
        0xFE => 6,
        _ => return None,
    };
    let mut o = 5usize;
    let mut v = match n {
        0 => b0 as u64,
        6 => 0,
        _ => (b0 & (0x3F >> n)) as u64,
    };
    for _ in 0..n {
        let c = *h.get(o)?;
        if c & 0xC0 != 0x80 {
            return None;
        }
        v = (v << 6) | (c & 0x3F) as u64;
        o += 1;
    }
    let block = match bs_code {
        1 => 192,
        2..=5 => 576 << (bs_code - 2),
        6 => {
            let b = *h.get(o)? as u32 + 1;
            o += 1;
            b
        }
        7 => {
            let b = u16::from_be_bytes([*h.get(o)?, *h.get(o + 1)?]) as u32 + 1;
            o += 2;
            b
        }
        c => 256 << (c - 8),
    };
    // An explicit sample rate (codes 12 to 14) follows the block size.
    o += match sr_code {
        12 => 1,
        13 | 14 => 2,
        _ => 0,
    };
    if crc8(h.get(..o)?) != *h.get(o)? {
        return None;
    }
    let sample = if variable { v } else { v.checked_mul(fixed_block.unwrap_or(block) as u64)? };
    Some(FrameInfo { sample, block, header_len: o + 1 })
}

struct SeekPoint {
    sample: u64,
    offset: u64,
}

/// A native FLAC file.
pub struct FlacDemuxer<S: Source> {
    rd: Reader<S>,
    win: Win,
    streams: Vec<StreamInfo>,
    meta: Metadata,
    duration: Option<Timestamp>,
    rate: u64,
    /// Block size when the stream uses one (min == max in STREAMINFO).
    fixed_block: Option<u32>,
    /// Offset of the first audio frame, and the end of the audio (the file size, as far as known).
    first: u64,
    pos: u64,
    seektable: Vec<SeekPoint>,
}

impl<S: Source> FlacDemuxer<S> {
    /// Read the metadata blocks of `src`.
    pub async fn open(src: S) -> Result<Self> {
        let mut rd = Reader::new(src).await;
        let mut meta = Metadata::default();
        let mut at = read_id3v2(&mut rd, &mut meta).await?;
        let mut magic = [0u8; 4];
        if rd.read_upto(at, &mut magic).await? < 4 || &magic != b"fLaC" {
            return Err(Error::Invalid("not a FLAC file".to_string()));
        }
        at += 4;
        let mut info: Option<[u8; 34]> = None;
        let mut seektable = Vec::new();
        let mut best_art: Option<(u32, rvp_core::Art)> = None;
        // Metadata blocks: 1 byte (last flag, type), 3 bytes length.
        for _ in 0..4096 {
            let mut h = [0u8; 4];
            rd.read_exact(at, &mut h).await?;
            let (last, kind) = (h[0] & 0x80 != 0, h[0] & 0x7F);
            let len = u32::from_be_bytes([0, h[1], h[2], h[3]]) as u64;
            let body = at + 4;
            at = body + len;
            match kind {
                0 if len >= 34 => {
                    let b = rd.read_vec(body, 34).await?;
                    info = Some(b[..].try_into().map_err(|_| Error::Invalid("STREAMINFO".to_string()))?);
                }
                3 => {
                    let b = rd.read_vec(body, len.min(1 << 20)).await?;
                    for e in b.chunks_exact(18) {
                        let sample = u64::from_be_bytes(e[..8].try_into().unwrap_or([0; 8]));
                        let offset = u64::from_be_bytes(e[8..16].try_into().unwrap_or([0; 8]));
                        // Placeholder points (all ones) pad the table.
                        if sample != u64::MAX {
                            seektable.push(SeekPoint { sample, offset });
                        }
                    }
                }
                4 if len <= tags::MAX_ID3 => {
                    let b = rd.read_vec(body, len).await?;
                    tags::parse_vorbis_comments(&b, &mut meta);
                }
                6 if len <= tags::MAX_ART as u64 + 4096 => {
                    let b = rd.read_vec(body, len).await?;
                    if let Some((t, art)) = tags::parse_flac_picture(&b) {
                        if best_art.as_ref().is_none_or(|(bt, _)| t == 3 && *bt != 3) {
                            best_art = Some((t, art));
                        }
                    }
                }
                _ => {}
            }
            if last {
                break;
            }
        }
        if meta.art.is_none() {
            meta.art = best_art.map(|(_, a)| a);
        }
        let si = info.ok_or_else(|| Error::Invalid("FLAC without STREAMINFO".to_string()))?;
        let (min_b, max_b) =
            (u16::from_be_bytes([si[0], si[1]]) as u32, u16::from_be_bytes([si[2], si[3]]) as u32);
        let packed = u64::from_be_bytes([si[10], si[11], si[12], si[13], si[14], si[15], si[16], si[17]]);
        let rate = (packed >> 44) as u32;
        let channels = (((packed >> 41) & 7) + 1) as u16;
        let total = packed & 0xF_FFFF_FFFF;
        if rate == 0 {
            return Err(Error::Invalid("FLAC with no sample rate".to_string()));
        }
        let mut extra = b"fLaC".to_vec();
        extra.extend_from_slice(&[0x80, 0, 0, 34]);
        extra.extend_from_slice(&si);
        let mut me = Self {
            rd,
            win: Win::new(),
            streams: Vec::new(),
            meta,
            duration: None,
            rate: rate as u64,
            fixed_block: (min_b == max_b && min_b > 0).then_some(min_b),
            first: at,
            pos: at,
            seektable,
        };
        // STREAMINFO may not know the length (streamed encodes); then the last frame says where the audio ends.
        let samples = if total > 0 { Some(total) } else { me.find_end().await.ok().flatten() };
        me.duration = samples.map(|s| (s as u128 * 1_000_000 / rate as u128) as i64);
        me.streams = alloc::vec![audio_stream("flac", rate, channels, extra, me.duration)];
        Ok(me)
    }

    fn us(&self, sample: u64) -> i64 {
        (sample as u128 * 1_000_000 / self.rate as u128).min(i64::MAX as u128) as i64
    }

    /// Where the frame at or after `from` starts, scanning byte by byte (`None` at the end of the data).
    async fn sync_from(&mut self, from: u64, limit: u64) -> Result<Option<(u64, FrameInfo)>> {
        let mut p = from;
        while p < limit {
            let w = self.win.get(&mut self.rd, p, 32).await?;
            if w.len() < 6 {
                return Ok(None);
            }
            // Skip quickly to the next 0xFF.
            let Some(skip) = w.iter().position(|&b| b == 0xFF) else {
                p += w.len() as u64;
                continue;
            };
            if skip > 0 {
                p += skip as u64;
                continue;
            }
            if let Some(fi) = parse_frame_header(w, self.fixed_block) {
                return Ok(Some((p, fi)));
            }
            p += 1;
        }
        Ok(None)
    }

    /// The length in samples of a stream whose STREAMINFO does not give it: the last frame's start plus its block size.
    async fn find_end(&mut self) -> Result<Option<u64>> {
        let Some(size) = self.rd.refresh_size().await else { return Ok(None) };
        let mut from = size.saturating_sub(1 << 20).max(self.first);
        loop {
            let mut last = None;
            let mut p = from;
            while let Some((q, fi)) = self.sync_from(p, size).await? {
                last = Some(fi);
                p = q + fi.header_len as u64;
            }
            if let Some(fi) = last {
                return Ok(Some(fi.sample + fi.block as u64));
            }
            if from <= self.first {
                return Ok(None);
            }
            from = from.saturating_sub(4 << 20).max(self.first);
        }
    }
}

impl<S: Source> Demuxer for FlacDemuxer<S> {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn duration_us(&self) -> Option<Timestamp> {
        self.duration
    }
    fn metadata(&self) -> &Metadata {
        &self.meta
    }
    fn chapters(&self) -> &[Chapter] {
        &[]
    }

    async fn next_packet(&mut self) -> Result<Option<Packet>> {
        let Some((start, fi)) = self.sync_from(self.pos, u64::MAX).await? else { return Ok(None) };
        let mut n = 64usize << 10;
        let data = loop {
            let w = self.win.get(&mut self.rd, start, n).await?;
            let eof = w.len() < n;
            match frame_end(w, &fi, self.fixed_block, eof) {
                FrameEnd::Found(len) => break w[..len].to_vec(),
                FrameEnd::More if n < (64 << 20) => n *= 2,
                FrameEnd::More | FrameEnd::Cut => {
                    // The data ends inside this frame: cut short, or still being written.
                    self.rd.refresh_size().await;
                    return Err(Error::Truncated);
                }
            }
        };
        self.pos = start + data.len() as u64;
        let pts = self.us(fi.sample);
        let dur = self.us(fi.sample + fi.block as u64) - pts;
        Ok(Some(Packet {
            stream_id: 1,
            pts,
            dts: pts,
            duration: dur,
            keyframe: true,
            discard_end_us: 0,
            data,
        }))
    }

    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        let want = (target_us.max(0) as u128 * self.rate as u128 / 1_000_000) as u64;
        // The seek table gives a byte offset (relative to the first frame) of a frame at or before the sample wanted.
        let (mut lo, mut lo_sample) = (self.first, 0u64);
        if let Some(sp) = self.seektable.iter().rfind(|p| p.sample <= want) {
            lo = self.first + sp.offset;
            lo_sample = sp.sample;
        }
        let size = self.rd.refresh_size().await;
        if self.seektable.is_empty() {
            // No table: bisect on the byte position, reading the sample number of the frame found there.
            let mut hi = size.unwrap_or(lo);
            while hi.saturating_sub(lo) > 64 << 10 {
                let mid = lo + (hi - lo) / 2;
                match self.sync_from(mid, hi).await? {
                    Some((p, fi)) if fi.sample <= want => {
                        lo = p;
                        lo_sample = fi.sample;
                    }
                    Some(_) => hi = mid,
                    None => hi = mid,
                }
            }
        }
        // Walk forward to the last frame at or before the target.
        let (mut best, mut best_sample) = (lo, lo_sample);
        let mut p = lo;
        while let Some((q, fi)) = self.sync_from(p, u64::MAX).await? {
            if fi.sample > want {
                break;
            }
            best = q;
            best_sample = fi.sample;
            if fi.sample + fi.block as u64 > want {
                break;
            }
            p = q + fi.header_len as u64 + 2;
        }
        self.pos = best;
        Ok(self.us(best_sample))
    }
}

enum FrameEnd {
    Found(usize),
    /// The bytes given do not reach the end of the frame; ask for more.
    More,
    /// The data ended inside the frame.
    Cut,
}

/// Find where the frame at the start of `w` (header described by `fi`) ends: the first place where the CRC-16 of everything
/// before the last two bytes equals those bytes and a valid frame header (or the end of the data) follows.
fn frame_end(w: &[u8], fi: &FrameInfo, fixed: Option<u32>, eof: bool) -> FrameEnd {
    let hl = fi.header_len;
    if w.len() < hl + 2 {
        return if eof { FrameEnd::Cut } else { FrameEnd::More };
    }
    let mut crc = w[..hl].iter().fold(0u16, |c, &x| crc16_step(c, x));
    let mut fallback = None;
    for e in hl + 2..=w.len() {
        if crc == u16::from_be_bytes([w[e - 2], w[e - 1]]) {
            if e == w.len() {
                if eof {
                    return FrameEnd::Found(e);
                }
                return FrameEnd::More;
            }
            if w[e] == 0xFF {
                if w.len() - e < 16 && !eof {
                    return FrameEnd::More;
                }
                if parse_frame_header(&w[e..], fixed).is_some_and(|n| n.sample >= fi.sample) {
                    return FrameEnd::Found(e);
                }
            }
            fallback.get_or_insert(e);
        }
        crc = crc16_step(crc, w[e - 2]);
    }
    match (eof, fallback) {
        // Junk after the last frame (a tag): the last CRC match is the end.
        (true, Some(e)) => FrameEnd::Found(e),
        (true, None) => FrameEnd::Cut,
        (false, _) => FrameEnd::More,
    }
}
