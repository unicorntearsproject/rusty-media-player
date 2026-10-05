//! Self-delimiting audio frames in a raw file: MPEG audio layer III (`.mp3`) and ADTS AAC (`.aac`). Both are a run of
//! frames that each start with a sync pattern and carry their own length, so one engine serves both: find a frame, check
//! that another follows it, hand it out, and keep a sparse index of frame positions for exact seeks.
use crate::Demuxer;
use crate::io::{Reader, Win};
use crate::tags;
use alloc::string::ToString;
use alloc::vec::Vec;
use rvp_core::{
    AudioInfo, Chapter, Error, Metadata, Packet, Rational, Result, StreamInfo, StreamKind, Timestamp,
};
use rvp_host::Source;

/// One frame header, as a format reads it.
pub(crate) struct FrameHdr {
    /// The whole frame, header included.
    pub len: usize,
    /// PCM samples per channel the frame decodes to.
    pub samples: u32,
    /// Header bytes left out of the packet (ADTS) or 0 when the decoder wants the frame as is (MP3).
    pub strip: usize,
    /// Stream parameters that must not change from frame to frame (sample rate, channel layout...).
    pub key: u32,
}

/// A frame format: how to recognise a header.
pub(crate) trait FrameFormat {
    /// Bytes `parse` needs.
    const NEED: usize;
    /// Parse a header at the start of `h` (at least `NEED` bytes unless the data ends); `None` if it is not one.
    fn parse(h: &[u8]) -> Option<FrameHdr>;
}

const STEP: u64 = 16;
/// Files up to this size are walked completely when they are opened (exact duration, a full seek index).
const FULL_SCAN: u64 = 24 << 20;

/// The frame walker shared by [`crate::Mp3Demuxer`] and [`crate::AdtsDemuxer`].
pub(crate) struct Framed<S: Source, F: FrameFormat> {
    rd: Reader<S>,
    win: Win,
    /// First audio frame.
    start: u64,
    /// End of the audio (before an ID3v1 tag), if known.
    end: Option<u64>,
    /// Next frame to read.
    pos: u64,
    /// Its index (0 is the first audio frame).
    idx: u64,
    key: u32,
    rate: u32,
    spf: u32,
    /// Position of frames 0, STEP, 2 * STEP... as far as they are known.
    index: Vec<u64>,
    /// Frames in the file, once the whole of it has been walked.
    total: Option<u64>,
    /// Timestamp of the first frame (negative: encoder delay to be trimmed).
    pub(crate) first_pts_us: i64,
    /// Where the valid audio ends (the rest is encoder padding), if the file says.
    pub(crate) end_us: Option<i64>,
    _f: core::marker::PhantomData<F>,
}

impl<S: Source, F: FrameFormat> Framed<S, F> {
    /// Find the first frame at or after `from` (needs two in a row to count) and set up the walker there.
    pub(crate) async fn find(
        mut rd: Reader<S>,
        from: u64,
        end: Option<u64>,
    ) -> Result<(Self, FrameHdr, u64)> {
        let mut win = Win::new();
        let mut p = from;
        let limit = from.saturating_add(1 << 20); // no frame in the first megabyte: not this format
        while p < limit {
            let h = win.get(&mut rd, p, F::NEED).await?;
            if h.len() < F::NEED.min(4) {
                break;
            }
            if let Some(hdr) = F::parse(h) {
                if Self::chain_ok(&mut win, &mut rd, p, &hdr, end).await? {
                    let me = Self {
                        rd,
                        win,
                        start: p,
                        end,
                        pos: p,
                        idx: 0,
                        key: hdr.key,
                        rate: 0,
                        spf: hdr.samples,
                        index: alloc::vec![p],
                        total: None,
                        first_pts_us: 0,
                        end_us: None,
                        _f: core::marker::PhantomData,
                    };
                    return Ok((me, hdr, p));
                }
            }
            p += 1;
        }
        Err(Error::Unsupported("no audio frames found".to_string()))
    }

