//! Matroska / WebM demuxer.
//!
//! Open reads `Info` and `Tracks` and walks the top-level elements of the segment once, recording where
//! every `Cluster` starts and its timestamp (a few bytes per cluster). That index replaces `Cues` for seeking,
//! so files without cues, or with cues at the end, seek the same way. Blocks are read lazily in file order.
//!
//! Limits: no content encodings (header stripping/compression), no attachments, chapters or tags yet, and an
//! unknown-size cluster (live streams) is indexed as it is reached, so seeking inside one is approximate.
use crate::Demuxer;
use crate::io::{Cur, Reader, invalid};
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_core::{AudioInfo, Error, Packet, Rational, Result, StreamInfo, StreamKind, Timestamp, VideoInfo};
use rvp_host::Source;

const ID_EBML: u32 = 0x1A45_DFA3;
const ID_DOCTYPE: u32 = 0x4282;
const ID_SEGMENT: u32 = 0x1853_8067;
const ID_INFO: u32 = 0x1549_A966;
const ID_TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const ID_DURATION: u32 = 0x4489;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_TRACK_ENTRY: u32 = 0xAE;
const ID_TRACK_NUMBER: u32 = 0xD7;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_CODEC_ID: u32 = 0x86;
const ID_CODEC_PRIVATE: u32 = 0x63A2;
const ID_DEFAULT_DURATION: u32 = 0x23_E383;
const ID_LANGUAGE: u32 = 0x22_B59C;
const ID_CODEC_DELAY: u32 = 0x56AA;
const ID_VIDEO: u32 = 0xE0;
const ID_PIXEL_WIDTH: u32 = 0xB0;
const ID_PIXEL_HEIGHT: u32 = 0xBA;
const ID_AUDIO: u32 = 0xE1;
const ID_SAMPLING_FREQUENCY: u32 = 0xB5;
const ID_CHANNELS: u32 = 0x9F;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_CLUSTER_TIMESTAMP: u32 = 0xE7;
const ID_SIMPLE_BLOCK: u32 = 0xA3;
const ID_BLOCK_GROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;
const ID_BLOCK_DURATION: u32 = 0x9B;
const ID_REFERENCE_BLOCK: u32 = 0xFB;

#[derive(Debug, Clone, Copy)]
struct Hdr {
    id: u32,
    /// `None` = unknown size.
    size: Option<u64>,
    hlen: u64,
}

/// Parse an element header from the start of `b`.
fn parse_hdr(b: &[u8]) -> Result<Hdr> {
    let first = *b.first().ok_or(Error::Truncated)?;
    let idlen = first.leading_zeros() as usize + 1;
    if idlen > 4 {
        return invalid("bad EBML element id");
    }
    if b.len() < idlen + 1 {
        return Err(Error::Truncated);
    }
    let id = b[..idlen].iter().fold(0u32, |a, &x| (a << 8) | x as u32);
    let (size, slen) = vint(&b[idlen..])?;
    Ok(Hdr { id, size, hlen: (idlen + slen) as u64 })
}

/// Parse a size-style variable-length integer. Returns (`None` if all value bits are ones, length).
fn vint(b: &[u8]) -> Result<(Option<u64>, usize)> {
    let first = *b.first().ok_or(Error::Truncated)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return invalid("bad EBML variable-length integer");
    }
    if b.len() < len {
        return Err(Error::Truncated);
    }
    let mut v = (first as u64) & ((1u64 << (8 - len)) - 1);
    for &x in &b[1..len] {
        v = (v << 8) | x as u64;
    }
    let all_ones = v == (1u64 << (7 * len)) - 1;
    Ok((if all_ones { None } else { Some(v) }, len))
}

fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0u64, |a, &x| (a << 8) | x as u64)
}

fn float(b: &[u8]) -> f64 {
    match b.len() {
        4 => f32::from_be_bytes(b.try_into().unwrap()) as f64,
        8 => f64::from_be_bytes(b.try_into().unwrap()),
        _ => 0.0,
    }
}

/// Iterate the children of an in-memory master element.
fn children(body: &[u8]) -> Result<Vec<(u32, &[u8])>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < body.len() {
        let h = parse_hdr(&body[pos..])?;
        let start = pos + h.hlen as usize;
        let end = match h.size {
            Some(s) => start
                .checked_add(usize::try_from(s).map_err(|_| Error::Truncated)?)
                .ok_or(Error::Truncated)?,
            None => body.len(),
        };
        if end > body.len() {
            return Err(Error::Truncated);
        }
        out.push((h.id, &body[start..end]));
        pos = end;
    }
    Ok(out)
}

