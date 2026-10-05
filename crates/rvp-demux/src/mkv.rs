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
use rvp_core::{
    Art, AudioInfo, Chapter, Error, Metadata, Packet, Rational, Result, StreamInfo, StreamKind, Timestamp,
    VideoInfo,
};
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
const ID_DISCARD_PADDING: u32 = 0x75A2;
const ID_TITLE: u32 = 0x7BA9;
const ID_TAGS: u32 = 0x1254_C367;
const ID_TAG: u32 = 0x7373;
const ID_SIMPLE_TAG: u32 = 0x67C8;
const ID_TAG_NAME: u32 = 0x45A3;
const ID_TAG_STRING: u32 = 0x4487;
const ID_CHAPTERS: u32 = 0x1043_A770;
const ID_EDITION_ENTRY: u32 = 0x45B9;
const ID_CHAPTER_ATOM: u32 = 0xB6;
const ID_CHAPTER_TIME_START: u32 = 0x91;
const ID_CHAPTER_DISPLAY: u32 = 0x80;
const ID_CHAP_STRING: u32 = 0x85;
const ID_ATTACHMENTS: u32 = 0x1941_A469;
const ID_ATTACHED_FILE: u32 = 0x61A7;
const ID_FILE_NAME: u32 = 0x466E;
const ID_FILE_MIME: u32 = 0x4660;
const ID_FILE_DATA: u32 = 0x465C;
/// Attachments and tag blocks larger than this are not read.
const MAX_META_BYTES: u64 = 8 << 20;

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
    /// Encoder delay (Opus pre-skip, MP3 decoder delay) in microseconds, subtracted from every timestamp of
    /// the track. Kept at full precision: Matroska ticks are usually 1 ms but delays are a few hundred samples.
    delay_us: i64,
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
    pts_us: Timestamp,
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
        "S_TEXT/WEBVTT" | "D_WEBVTT/SUBTITLES" | "D_WEBVTT/CAPTIONS" => "webvtt",
        "S_TEXT/ASS" | "S_TEXT/SSA" | "S_ASS" | "S_SSA" => "ass",
        "S_HDMV/PGS" => "hdmv_pgs",
        "A_MPEG/L2" => "mp2",
        "A_MPEG/L1" => "mp1",
        s if s.starts_with("A_AAC") => "aac",
        s if s.starts_with("V_MPEG4/ISO/") => "mpeg4",
        s => return s.to_lowercase(),
    };
    n.to_string()
}

fn parse_track_entry(body: &[u8], tb: Rational) -> Result<Option<(MkTrack, StreamInfo)>> {
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
        MkTrack { number, default_duration_ns: default_ns, delay_us: (codec_delay_ns / 1000) as i64 },
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
    meta: Metadata,
    chapters: Vec<Chapter>,
    /// Where the scan for clusters stopped, so a file that grows can be read on.
    scan_pos: u64,
}

/// Read `SimpleTag`s (recursively) into `meta`: TITLE, ARTIST, ALBUM and the loudness tags.
fn read_simple_tags(body: &[u8], meta: &mut Metadata, depth: usize) {
    if depth > MAX_TAG_DEPTH {
        return;
    }
    for (id, b) in children(body).unwrap_or_default() {
        match id {
            ID_TAG => read_simple_tags(b, meta, depth + 1),
            ID_SIMPLE_TAG => simple_tag(b, meta, depth + 1),
            _ => {}
        }
    }
}

/// One `SimpleTag` body: its name and value, then its nested `SimpleTag`s.
fn simple_tag(b: &[u8], meta: &mut Metadata, depth: usize) {
    if depth > MAX_TAG_DEPTH {
        return;
    }
    let (mut name, mut value) = (None, None);
    for (cid, cb) in children(b).unwrap_or_default() {
        match cid {
            ID_TAG_NAME => name = Some(String::from_utf8_lossy(cb).to_ascii_uppercase()),
            ID_TAG_STRING => value = Some(String::from_utf8_lossy(cb).into_owned()),
            ID_SIMPLE_TAG => simple_tag(cb, meta, depth + 1),
            _ => {}
        }
    }
    if let (Some(n), Some(v)) = (name, value) {
        let v = v.trim_end_matches('\0').to_string();
        if v.is_empty() {
            return;
        }
        match n.as_str() {
            "TITLE" => {
                meta.title.get_or_insert(v);
            }
            "ARTIST" => {
                meta.artist.get_or_insert(v);
            }
            "ALBUM" => {
                meta.album.get_or_insert(v);
            }
            other => {
                crate::tags::apply_loudness_tag(meta, other, &v);
            }
        }
    }
}