    /// Does a frame of the same stream follow the one at `p`? (The data ending right after it counts.)
    async fn chain_ok(
        win: &mut Win,
        rd: &mut Reader<S>,
        p: u64,
        hdr: &FrameHdr,
        end: Option<u64>,
    ) -> Result<bool> {
        let next = p + hdr.len as u64;
        if end.is_some_and(|e| next >= e) {
            return Ok(true);
        }
        let h = win.get(rd, next, F::NEED).await?;
        if h.is_empty() {
            return Ok(true);
        }
        Ok(F::parse(h).is_some_and(|n| n.key == hdr.key))
    }

    /// Set the sample rate and move the start past a frame that is not audio (a Xing header).
    pub(crate) fn configure(&mut self, rate: u32, start: u64) {
        self.rate = rate;
        self.start = start;
        self.pos = start;
        self.idx = 0;
        self.index = alloc::vec![start];
    }

    pub(crate) fn spf(&self) -> u32 {
        self.spf
    }

    pub(crate) async fn bytes_at(&mut self, off: u64, n: usize) -> Result<&[u8]> {
        self.win.get(&mut self.rd, off, n).await
    }

    fn frame_us(&self, i: u64) -> i64 {
        (i as u128 * self.spf as u128 * 1_000_000 / self.rate.max(1) as u128) as i64 + self.first_pts_us
    }

    /// Walk the whole file once: counts the frames and fills the seek index. Gives up on a huge file (the duration is then
    /// estimated by the caller).
    pub(crate) async fn scan(&mut self) -> Result<()> {
        let size = self.rd.refresh_size().await;
        let stop = self.end.or(size).unwrap_or(u64::MAX);
        if stop.saturating_sub(self.start) > FULL_SCAN {
            return Ok(());
        }
        let (mut pos, mut idx) = (self.start, 0u64);
        let mut index = alloc::vec![pos];
        // A cut-off last frame is not counted.
        while let Some((p, hdr)) = self.next_header(pos).await? {
            if (p + hdr.len as u64) > stop {
                break;
            }
            idx += 1;
            pos = p + hdr.len as u64;
            if idx % STEP == 0 {
                index.push(pos);
            }
        }
        self.index = index;
        self.total = Some(idx);
        Ok(())
    }

    /// The next frame at or after `from`, skipping whatever is not one (junk, a tag, a different stream). `None` at the end.
    async fn next_header(&mut self, mut from: u64) -> Result<Option<(u64, FrameHdr)>> {
        loop {
            if self.end.is_some_and(|e| from >= e) {
                return Ok(None);
            }
            let h = self.win.get(&mut self.rd, from, F::NEED).await?;
            if h.len() < F::NEED.min(4) {
                return Ok(None);
            }
            if let Some(hdr) = F::parse(h) {
                if hdr.key == self.key
                    && Self::chain_ok(&mut self.win, &mut self.rd, from, &hdr, self.end).await?
                {
                    return Ok(Some((from, hdr)));
                }
            }
            from += 1;
        }
    }

    pub(crate) fn duration_us(&self, estimate_bits_per_sec: Option<u64>) -> Option<Timestamp> {
        if let Some(n) = self.total {
            return Some(self.frame_us(n) - self.first_pts_us);
        }
        let size = self.end.or(self.rd.size())?;
        estimate_bits_per_sec
            .filter(|&b| b > 0)
            .map(|b| (size.saturating_sub(self.start) * 8 * 1_000_000 / b) as i64)
    }