struct MkTrack {
    number: u64,
    default_duration_ns: u64,
    /// Encoder delay (Opus pre-skip) in timescale ticks; subtracted from every timestamp of the track.
    delay_ticks: i64,
}

#[derive(Debug, Clone, Copy)]
struct ClusterIdx {
    /// First byte after the element header.
    data_start: u64,
    /// First byte after the cluster.
    end: u64,
    /// Cluster timestamp in timescale ticks.
    ts: i64,
}

/// A keyframe found while scanning a cluster during a seek.
struct Hit {
    pts_ticks: i64,
    offset: u64,
}

fn codec_name(id: &str) -> String {
    let n = match id {
        "V_MPEG4/ISO/AVC" => "h264",
        "V_MPEGH/ISO/HEVC" => "hevc",
        "V_AV1" => "av1",
        "V_VP9" => "vp9",
        "V_VP8" => "vp8",
        "A_OPUS" => "opus",
        "A_VORBIS" => "vorbis",
        "A_FLAC" => "flac",
        "A_MPEG/L3" => "mp3",
        "A_AC3" => "ac3",
        "A_EAC3" => "eac3",
        "S_TEXT/UTF8" => "subrip",
        "S_TEXT/WEBVTT" => "webvtt",
        "S_TEXT/ASS" | "S_TEXT/SSA" => "ass",
        s if s.starts_with("A_AAC") => "aac",
        s if s.starts_with("V_MPEG4/ISO/") => "mpeg4",
        s => return s.to_lowercase(),
    };
    n.to_string()
}

fn parse_track_entry(body: &[u8], tb: Rational, timescale_ns: u64) -> Result<Option<(MkTrack, StreamInfo)>> {
    let (mut number, mut ty, mut codec_id, mut private) = (0u64, 0u64, String::new(), Vec::new());
    let (mut default_ns, mut language, mut codec_delay_ns) = (0u64, None, 0u64);
    let (mut video, mut audio) = (None, None);
    for (id, b) in children(body)? {
        match id {
            ID_TRACK_NUMBER => number = uint(b),
            ID_TRACK_TYPE => ty = uint(b),
            ID_CODEC_ID => codec_id = String::from_utf8_lossy(b).into_owned(),
            ID_CODEC_PRIVATE => private = b.to_vec(),
            ID_DEFAULT_DURATION => default_ns = uint(b),
            ID_CODEC_DELAY => codec_delay_ns = uint(b),
            ID_LANGUAGE => {
                let l = String::from_utf8_lossy(b).into_owned();
                language = (l != "und").then_some(l);
            }
            ID_VIDEO => {
                let (mut w, mut h) = (0, 0);
                for (id, b) in children(b)? {
                    match id {
                        ID_PIXEL_WIDTH => w = uint(b) as u32,
                        ID_PIXEL_HEIGHT => h = uint(b) as u32,
                        _ => {}
                    }
                }
                video = Some(VideoInfo { width: w, height: h });
            }
            ID_AUDIO => {
                let (mut rate, mut ch) = (8000.0, 1);
                for (id, b) in children(b)? {
                    match id {
                        ID_SAMPLING_FREQUENCY => rate = float(b),
                        ID_CHANNELS => ch = uint(b) as u16,
                        _ => {}
                    }
                }
                audio = Some(AudioInfo { sample_rate: rate as u32, channels: ch });
            }
            _ => {}
        }
    }
    let kind = match ty {
        1 => StreamKind::Video,
        2 => StreamKind::Audio,
        0x11 => StreamKind::Subtitle,
        _ => return Ok(None),
    };
    if number == 0 {
        return invalid("track without a number");
    }
    let info = StreamInfo {
        id: number as u32,
        kind,
        codec: codec_name(&codec_id),
        time_base: tb,
        language,
        extra_data: private,
        video: if kind == StreamKind::Video { video } else { None },
        audio: if kind == StreamKind::Audio { audio } else { None },
        duration_us: None,
    };
    Ok(Some((
        MkTrack {
            number,
            default_duration_ns: default_ns,
            delay_ticks: ((codec_delay_ns as u128 * 2 + timescale_ns as u128)
                / (2 * timescale_ns.max(1) as u128)) as i64,
        },
        info,
    )))
}