/// Deepest tag nesting followed.
const MAX_TAG_DEPTH: usize = 8;

/// Deepest chapter nesting followed, and most chapters kept.
const MAX_CHAPTER_DEPTH: usize = 8;
const MAX_CHAPTERS: usize = 10_000;
/// Most clusters indexed (a cluster header is at least a few bytes; this is far beyond any real file).
const MAX_CLUSTERS: usize = 1 << 20;

/// Chapter atoms of every edition, flattened and sorted by start time.
fn read_chapters(body: &[u8], out: &mut Vec<Chapter>) {
    for (id, ed) in children(body).unwrap_or_default() {
        if id != ID_EDITION_ENTRY {
            continue;
        }
        fn atoms(b: &[u8], out: &mut Vec<Chapter>, depth: usize) {
            // Nested atoms are legal but a hostile file can nest them deeper than a small stack allows.
            if depth > MAX_CHAPTER_DEPTH {
                return;
            }
            for (id, a) in children(b).unwrap_or_default() {
                if out.len() >= MAX_CHAPTERS {
                    return;
                }
                if id != ID_CHAPTER_ATOM {
                    continue;
                }
                let (mut start, mut title) = (0i64, String::new());
                for (cid, cb) in children(a).unwrap_or_default() {
                    match cid {
                        ID_CHAPTER_TIME_START => start = uint(cb) as i64 / 1000,
                        ID_CHAPTER_DISPLAY => {
                            if title.is_empty() {
                                for (did, db) in children(cb).unwrap_or_default() {
                                    if did == ID_CHAP_STRING {
                                        title = String::from_utf8_lossy(db).into_owned();
                                    }
                                }
                            }
                        }
                        ID_CHAPTER_ATOM => atoms(a, out, depth + 1),
                        _ => {}
                    }
                }
                out.push(Chapter { start_us: start, title });
            }
        }
        atoms(ed, out, 0);
        break; // the first edition is the default one
    }
    out.sort_by_key(|c| c.start_us);
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
            (Some(s), Some(len)) => data_start.saturating_add(s).min(len),
            (Some(s), None) => data_start.saturating_add(s),
            (None, Some(len)) => len,
            (None, None) => u64::MAX,
        };

        let mut timescale = 1_000_000u64;
        let mut duration_ticks = 0.0f64;
        let mut tracks_body: Option<Vec<u8>> = None;
        let mut clusters: Vec<ClusterIdx> = Vec::new();
        let mut meta = Metadata::default();
        let mut chapters: Vec<Chapter> = Vec::new();
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
                                ID_TITLE => {
                                    let t = String::from_utf8_lossy(v).trim_end_matches('\0').to_string();
                                    if !t.is_empty() {
                                        meta.title = Some(t);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                ID_TAGS | ID_CHAPTERS | ID_ATTACHMENTS if h.size.is_some_and(|s| s <= MAX_META_BYTES) => {
                    let b = rd.read_vec(body, h.size.unwrap_or(0)).await?;
                    match h.id {
                        ID_TAGS => read_simple_tags(&b, &mut meta, 0),
                        ID_CHAPTERS => read_chapters(&b, &mut chapters),
                        _ => {
                            for (id, f) in children(&b).unwrap_or_default() {
                                if id != ID_ATTACHED_FILE {
                                    continue;
                                }
                                let (mut name, mut mime, mut data) = (String::new(), String::new(), None);
                                for (cid, cb) in children(f).unwrap_or_default() {
                                    match cid {
                                        ID_FILE_NAME => {
                                            name = String::from_utf8_lossy(cb).to_ascii_lowercase()
                                        }
                                        ID_FILE_MIME => mime = String::from_utf8_lossy(cb).into_owned(),
                                        ID_FILE_DATA => data = Some(cb.to_vec()),
                                        _ => {}
                                    }
                                }
                                if let Some(data) = data {
                                    if name.starts_with("cover")
                                        && mime.starts_with("image/")
                                        && meta.art.is_none()
                                    {
                                        meta.art = Some(Art { mime, data });
                                    }
                                }
                            }
                        }
                    }
                }
                ID_CLUSTER => {
                    if clusters.len() >= MAX_CLUSTERS {
                        return invalid("too many clusters");
                    }
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
                if let Some((t, s)) = parse_track_entry(b, time_base)? {
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
            meta,
            chapters,
            scan_pos: pos,
        })
    }

    /// A file that is still being written: look for clusters appended since the last scan. True if there are new ones.
    async fn grow(&mut self) -> Result<bool> {
        let before = self.rd.size();
        let now = self.rd.refresh_size().await;
        let Some(end) = now else { return Ok(false) };
        if before.is_some_and(|b| end <= b) && self.scan_pos >= end {
            return Ok(false);
        }
        let mut hb = [0u8; 16];
        let mut added = false;
        while self.scan_pos < end {
            let n = self.rd.read_upto(self.scan_pos, &mut hb).await?;
            let Ok(h) = parse_hdr(&hb[..n]) else { break };
            let body = self.scan_pos + h.hlen;
            if h.id == ID_CLUSTER {
                if self.clusters.len() >= MAX_CLUSTERS {
                    return invalid("too many clusters");
                }
                let c = Self::read_cluster_hdr(&mut self.rd, self.scan_pos, h, end).await?;
                self.clusters.push(c);
                added = true;
                if h.size.is_none() {
                    // An unknown-size cluster runs to the end of what is there: look inside it next time.
                    self.scan_pos = body;
                    break;
                }
            }
            match h.size {
                Some(s) => self.scan_pos = body.saturating_add(s).max(self.scan_pos + 1),
                None => break,
            }
        }
        Ok(added)
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
        let data_start = pos.saturating_add(h.hlen);
        // Not clipped to the length we know now: the cluster may be cut off at the end of a file that is still growing.
        let end = h.size.map_or(seg_end, |s| data_start.saturating_add(s));
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
        let pts = self.time_base.ticks_to_us(ticks) - self.tracks[ti].delay_us;
        Packet {
            stream_id: self.tracks[ti].number as u32,
            pts,
            dts: pts, // Matroska stores no decode timestamps; decoders reorder from pts.
            duration: self.time_base.ticks_to_us(dur_ticks),
            keyframe,
            discard_end_us: 0,
            data,
        }
    }

    /// Decode a `BlockGroup` body: (block bytes, duration ticks, has reference).
    fn parse_group(&self, body: &[u8], cluster_ts: i64) -> Result<Vec<Packet>> {
        let (mut block, mut dur, mut has_ref, mut discard_ns) = (None, None, false, 0i64);
        for (id, b) in children(body)? {
            match id {
                ID_BLOCK => block = Some(b),
                ID_BLOCK_DURATION => dur = Some(uint(b) as i64),
                ID_REFERENCE_BLOCK => has_ref = true,
                ID_DISCARD_PADDING => {
                    // A signed big-endian integer of 1 to 8 bytes (nanoseconds).
                    discard_ns = b
                        .iter()
                        .fold(if b.first().is_some_and(|x| x & 0x80 != 0) { -1i64 } else { 0 }, |a, &x| {
                            (a << 8) | x as i64
                        });
                }
                _ => {}
            }
        }
        match block {
            Some(b) => {
                let mut pkts = self.parse_block(b, cluster_ts, Some(!has_ref), dur)?;
                if discard_ns > 0 {
                    if let Some(last) = pkts.last_mut() {
                        last.discard_end_us = discard_ns / 1000;
                    }
                }
                Ok(pkts)
            }
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
        let delay_us = self.tracks.iter().find(|t| t.number == number).map_or(0, |t| t.delay_us);
        let tb = self.time_base;
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
                            hits.push(Hit { pts_us: tb.ticks_to_us(ts + tc) - delay_us, offset: pos });
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
                                    hits.push(Hit {
                                        pts_us: tb.ticks_to_us(ts + tc) - delay_us,
                                        offset: pos,
                                    });
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            // Always forward (a size that wraps would loop for ever).
            pos = body.saturating_add(size).max(pos + 1);
        }
        Ok(hits)
    }

    /// Blocks of the tracks `numbers` in cluster `idx` (the caller filters by time).
    async fn scan_tracks(&mut self, idx: usize, numbers: &[u64], out: &mut Vec<Packet>) -> Result<()> {
        let c = self.clusters[idx];
        let mut pos = c.data_start;
        let mut ts = c.ts;
        let mut hb = [0u8; 48];
        while pos < c.end {
            let n = self.rd.read_upto(pos, &mut hb).await?;
            let Ok(h) = parse_hdr(&hb[..n]) else { break };
            let body = pos + h.hlen;
            let Some(size) = h.size else { break };
            let head = &hb[(h.hlen as usize).min(n)..n];
            match h.id {
                ID_CLUSTER_TIMESTAMP => {
                    let v = self.rd.read_vec(body, size.min(8)).await?;
                    ts = uint(&v) as i64;
                }
                ID_SIMPLE_BLOCK => {
                    if matches!(vint(head), Ok((Some(tn), _)) if numbers.contains(&tn)) {
                        let b = self.rd.read_vec(body, size).await?;
                        out.extend(self.parse_block(&b, ts, None, None)?);
                    }
                }
                ID_BLOCK_GROUP => {
                    // The block is normally the group's first child: look at its track number before reading the rest.
                    let mut at = 0usize;
                    let mut wanted = false;
                    while let Ok(g) = parse_hdr(head.get(at..).unwrap_or(&[])) {
                        let inner = at + g.hlen as usize;
                        if g.id == ID_BLOCK {
                            let tn = vint(head.get(inner..).unwrap_or(&[]));
                            wanted = matches!(tn, Ok((Some(tn), _)) if numbers.contains(&tn));
                            break;
                        }
                        match g.size {
                            Some(s) if (inner as u64).saturating_add(s) < head.len() as u64 => {
                                at = inner + s as usize
                            }
                            _ => break,
                        }
                    }
                    if wanted {
                        let g = self.rd.read_vec(body, size).await?;
                        out.extend(self.parse_group(&g, ts)?);
                    }
                }
                _ => {}
            }
            pos = body.saturating_add(size).max(pos + 1);
        }
        Ok(())
    }
}