    pub(crate) async fn next(&mut self, stream_id: u32) -> Result<Option<Packet>> {
        let Some((p, hdr)) = self.next_header(self.pos).await? else { return Ok(None) };
        let n = hdr.len;
        let data = self.win.get(&mut self.rd, p, n).await?;
        if data.len() < n {
            // The data ends inside this frame: a file that was cut short (or is still being written).
            self.rd.refresh_size().await;
            return Err(Error::Truncated);
        }
        let payload = data[hdr.strip.min(n)..].to_vec();
        let (idx, pos) = (self.idx, p + n as u64);
        if idx % STEP == 0 && (idx / STEP) as usize == self.index.len() {
            self.index.push(p);
        }
        self.pos = pos;
        self.idx = idx + 1;
        let pts = self.frame_us(idx);
        let dur = self.frame_us(idx + 1) - pts;
        let discard = self.end_us.map_or(0, |e| (pts + dur - e).clamp(0, dur));
        Ok(Some(Packet {
            stream_id,
            pts,
            dts: pts,
            duration: dur,
            keyframe: true,
            discard_end_us: discard,
            data: payload,
        }))
    }

    /// Land on a frame before `target_us` (one frame earlier than asked, so a decoder that needs the frame before has it).
    pub(crate) async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        let t = (target_us - self.first_pts_us).max(0) as u128;
        let want = (t * self.rate as u128 / 1_000_000 / self.spf.max(1) as u128) as u64;
        let want = want.saturating_sub(1).min(self.total.map_or(u64::MAX, |n| n.saturating_sub(1)));
        // Start from the nearest indexed frame at or before the one wanted; walk on from the last known one if the file has
        // not been walked that far.
        let k = ((want / STEP) as usize).min(self.index.len() - 1);
        let (mut idx, mut pos) = (k as u64 * STEP, self.index[k]);
        while idx < want {
            let Some((p, hdr)) = self.next_header(pos).await? else { break };
            if idx % STEP == 0 && (idx / STEP) as usize == self.index.len() {
                self.index.push(p);
            }
            pos = p + hdr.len as u64;
            idx += 1;
        }
        self.pos = pos;
        self.idx = idx;
        Ok(self.frame_us(idx))
    }
}

/// What the file-level wrappers need from the engine, so the two demuxers stay thin.
pub(crate) struct Common {
    pub streams: Vec<StreamInfo>,
    pub meta: Metadata,
    pub duration: Option<Timestamp>,
}