/// A Matroska/WebM demuxer over a host [`Source`].
pub struct MkvDemuxer<S: Source> {
    rd: Reader<S>,
    streams: Vec<StreamInfo>,
    tracks: Vec<MkTrack>,
    time_base: Rational,
    duration_us: Option<Timestamp>,
    doc_type: String,
    clusters: Vec<ClusterIdx>,
    // Read position.
    cur: Option<usize>,
    pos: u64,
    cluster_end: u64,
    cluster_ts: i64,
    pending: VecDeque<Packet>,
}

impl<S: Source> MkvDemuxer<S> {
    /// Parse headers and index the clusters.
    pub async fn open(src: S) -> Result<Self> {
        let mut rd = Reader::new(src).await;
        let mut hb = [0u8; 16];
        let n = rd.read_upto(0, &mut hb).await?;
        let h = parse_hdr(&hb[..n])?;
        if h.id != ID_EBML {
            return Err(Error::Unsupported("not a Matroska/WebM file".to_string()));
        }
        let ebml_len = h.size.ok_or(Error::Invalid("EBML header of unknown size".to_string()))?;
        let ebml = rd.read_vec(h.hlen, ebml_len).await?;
        let doc_type = children(&ebml)?
            .into_iter()
            .find(|(id, _)| *id == ID_DOCTYPE)
            .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();

        let seg_pos = h.hlen + ebml_len;
        let n = rd.read_upto(seg_pos, &mut hb).await?;
        let sh = parse_hdr(&hb[..n])?;
        if sh.id != ID_SEGMENT {
            return invalid("no Segment after the EBML header");
        }
        let data_start = seg_pos + sh.hlen;
        let seg_end = match (sh.size, rd.size()) {
            (Some(s), Some(len)) => (data_start + s).min(len),
            (Some(s), None) => data_start + s,
            (None, Some(len)) => len,
            (None, None) => u64::MAX,
        };

        let mut timescale = 1_000_000u64;
        let mut duration_ticks = 0.0f64;
        let mut tracks_body: Option<Vec<u8>> = None;
        let mut clusters: Vec<ClusterIdx> = Vec::new();
        let mut pos = data_start;
        while pos < seg_end {
            let n = rd.read_upto(pos, &mut hb).await?;
            let Ok(h) = parse_hdr(&hb[..n]) else { break };
            let body = pos + h.hlen;
            match h.id {
                ID_INFO | ID_TRACKS => {
                    let Some(size) = h.size else { break };
                    let b = rd.read_vec(body, size).await?;
                    if h.id == ID_TRACKS {
                        tracks_body = Some(b);
                    } else {
                        for (id, v) in children(&b)? {
                            match id {
                                ID_TIMESTAMP_SCALE => timescale = uint(v).max(1),
                                ID_DURATION => duration_ticks = float(v),
                                _ => {}
                            }
                        }
                    }
                }
                ID_CLUSTER => {
                    let c = Self::read_cluster_hdr(&mut rd, pos, h, seg_end).await?;
                    clusters.push(c);
                    if h.size.is_none() {
                        break;
                    }
                }
                _ => {}
            }
            match h.size {
                Some(s) => {
                    pos = body.checked_add(s).ok_or(Error::Invalid("element size overflow".to_string()))?
                }
                None => break,
            }
        }

        let time_base = Rational::new(u32::try_from(timescale).unwrap_or(u32::MAX), 1_000_000_000);
        let mut streams = Vec::new();
        let mut tracks = Vec::new();
        let tracks_body = tracks_body.ok_or(Error::Invalid("segment without Tracks".to_string()))?;
        for (id, b) in children(&tracks_body)? {
            if id == ID_TRACK_ENTRY {
                if let Some((t, s)) = parse_track_entry(b, time_base, timescale)? {
                    tracks.push(t);
                    streams.push(s);
                }
            }
        }
        let duration_us =
            (duration_ticks > 0.0).then(|| (duration_ticks * timescale as f64 / 1000.0 + 0.5) as Timestamp);
        Ok(Self {
            rd,
            streams,
            tracks,
            time_base,
            duration_us,
            doc_type,
            clusters,
            cur: None,
            pos: 0,
            cluster_end: 0,
            cluster_ts: 0,
            pending: VecDeque::new(),
        })
    }