impl<S: Source> Demuxer for MkvDemuxer<S> {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn duration_us(&self) -> Option<Timestamp> {
        self.duration_us
    }

    fn metadata(&self) -> &Metadata {
        &self.meta
    }

    fn chapters(&self) -> &[Chapter] {
        &self.chapters
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
                    // Out of clusters: a file that is still being written may have more by now.
                    if self.grow().await? {
                        continue;
                    }
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
                if self.clusters.len() >= MAX_CLUSTERS {
                    return invalid("too many clusters");
                }
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
            self.pos = body.saturating_add(size).max(self.pos + 1);
        }
    }

    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        self.pending.clear();
        if self.clusters.is_empty() || self.tracks.is_empty() {
            return Ok(0);
        }
        let number = self.tracks[self.primary()].number;
        let delay = self.tracks[self.primary()].delay_us;
        // Raw (undelayed) time of the target in ticks, for comparing with cluster timestamps.
        let target_ticks = self.time_base.us_to_ticks_floor(target_us + delay);
        let mut idx = self.clusters.partition_point(|c| c.ts <= target_ticks).saturating_sub(1);
        // Walk back until a cluster holds a keyframe at or before the target.
        let mut best: Option<(usize, Hit)> = None;
        loop {
            let hits = self.scan_keys(idx, number).await?;
            if let Some(h) = hits.into_iter().rev().find(|h| h.pts_us <= target_us) {
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
                self.scan_keys(j, number).await?.into_iter().rev().find(|h| h.pts_us <= target_us)
            {
                if best.as_ref().is_none_or(|(_, b)| h.pts_us > b.pts_us) {
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
        Ok(hit.pts_us)
    }

    async fn side_packets(
        &mut self,
        ids: &[u32],
        from_us: Timestamp,
        to_us: Timestamp,
    ) -> Result<Vec<Packet>> {
        let numbers: Vec<u64> = ids.iter().map(|&i| i as u64).collect();
        let mut out = Vec::new();
        if numbers.is_empty() || self.clusters.is_empty() || to_us < from_us {
            return Ok(out);
        }
        // Blocks sit in the cluster that starts at or before them (a little later is possible with negative offsets).
        let slack = self.time_base.us_to_ticks_floor(1_000_000);
        let from_ticks = self.time_base.us_to_ticks_floor(from_us.max(0));
        let to_ticks = self.time_base.us_to_ticks_floor(to_us);
        let mut i = self.clusters.partition_point(|c| c.ts <= from_ticks).saturating_sub(1);
        while i < self.clusters.len() && self.clusters[i].ts <= to_ticks + slack {
            self.scan_tracks(i, &numbers, &mut out).await?;
            i += 1;
        }
        out.retain(|p| p.pts >= from_us && p.pts <= to_us);
        out.sort_by_key(|p| p.pts);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// An EBML element with a one-byte size (bodies up to 126 bytes).
    fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.push(0x80 | body.len() as u8);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn nested_simple_tags_are_read_and_do_not_recurse_for_ever() {
        // Tag { SimpleTag { TagName "ARTIST", TagString "Nested Artist", SimpleTag { TagName "TITLE", TagString "T" } } }
        let inner = el(&[0x67, 0xC8], &[el(&[0x45, 0xA3], b"TITLE"), el(&[0x44, 0x87], b"T")].concat());
        let outer = el(
            &[0x67, 0xC8],
            &[el(&[0x45, 0xA3], b"ARTIST"), el(&[0x44, 0x87], b"Nested Artist"), inner].concat(),
        );
        let tag = el(&[0x73, 0x73], &outer);
        let mut meta = Metadata::default();
        read_simple_tags(&tag, &mut meta, 0);
        assert_eq!(meta.artist.as_deref(), Some("Nested Artist"));
        assert_eq!(meta.title.as_deref(), Some("T"));
        // A hostile file nests tags as deep as its size allows: the depth limit stops it.
        let mut body = el(&[0x67, 0xC8], &[el(&[0x45, 0xA3], b"TITLE"), el(&[0x44, 0x87], b"x")].concat());
        for _ in 0..30 {
            if body.len() > 120 {
                break;
            }
            body = el(&[0x67, 0xC8], &body);
        }
        let mut meta = Metadata::default();
        read_simple_tags(&el(&[0x73, 0x73], &body), &mut meta, 0);
        let _ = vec![0u8; 0];
    }

    #[test]
    fn replaygain_simple_tags() {
        let simple = |name: &str, value: &str| {
            el(
                &[0x67, 0xC8],
                &[el(&[0x45, 0xA3], name.as_bytes()), el(&[0x44, 0x87], value.as_bytes())].concat(),
            )
        };
        let tag = el(
            &[0x73, 0x73],
            &[
                simple("replaygain_track_gain", "-4.20 dB"),
                simple("REPLAYGAIN_ALBUM_GAIN", "-5.00 dB"),
                simple("REPLAYGAIN_TRACK_PEAK", "0.75"),
                simple("TITLE", "T"),
            ]
            .concat(),
        );
        let mut meta = Metadata::default();
        read_simple_tags(&tag, &mut meta, 0);
        assert_eq!(meta.loudness.track_lufs, Some(-13.8));
        assert_eq!(meta.loudness.album_lufs, Some(-13.0));
        assert_eq!(meta.loudness.track_peak, Some(0.75));
        assert_eq!(meta.title.as_deref(), Some("T"));
    }

    #[test]
    fn chapter_nesting_is_bounded() {
        let mut atom = el(&[0xB6], &el(&[0x91], &[0x05]));
        for _ in 0..50 {
            if atom.len() > 120 {
                break;
            }
            atom = el(&[0xB6], &atom);
        }
        let edition = el(&[0x45, 0xB9], &atom);
        let mut chapters = Vec::new();
        read_chapters(&edition, &mut chapters);
        assert!(chapters.len() <= MAX_CHAPTERS);
    }
}