pub(crate) fn audio_stream(
    codec: &str,
    rate: u32,
    channels: u16,
    extra: Vec<u8>,
    duration: Option<Timestamp>,
) -> StreamInfo {
    StreamInfo {
        id: 1,
        kind: StreamKind::Audio,
        codec: codec.to_string(),
        time_base: Rational::new(1, 1_000_000),
        language: None,
        extra_data: extra,
        video: None,
        audio: Some(AudioInfo { sample_rate: rate, channels }),
        duration_us: duration,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// MPEG audio layer III

/// Layer III frame headers (MPEG 1, 2 and 2.5).
pub(crate) struct Mpeg3;

const MP3_BR_V1: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
const MP3_BR_V2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

impl Mpeg3 {
    pub(crate) fn rate(h: &[u8]) -> u32 {
        let ver = (h[1] >> 3) & 3;
        let i = ((h[2] >> 2) & 3) as usize;
        let base = [44_100, 48_000, 32_000][i.min(2)];
        match ver {
            3 => base,
            2 => base / 2,
            _ => base / 4,
        }
    }

    pub(crate) fn channels(h: &[u8]) -> u16 {
        if h[3] >> 6 == 3 { 1 } else { 2 }
    }

    pub(crate) fn bitrate(h: &[u8]) -> u32 {
        let i = (h[2] >> 4) as usize;
        1000 * if (h[1] >> 3) & 3 == 3 { MP3_BR_V1[i.min(14)] } else { MP3_BR_V2[i.min(14)] }
    }
}

impl FrameFormat for Mpeg3 {
    const NEED: usize = 4;

    fn parse(h: &[u8]) -> Option<FrameHdr> {
        if h.len() < 4 || h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
            return None;
        }
        let ver = (h[1] >> 3) & 3; // 0 = 2.5, 1 reserved, 2 = 2, 3 = 1
        let layer = (h[1] >> 1) & 3; // 1 = III
        let (br, sr) = (h[2] >> 4, (h[2] >> 2) & 3);
        if ver == 1 || layer != 1 || br == 0 || br == 15 || sr == 3 {
            return None;
        }
        let pad = ((h[2] >> 1) & 1) as u32;
        let v1 = ver == 3;
        let rate = Self::rate(h);
        let len = (if v1 { 144 } else { 72 }) * Self::bitrate(h) / rate + pad;
        let mono = h[3] >> 6 == 3;
        Some(FrameHdr {
            len: len as usize,
            samples: if v1 { 1152 } else { 576 },
            strip: 0,
            key: (ver as u32) << 8 | sr as u32 | (mono as u32) << 4,
        })
    }
}

/// The MPEG audio (Xing/Info/LAME) header in a first frame: frames, bytes and encoder delay and padding.
#[derive(Default)]
struct XingInfo {
    frames: Option<u32>,
    delay: Option<u32>,
    padding: Option<u32>,
}

fn parse_xing(frame: &[u8]) -> Option<XingInfo> {
    if frame.len() < 8 {
        return None;
    }
    let v1 = (frame[1] >> 3) & 3 == 3;
    let mono = frame[3] >> 6 == 3;
    let crc = if frame[1] & 1 == 0 { 2 } else { 0 };
    let side = match (v1, mono) {
        (true, true) => 17,
        (true, false) => 32,
        (false, true) => 9,
        (false, false) => 17,
    };
    let off = 4 + crc + side;
    let tag = frame.get(off..)?;
    if tag.len() < 8 || !(&tag[..4] == b"Xing" || &tag[..4] == b"Info") {
        // Fraunhofer VBRI: fixed position, frames count at +14.
        let v = frame.get(36..)?;
        if v.len() >= 18 && &v[..4] == b"VBRI" {
            return Some(XingInfo {
                frames: Some(u32::from_be_bytes([v[14], v[15], v[16], v[17]])),
                ..Default::default()
            });
        }
        return None;
    }
    let be = |o: usize| tag.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let flags = be(4)?;
    let mut o = 8;
    let mut info = XingInfo::default();
    if flags & 1 != 0 {
        info.frames = be(o);
        o += 4;
    }
    if flags & 2 != 0 {
        o += 4;
    }
    if flags & 4 != 0 {
        o += 100;
    }
    if flags & 8 != 0 {
        o += 4;
    }
    // The LAME extension: 9 bytes of encoder name, then fields; delay and padding are 12 bits each at +21.
    if let Some(e) = tag.get(o..o + 24) {
        if e.starts_with(b"LAME") || e.starts_with(b"Lavf") || e.starts_with(b"Lavc") {
            let (d, p) =
                (((e[21] as u32) << 4) | (e[22] as u32 >> 4), ((e[22] as u32 & 15) << 8) | e[23] as u32);
            if d <= 2880 && p <= 4096 {
                info.delay = Some(d);
                info.padding = Some(p);
            }
        }
    }
    Some(info)
}

/// Layer III in a raw `.mp3` file: ID3v2/ID3v1 tags, Xing/Info/LAME header (frame count, gapless trimming), exact seeks.
///
/// The encoder delay and the decoder delay (529 samples) become a negative first timestamp, which the player trims by
/// dropping audio before time zero; the padding at the end becomes `discard_end_us` on the last frames.
pub struct Mp3Demuxer<S: Source> {
    f: Framed<S, Mpeg3>,
    c: Common,
}

impl<S: Source> Mp3Demuxer<S> {
    /// Read the tags and the first frames of `src`.
    pub async fn open(src: S) -> Result<Self> {
        Self::open_with(src, false).await
    }

    /// Like [`Mp3Demuxer::open`], but with `quick` the file is not walked: the duration comes from the Xing/LAME frame count,
    /// or is estimated from the size and the first frame's bit rate. Meant for scanning a library, where reading every file
    /// whole would cost far more than the tags are worth.
    pub async fn open_with(src: S, quick: bool) -> Result<Self> {
        let mut rd = Reader::new(src).await;
        let mut meta = Metadata::default();
        let start = read_id3v2(&mut rd, &mut meta).await?;
        let size = rd.size();
        let end = read_id3v1(&mut rd, &mut meta, size).await?;
        let (mut f, hdr, pos) = Framed::<S, Mpeg3>::find(rd, start, end).await?;
        let head = f.bytes_at(pos, hdr.len.min(2048)).await?.to_vec();
        let (rate, channels) = (Mpeg3::rate(&head), Mpeg3::channels(&head));
        let xing = parse_xing(&head);
        let audio_start = if xing.is_some() { pos + hdr.len as u64 } else { pos };
        f.configure(rate, audio_start);
        let spf = f.spf() as u64;
        let mut duration_hint = None;
        if let Some(x) = &xing {
            if let (Some(d), Some(p)) = (x.delay, x.padding) {
                f.first_pts_us = -(((d + 529) as u64 * 1_000_000 / rate as u64) as i64);
                if let Some(n) = x.frames {
                    let valid = (n as u64 * spf).saturating_sub(d as u64 + p as u64);
                    f.end_us = Some((valid * 1_000_000 / rate as u64) as i64);
                    duration_hint = f.end_us;
                }
            } else if let Some(n) = x.frames {
                duration_hint = Some((n as u64 * spf * 1_000_000 / rate as u64) as i64);
            }
        }
        if !quick {
            f.scan().await?;
        }
        let duration = duration_hint.or_else(|| f.duration_us(Some(Mpeg3::bitrate(&head) as u64)));
        let c = Common {
            streams: alloc::vec![audio_stream("mp3", rate, channels, Vec::new(), duration)],
            meta,
            duration,
        };
        Ok(Self { f, c })
    }
}

/// Read ID3v2 tags at the start of the file (there can be several); returns where the audio starts.
pub(crate) async fn read_id3v2<S: Source>(rd: &mut Reader<S>, meta: &mut Metadata) -> Result<u64> {
    let mut at = 0u64;
    loop {
        let mut h = [0u8; 10];
        if rd.read_upto(at, &mut h).await? < 10 {
            return Ok(at);
        }
        let Some(len) = tags::id3v2_len(&h) else { return Ok(at) };
        if len > tags::MAX_ID3 {
            // Too big to parse: skip it.
            at += len;
            continue;
        }
        match rd.read_vec(at, len).await {
            Ok(tag) => tags::parse_id3v2(&tag, meta),
            Err(Error::Truncated) => return Ok(at + len),
            Err(e) => return Err(e),
        }
        at += len;
    }
}

/// Read an ID3v1 tag at the end of the file, if there is one; returns where the audio ends.
pub(crate) async fn read_id3v1<S: Source>(
    rd: &mut Reader<S>,
    meta: &mut Metadata,
    size: Option<u64>,
) -> Result<Option<u64>> {
    let Some(size) = size else { return Ok(None) };
    if size < 128 {
        return Ok(Some(size));
    }
    let mut b = [0u8; 128];
    if rd.read_upto(size - 128, &mut b).await? == 128 && &b[..3] == b"TAG" {
        tags::parse_id3v1(&b, meta);
        return Ok(Some(size - 128));
    }
    Ok(Some(size))
}

impl<S: Source> Demuxer for Mp3Demuxer<S> {
    fn streams(&self) -> &[StreamInfo] {
        &self.c.streams
    }
    fn duration_us(&self) -> Option<Timestamp> {
        self.c.duration
    }
    fn metadata(&self) -> &Metadata {
        &self.c.meta
    }
    fn chapters(&self) -> &[Chapter] {
        &[]
    }
    async fn next_packet(&mut self) -> Result<Option<Packet>> {
        self.f.next(1).await
    }
    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        self.f.seek(target_us).await
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// ADTS AAC

/// ADTS frame headers.
pub(crate) struct Adts;

const ADTS_RATES: [u32; 13] =
    [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

impl FrameFormat for Adts {
    const NEED: usize = 7;

    fn parse(h: &[u8]) -> Option<FrameHdr> {
        if h.len() < 7 || h[0] != 0xFF || h[1] & 0xF6 != 0xF0 {
            return None;
        }
        let sfi = (h[2] >> 2) & 15;
        let chan = ((h[2] & 1) << 2) | (h[3] >> 6);
        let len = (((h[3] & 3) as usize) << 11) | ((h[4] as usize) << 3) | (h[5] as usize >> 5);
        let head = if h[1] & 1 == 0 { 9 } else { 7 };
        if sfi as usize >= ADTS_RATES.len() || chan == 0 || len <= head {
            return None;
        }
        let blocks = (h[6] & 3) as u32 + 1;
        Some(FrameHdr {
            len,
            samples: 1024 * blocks,
            strip: head,
            key: (h[2] >> 6) as u32 | (sfi as u32) << 2 | (chan as u32) << 8 | ((h[1] >> 3 & 1) as u32) << 12,
        })
    }
}

/// An AAC elementary stream in ADTS frames (`.aac`), with optional ID3v2/ID3v1 tags.
pub struct AdtsDemuxer<S: Source> {
    f: Framed<S, Adts>,
    c: Common,
}

impl<S: Source> AdtsDemuxer<S> {
    /// Read the tags and the first frames of `src`.
    pub async fn open(src: S) -> Result<Self> {
        Self::open_with(src, false).await
    }

    /// Like [`AdtsDemuxer::open`]; with `quick` the file is not walked and the duration is estimated from the first frame.
    pub async fn open_with(src: S, quick: bool) -> Result<Self> {
        let mut rd = Reader::new(src).await;
        let mut meta = Metadata::default();
        let start = read_id3v2(&mut rd, &mut meta).await?;
        let size = rd.size();
        let end = read_id3v1(&mut rd, &mut meta, size).await?;
        let (mut f, hdr, pos) = Framed::<S, Adts>::find(rd, start, end).await?;
        let h = f.bytes_at(pos, 7).await?.to_vec();
        let (profile, sfi, chan) = (h[2] >> 6, (h[2] >> 2) & 15, ((h[2] & 1) << 2) | (h[3] >> 6));
        let rate = ADTS_RATES[sfi as usize];
        // AudioSpecificConfig: object type (profile + 1), sampling frequency index, channel configuration.
        let asc = (((profile as u16) + 1) << 11) | ((sfi as u16) << 7) | ((chan as u16) << 3);
        let channels = if chan == 7 { 8 } else { chan as u16 };
        f.configure(rate, pos);
        if !quick {
            f.scan().await?;
        }
        // Beyond the full-scan size: estimate from the first frame's size (a constant bit rate is the usual case).
        let bits = hdr.len as u64 * 8 * rate as u64 / hdr.samples.max(1) as u64;
        let duration = f.duration_us(Some(bits));
        let c = Common {
            streams: alloc::vec![audio_stream("aac", rate, channels, asc.to_be_bytes().to_vec(), duration)],
            meta,
            duration,
        };
        Ok(Self { f, c })
    }
}

impl<S: Source> Demuxer for AdtsDemuxer<S> {
    fn streams(&self) -> &[StreamInfo] {
        &self.c.streams
    }
    fn duration_us(&self) -> Option<Timestamp> {
        self.c.duration
    }
    fn metadata(&self) -> &Metadata {
        &self.c.meta
    }
    fn chapters(&self) -> &[Chapter] {
        &[]
    }
    async fn next_packet(&mut self) -> Result<Option<Packet>> {
        self.f.next(1).await
    }
    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        self.f.seek(target_us).await
    }
}