    #[doc(hidden)]
    pub fn debug_clusters(&self) -> alloc::string::String {
        alloc::format!("{:?}", self.clusters)
    }

    /// `"webm"` or `"matroska"`.
    pub fn doc_type(&self) -> &str {
        &self.doc_type
    }

    async fn read_cluster_hdr(rd: &mut Reader<S>, pos: u64, h: Hdr, seg_end: u64) -> Result<ClusterIdx> {
        let data_start = pos + h.hlen;
        let end = h.size.map_or(seg_end, |s| (data_start + s).min(seg_end));
        // The Timestamp comes first, possibly after a CRC-32 or Void element: look through the first 64 bytes.
        let mut b = [0u8; 64];
        let n = rd.read_upto(data_start, &mut b).await?;
        let (mut ts, mut at) = (0i64, 0usize);
        while at < n {
            let Ok(c) = parse_hdr(&b[at..n]) else { break };
            let (o, Some(sz)) = (at + c.hlen as usize, c.size) else { break };
            if c.id == ID_CLUSTER_TIMESTAMP {
                if o + sz.min(8) as usize <= n {
                    ts = uint(&b[o..o + sz.min(8) as usize]) as i64;
                }
                break;
            }
            at = o + sz as usize;
        }
        Ok(ClusterIdx { data_start, end, ts })
    }

    fn enter_cluster(&mut self, i: usize) {
        let c = self.clusters[i];
        self.cur = Some(i);
        self.pos = c.data_start;
        self.cluster_end = c.end;
        self.cluster_ts = c.ts;
    }

    fn track_index(&self, number: u64) -> Option<usize> {
        self.tracks.iter().position(|t| t.number == number)
    }

    /// Decode a (Simple)Block body into packets.
    fn parse_block(
        &self,
        body: &[u8],
        cluster_ts: i64,
        key: Option<bool>,
        dur_ticks: Option<i64>,
    ) -> Result<Vec<Packet>> {
        let (tn, tl) = vint(body)?;
        let tn = tn.ok_or(Error::Invalid("block track number".to_string()))?;
        let mut c = Cur::new(&body[tl..]);
        let tc = c.u16()? as i16 as i64;
        let flags = c.u8()?;
        let Some(ti) = self.track_index(tn) else { return Ok(Vec::new()) };
        let keyframe = key.unwrap_or(flags & 0x80 != 0);
        let default_ticks = (self.tracks[ti].default_duration_ns / self.time_base.num.max(1) as u64)
            .min(i64::MAX as u64) as i64;
        let mut sizes: Vec<usize> = Vec::new();
        let lacing = (flags >> 1) & 3;
        if lacing == 0 {
            sizes.push(c.remaining());
        } else {
            let n = c.u8()? as usize + 1;
            match lacing {
                1 => {
                    let mut total = 0;
                    for _ in 0..n - 1 {
                        let mut s = 0;
                        loop {
                            let b = c.u8()? as usize;
                            s += b;
                            if b != 255 {
                                break;
                            }
                        }
                        total += s;
                        sizes.push(s);
                    }
                    sizes.push(c.remaining().checked_sub(total).ok_or(Error::Truncated)?);
                }
                3 => {
                    let rest = c.rest();
                    let (first, l) = vint(rest)?;
                    let mut at = l;
                    let mut cur = first.ok_or(Error::Invalid("lace size".to_string()))? as i64;
                    let mut total = cur as usize;
                    sizes.push(cur as usize);
                    for _ in 1..n - 1 {
                        let (v, l) = vint(rest.get(at..).ok_or(Error::Truncated)?)?;
                        let raw = v.ok_or(Error::Invalid("lace size".to_string()))? as i64;
                        cur += raw - ((1i64 << (7 * l - 1)) - 1);
                        if cur < 0 {
                            return invalid("negative lace size");
                        }
                        at += l;
                        total += cur as usize;
                        sizes.push(cur as usize);
                    }
                    let payload = rest.len().checked_sub(at).ok_or(Error::Truncated)?;
                    sizes.push(payload.checked_sub(total).ok_or(Error::Truncated)?);
                    // Re-point the cursor at the payload start.
                    let hdr_used = body.len() - payload;
                    return self.finish_laced(
                        body,
                        hdr_used,
                        &sizes,
                        ti,
                        cluster_ts + tc,
                        keyframe,
                        default_ticks,
                    );
                }
                _ => {
                    let each = c.remaining() / n;
                    sizes = alloc::vec![each; n];
                }
            }
        }
        let hdr_used = body.len() - c.remaining();
        // For non-EBML lacing, the cursor is already at the payload.
        let mut out = Vec::new();
        let mut off = hdr_used;
        for (i, s) in sizes.iter().enumerate() {
            let data = body.get(off..off + s).ok_or(Error::Truncated)?.to_vec();
            off += s;
            out.push(self.make_packet(
                ti,
                cluster_ts + tc + i as i64 * default_ticks,
                keyframe,
                dur_ticks.unwrap_or(default_ticks),
                data,
            ));
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_laced(
        &self,
        body: &[u8],
        mut off: usize,
        sizes: &[usize],
        ti: usize,
        ts: i64,
        key: bool,
        dur: i64,
    ) -> Result<Vec<Packet>> {
        let mut out = Vec::new();
        for (i, s) in sizes.iter().enumerate() {
            let data = body.get(off..off + s).ok_or(Error::Truncated)?.to_vec();
            off += s;
            out.push(self.make_packet(ti, ts + i as i64 * dur, key, dur, data));
        }
        Ok(out)
    }

    fn make_packet(&self, ti: usize, ticks: i64, keyframe: bool, dur_ticks: i64, data: Vec<u8>) -> Packet {
        let pts = self.time_base.ticks_to_us(ticks - self.tracks[ti].delay_ticks);
        Packet {
            stream_id: self.tracks[ti].number as u32,
            pts,
            dts: pts, // Matroska stores no decode timestamps; decoders reorder from pts.
            duration: self.time_base.ticks_to_us(dur_ticks),
            keyframe,
            data,
        }
    }

    /// Decode a `BlockGroup` body: (block bytes, duration ticks, has reference).
    fn parse_group(&self, body: &[u8], cluster_ts: i64) -> Result<Vec<Packet>> {
        let (mut block, mut dur, mut has_ref) = (None, None, false);
        for (id, b) in children(body)? {
            match id {
                ID_BLOCK => block = Some(b),
                ID_BLOCK_DURATION => dur = Some(uint(b) as i64),
                ID_REFERENCE_BLOCK => has_ref = true,
                _ => {}
            }
        }
        match block {
            Some(b) => self.parse_block(b, cluster_ts, Some(!has_ref), dur),
            None => Ok(Vec::new()),
        }
    }

    /// Primary seek track: first video track, else the first track.
    fn primary(&self) -> usize {
        self.streams.iter().position(|s| s.kind == StreamKind::Video).unwrap_or(0)
    }

    /// List keyframes of `number` inside cluster `idx` (element offsets, so reading can restart there).
    async fn scan_keys(&mut self, idx: usize, number: u64) -> Result<Vec<Hit>> {
        let c = self.clusters[idx];
        let mut pos = c.data_start;
        let mut ts = c.ts;
        let delay = self.tracks.iter().find(|t| t.number == number).map_or(0, |t| t.delay_ticks);
        let mut hits = Vec::new();
        let mut hb = [0u8; 32];
        while pos < c.end {
            let n = self.rd.read_upto(pos, &mut hb).await?;
            let Ok(h) = parse_hdr(&hb[..n]) else { break };
            let body = pos + h.hlen;
            let Some(size) = h.size else { break };
            match h.id {
                ID_CLUSTER_TIMESTAMP => {
                    let v = self.rd.read_vec(body, size.min(8)).await?;
                    ts = uint(&v) as i64;
                }
                ID_SIMPLE_BLOCK => {
                    let b = &hb[h.hlen as usize..n];
                    if let Ok((Some(tn), tl)) = vint(b) {
                        if tn == number && b.len() >= tl + 3 && b[tl + 2] & 0x80 != 0 {
                            let tc = i16::from_be_bytes([b[tl], b[tl + 1]]) as i64;
                            hits.push(Hit { pts_ticks: ts + tc - delay, offset: pos });
                        }
                    }
                }
                ID_BLOCK_GROUP => {
                    let g = self.rd.read_vec(body, size).await?;
                    if let Ok(kids) = children(&g) {
                        let has_ref = kids.iter().any(|(id, _)| *id == ID_REFERENCE_BLOCK);
                        if let Some((_, b)) = kids.iter().find(|(id, _)| *id == ID_BLOCK) {
                            if let Ok((Some(tn), tl)) = vint(b) {
                                if tn == number && !has_ref && b.len() >= tl + 3 {
                                    let tc = i16::from_be_bytes([b[tl], b[tl + 1]]) as i64;
                                    hits.push(Hit { pts_ticks: ts + tc - delay, offset: pos });
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            pos = body + size;
        }
        Ok(hits)
    }
}

impl<S: Source> Demuxer for MkvDemuxer<S> {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn duration_us(&self) -> Option<Timestamp> {
        self.duration_us
    }

    async fn next_packet(&mut self) -> Result<Option<Packet>> {
        let mut hb = [0u8; 16];
        loop {
            if let Some(p) = self.pending.pop_front() {
                return Ok(Some(p));
            }
            if self.cur.is_none() || self.pos >= self.cluster_end {
                let next = self.cur.map_or(0, |c| c + 1);
                if next >= self.clusters.len() {
                    return Ok(None);
                }
                self.enter_cluster(next);
                continue;
            }
            let n = self.rd.read_upto(self.pos, &mut hb).await?;
            let Ok(h) = parse_hdr(&hb[..n]) else {
                self.pos = self.cluster_end;
                continue;
            };
            let body = self.pos + h.hlen;
            if h.id == ID_CLUSTER {
                // Reached the next cluster inside an unknown-size one.
                let seg_end = self.rd.size().unwrap_or(u64::MAX);
                let c = Self::read_cluster_hdr(&mut self.rd, self.pos, h, seg_end).await?;
                self.clusters.push(c);
                let last = self.clusters.len() - 1;
                self.enter_cluster(last);
                continue;
            }
            let Some(size) = h.size else {
                self.pos = self.cluster_end;
                continue;
            };
            match h.id {
                ID_CLUSTER_TIMESTAMP => {
                    let v = self.rd.read_vec(body, size.min(8)).await?;
                    self.cluster_ts = uint(&v) as i64;
                }
                ID_SIMPLE_BLOCK | ID_BLOCK_GROUP => {
                    let b = self.rd.read_vec(body, size).await?;
                    let pkts = if h.id == ID_SIMPLE_BLOCK {
                        self.parse_block(&b, self.cluster_ts, None, None)?
                    } else {
                        self.parse_group(&b, self.cluster_ts)?
                    };
                    self.pending.extend(pkts);
                }
                _ => {}
            }
            self.pos = body + size;
        }
    }

    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        self.pending.clear();
        if self.clusters.is_empty() || self.tracks.is_empty() {
            return Ok(0);
        }
        let number = self.tracks[self.primary()].number;
        let target_ticks = self.time_base.us_to_ticks_floor(target_us);
        let mut idx = self.clusters.partition_point(|c| c.ts <= target_ticks).saturating_sub(1);
        // Walk back until a cluster holds a keyframe at or before the target.
        let mut best: Option<(usize, Hit)> = None;
        loop {
            let hits = self.scan_keys(idx, number).await?;
            if let Some(h) = hits.into_iter().rev().find(|h| h.pts_ticks <= target_ticks) {
                best = Some((idx, h));
                break;
            }
            if idx == 0 {
                break;
            }
            idx -= 1;
        }
        // A keyframe's pts can be below its cluster's timestamp (negative block offsets), so a later
        // cluster may also hold a keyframe at or before the target. Look a little ahead.
        let slack = self.time_base.us_to_ticks_floor(1_000_000);
        let mut j = idx + 1;
        while j < self.clusters.len() && self.clusters[j].ts <= target_ticks + slack {
            if let Some(h) =
                self.scan_keys(j, number).await?.into_iter().rev().find(|h| h.pts_ticks <= target_ticks)
            {
                if best.as_ref().is_none_or(|(_, b)| h.pts_ticks > b.pts_ticks) {
                    best = Some((j, h));
                }
            }
            j += 1;
        }
        let (ci, hit) = match best {
            Some(b) => b,
            None => {
                // Nothing at or before the target: land on the first keyframe in the file.
                let mut found = None;
                for i in 0..self.clusters.len() {
                    if let Some(h) = self.scan_keys(i, number).await?.into_iter().next() {
                        found = Some((i, h));
                        break;
                    }
                }
                match found {
                    Some(f) => f,
                    None => return Ok(0),
                }
            }
        };
        self.enter_cluster(ci);
        self.pos = hit.offset;
        Ok(self.time_base.ticks_to_us(hit.pts_ticks))
    }
}
