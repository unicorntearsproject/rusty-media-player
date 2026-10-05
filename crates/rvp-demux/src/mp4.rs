//! ISO base media file format (MP4/M4A/M4V) demuxer, including fragmented files.
//!
//! Open reads the whole `moov` plus every `moof` header into per-track sample tables (offset, size, dts,
//! composition offset, sync flag). That costs about 25 bytes per sample, which is fine for the files a
//! player opens, and makes seeking and interleaving trivial. Packet payloads are read lazily.
//!
//! Timestamps: the first non-empty edit-list entry shifts a track (`pts = dts + cts - media_time +
//! empty_edit`), which is what players do to hide encoder delay. Other edit-list shapes are ignored.
use crate::Demuxer;
use crate::io::{Cur, Reader, invalid};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_core::{
    Art, AudioInfo, Chapter, Error, Metadata, Packet, Rational, Result, StreamInfo, StreamKind, Timestamp,
    VideoInfo,
};
use rvp_host::Source;

/// Most samples a single track may have (four million is about a day of video or audio): bounds what a hostile file
/// can make us allocate.
const MAX_SAMPLES: usize = 1 << 22;

type Fourcc = [u8; 4];

#[derive(Clone, Copy)]
struct Sample {
    offset: u64,
    size: u32,
    dts: i64,
    cts: i32,
    dur: u32,
    key: bool,
}

struct Track {
    id: u32,
    time_base: Rational,
    /// Added to dts and pts (in media ticks): `-media_time + empty_edit`.
    shift: i64,
    samples: Vec<Sample>,
    cursor: usize,
    /// Indices into `samples` of sync samples, in order.
    keys: Vec<usize>,
}

impl Track {
    fn pts_ticks(&self, i: usize) -> i64 {
        let s = &self.samples[i];
        s.dts + s.cts as i64 + self.shift
    }

    fn pts_us(&self, i: usize) -> Timestamp {
        self.time_base.ticks_to_us(self.pts_ticks(i))
    }

    fn rebuild_keys(&mut self) {
        self.keys = self.samples.iter().enumerate().filter(|(_, s)| s.key).map(|(i, _)| i).collect();
    }
}

/// Defaults from `trex`, per track id.
#[derive(Clone, Copy, Default)]
struct Trex {
    duration: u32,
    size: u32,
    flags: u32,
}

/// An MP4 demuxer over a host [`Source`].
pub struct Mp4Demuxer<S: Source> {
    rd: Reader<S>,
    streams: Vec<StreamInfo>,
    tracks: Vec<Track>,
    duration_us: Option<Timestamp>,
    meta: Metadata,
    chapters: Vec<Chapter>,
    /// Fragment defaults (`trex`) and where the scan of top-level boxes stopped, so a file that grows can be read on.
    trex: Vec<(u32, Trex)>,
    scan_pos: u64,
}

/// Tags from `udta/meta/ilst` (iTunes style) and chapters from the Nero `udta/chpl` box.
fn parse_udta(udta: &[u8], meta: &mut Metadata, chapters: &mut Vec<Chapter>) {
    let Ok(kids) = boxes(udta) else { return };
    if let Some(m) = find(&kids, b"meta") {
        // `meta` is a full box: skip version and flags.
        if let Some(body) = m.get(4..) {
            if let Ok(mk) = boxes(body) {
                if let Some(ilst) = find(&mk, b"ilst") {
                    for (ty, item) in boxes(ilst).unwrap_or_default() {
                        let Some(data) = boxes(item).ok().and_then(|b| find(&b, b"data")) else { continue };
                        // version/flags (type in the low 24 bits), 4 reserved bytes, then the value.
                        let (Some(flags), Some(value)) = (data.get(0..4), data.get(8..)) else { continue };
                        let kind = u32::from_be_bytes([flags[0], flags[1], flags[2], flags[3]]) & 0xFF_FFFF;
                        let text = || {
                            core::str::from_utf8(value)
                                .ok()
                                .map(ToString::to_string)
                                .filter(|s| !s.is_empty())
                        };
                        match &ty {
                            [0xA9, b'n', b'a', b'm'] => meta.title = text(),
                            [0xA9, b'A', b'R', b'T'] => meta.artist = text().or(meta.artist.take()),
                            b"aART" => {
                                meta.album_artist = text();
                                meta.artist = meta.artist.take().or_else(text);
                            }
                            [0xA9, b'a', b'l', b'b'] => meta.album = text(),
                            [0xA9, b'g', b'e', b'n'] => meta.genre = text(),
                            [0xA9, b'd', b'a', b'y'] => {
                                meta.year = text().and_then(|d| crate::tags::year_of(&d))
                            }
                            // Track and disc numbers: two reserved bytes, the number, the total (big-endian u16 each).
                            b"trkn" | b"disk" if value.len() >= 6 => {
                                let n = u16::from_be_bytes([value[2], value[3]]) as u32;
                                let total = u16::from_be_bytes([value[4], value[5]]) as u32;
                                let (num, tot) = if &ty == b"trkn" {
                                    (&mut meta.track, &mut meta.track_total)
                                } else {
                                    (&mut meta.disc, &mut meta.disc_total)
                                };
                                *num = (n > 0).then_some(n);
                                *tot = (total > 0).then_some(total);
                            }
                            b"covr" if !value.is_empty() => {
                                let mime = if kind == 14 { "image/png" } else { "image/jpeg" };
                                meta.art = Some(Art { mime: mime.to_string(), data: value.to_vec() });
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    if let Some(c) = find(&kids, b"chpl") {
        // version, flags, 4 reserved bytes, a count byte, then (start in 100 ns, title length, title) entries.
        let mut cur = Cur::new(c);
        let mut go = |cur: &mut Cur| -> Result<()> {
            cur.skip(8)?;
            let n = cur.u8()?;
            for _ in 0..n {
                let start = cur.u64()? as i64 / 10;
                let len = cur.u8()? as usize;
                let title = String::from_utf8_lossy(cur.take(len)?).into_owned();
                chapters.push(Chapter { start_us: start, title });
            }
            Ok(())
        };
        let _ = go(&mut cur);
        chapters.sort_by_key(|c| c.start_us);
    }
}

fn boxes(d: &[u8]) -> Result<Vec<(Fourcc, &[u8])>> {
    let mut out = Vec::new();
    let mut c = Cur::new(d);
    while c.remaining() >= 8 {
        let size = c.u32()? as u64;
        let ty: Fourcc = c.take(4)?.try_into().unwrap();
        let (hdr, size) = match size {
            0 => (8, 8 + c.remaining() as u64),
            1 => (16, c.u64()?),
            n => (8, n),
        };
        if size < hdr {
            return invalid("box size smaller than its header");
        }
        let body = c.take((size - hdr) as usize)?;
        out.push((ty, body));
    }
    Ok(out)
}

fn find<'a>(list: &[(Fourcc, &'a [u8])], ty: &[u8; 4]) -> Option<&'a [u8]> {
    list.iter().find(|(t, _)| t == ty).map(|(_, b)| *b)
}

fn full_box<'a>(body: &'a [u8]) -> Result<(u8, u32, Cur<'a>)> {
    let mut c = Cur::new(body);
    let v = c.u32()?;
    Ok(((v >> 24) as u8, v & 0xFF_FFFF, c))
}

struct TrackBuild {
    info: StreamInfo,
    track: Track,
}

fn lang(v: u16) -> Option<String> {
    let c = [((v >> 10) & 31) as u8 + 0x60, ((v >> 5) & 31) as u8 + 0x60, (v & 31) as u8 + 0x60];
    if c.iter().all(u8::is_ascii_lowercase) && c != *b"und" {
        core::str::from_utf8(&c).ok().map(ToString::to_string)
    } else {
        None
    }
}

/// MPEG-4 descriptor length: 1 to 4 bytes, 7 bits each.
fn descr_len(c: &mut Cur) -> Result<usize> {
    let mut n = 0usize;
    for _ in 0..4 {
        let b = c.u8()?;
        n = (n << 7) | (b & 0x7F) as usize;
        if b & 0x80 == 0 {
            break;
        }
    }
    Ok(n)
}

/// Parse an `esds` box body: returns (object type indication, DecoderSpecificInfo bytes).
fn parse_esds(body: &[u8]) -> Result<(u8, Vec<u8>)> {
    let (_, _, mut c) = full_box(body)?;
    let (mut oti, mut dsi) = (0u8, Vec::new());
    // Flat walk: ES_Descr (3) and DecoderConfigDescr (4) contain further descriptors; DecSpecificInfo (5) is a leaf.
    while c.remaining() >= 2 {
        let tag = c.u8()?;
        let len = descr_len(&mut c)?;
        match tag {
            0x03 => {
                c.skip(2)?;
                let flags = c.u8()?;
                if flags & 0x80 != 0 {
                    c.skip(2)?;
                }
                if flags & 0x40 != 0 {
                    let n = c.u8()? as usize;
                    c.skip(n)?;
                }
                if flags & 0x20 != 0 {
                    c.skip(2)?;
                }
            }
            0x04 => {
                oti = c.u8()?;
                c.skip(12)?;
            }
            0x05 => {
                dsi = c.take(len.min(c.remaining()))?.to_vec();
                break;
            }
            _ => c.skip(len.min(c.remaining()))?,
        }
    }
    Ok((oti, dsi))
}

fn parse_trak(trak: &[u8], movie_ts: u32) -> Result<Option<TrackBuild>> {
    let kids = boxes(trak)?;
    let Some(tkhd) = find(&kids, b"tkhd") else { return invalid("trak without tkhd") };
    let (v, _, mut c) = full_box(tkhd)?;
    c.skip(if v == 1 { 16 } else { 8 })?;
    let id = c.u32()?;
    let mdia = boxes(find(&kids, b"mdia").ok_or(Error::Invalid("trak without mdia".to_string()))?)?;
    let Some(mdhd) = find(&mdia, b"mdhd") else { return invalid("mdia without mdhd") };
    let (v, _, mut c) = full_box(mdhd)?;
    c.skip(if v == 1 { 16 } else { 8 })?;
    let timescale = c.u32()?;
    let mdhd_dur = if v == 1 { c.u64()? } else { c.u32()? as u64 };
    let language = lang(c.u16()?);
    if timescale == 0 {
        return invalid("zero media timescale");
    }
    let Some(hdlr) = find(&mdia, b"hdlr") else { return invalid("mdia without hdlr") };
    let (_, _, mut c) = full_box(hdlr)?;
    c.skip(4)?;
    let handler: Fourcc = c.take(4)?.try_into().unwrap();
    let kind = match &handler {
        b"vide" => StreamKind::Video,
        b"soun" => StreamKind::Audio,
        b"subt" | b"text" | b"sbtl" | b"clcp" => StreamKind::Subtitle,
        _ => return Ok(None),
    };
    let minf = boxes(find(&mdia, b"minf").ok_or(Error::Invalid("mdia without minf".to_string()))?)?;
    let stbl = boxes(find(&minf, b"stbl").ok_or(Error::Invalid("minf without stbl".to_string()))?)?;

    // Sample entry.
    let stsd = find(&stbl, b"stsd").ok_or(Error::Invalid("stbl without stsd".to_string()))?;
    let (_, _, mut c) = full_box(stsd)?;
    if c.u32()? == 0 {
        return Ok(None);
    }
    let entry_size = c.u32()? as usize;
    let format: Fourcc = c.take(4)?.try_into().unwrap();
    let mut e = Cur::new(c.take(entry_size.saturating_sub(8).min(c.remaining()))?);
    e.skip(8)?; // reserved + data_reference_index
    let (mut video, mut audio) = (None, None);
    match kind {
        StreamKind::Video => {
            e.skip(16)?;
            let (w, h) = (e.u16()? as u32, e.u16()? as u32);
            e.skip(50)?;
            video = Some(VideoInfo { width: w, height: h });
        }
        StreamKind::Audio => {
            let ver = e.u16()?;
            e.skip(6)?;
            let ch = e.u16()?;
            e.skip(6)?;
            let rate = e.u32()? >> 16;
            match ver {
                1 => e.skip(16)?,
                2 => e.skip(36)?,
                _ => {}
            }
            audio = Some(AudioInfo { sample_rate: rate, channels: ch });
        }
        StreamKind::Subtitle => {}
    }
    let cfg = boxes(e.rest()).unwrap_or_default();
    let mut codec = match &format {
        b"avc1" | b"avc3" => "h264".to_string(),
        b"hvc1" | b"hev1" => "hevc".to_string(),
        b"av01" => "av1".to_string(),
        b"vp09" => "vp9".to_string(),
        b"vp08" => "vp8".to_string(),
        b"mp4v" => "mpeg4".to_string(),
        b"Opus" => "opus".to_string(),
        b"fLaC" => "flac".to_string(),
        b"ac-3" => "ac3".to_string(),
        b"ec-3" => "eac3".to_string(),
        b"alac" => "alac".to_string(),
        b"tx3g" => "mov_text".to_string(),
        b"wvtt" => "webvtt".to_string(),
        b"mp4a" => "mp4a".to_string(),
        other => String::from_utf8_lossy(other).to_lowercase(),
    };
    let mut extra_data = Vec::new();
    for key in [b"avcC", b"hvcC", b"av1C", b"vpcC", b"dOps", b"dfLa"] {
        if let Some(b) = find(&cfg, key) {
            extra_data = b.to_vec();
            break;
        }
    }
    if let Some(esds) = find(&cfg, b"esds") {
        let (oti, dsi) = parse_esds(esds)?;
        extra_data = dsi;
        codec = match (&format, oti) {
            (b"mp4a", 0x40 | 0x66..=0x68) => "aac".to_string(),
            (b"mp4a", 0x69 | 0x6B) => "mp3".to_string(),
            (b"mp4a", o) => alloc::format!("mp4a-{o:02x}"),
            (_, _) => codec,
        };
    }

    // Sample tables.
    let tb = Rational::new(1, timescale);
    let mut samples: Vec<Sample> = Vec::new();
    if let Some(stsz) = find(&stbl, b"stsz") {
        let (_, _, mut c) = full_box(stsz)?;
        let fixed = c.u32()?;
        let n = c.u32()? as usize;
        if fixed == 0 && n > c.remaining() / 4 {
            return Err(Error::Truncated);
        }
        if n > MAX_SAMPLES {
            return invalid("too many samples in a track");
        }
        let stco = find(&stbl, b"stco");
        let co64 = find(&stbl, b"co64");
        let (offsets64, is64) = match (co64, stco) {
            (Some(b), _) => (b, true),
            (None, Some(b)) => (b, false),
            _ => return invalid("stbl without chunk offsets"),
        };
        let (_, _, mut oc) = full_box(offsets64)?;
        let chunk_count = oc.u32()? as usize;
        if chunk_count > oc.remaining() / if is64 { 8 } else { 4 } {
            return Err(Error::Truncated);
        }
        let mut chunks = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            chunks.push(if is64 { oc.u64()? } else { oc.u32()? as u64 });
        }
        let stsc = find(&stbl, b"stsc").ok_or(Error::Invalid("stbl without stsc".to_string()))?;
        let (_, _, mut sc) = full_box(stsc)?;
        let runs = sc.u32()? as usize;
        if runs > sc.remaining() / 12 {
            return Err(Error::Truncated);
        }
        let mut stsc_runs = Vec::with_capacity(runs);
        for _ in 0..runs {
            let first = sc.u32()?;
            let per = sc.u32()?;
            sc.skip(4)?;
            stsc_runs.push((first as usize, per as usize));
        }
        // Sizes (a fixed size needs no table, whatever the sample count claims).
        let mut sizes = Vec::with_capacity(if fixed != 0 { 0 } else { n });
        if fixed == 0 {
            for _ in 0..n {
                sizes.push(c.u32()?);
            }
        }
        let size_of = |i: usize| if fixed != 0 { fixed } else { sizes[i] };
        // Offsets: walk chunks.
        let mut idx = 0usize;
        'outer: for (ci, &chunk_off) in chunks.iter().enumerate() {
            let chunk_no = ci + 1;
            let per = match stsc_runs.iter().rposition(|&(first, _)| first <= chunk_no) {
                Some(r) => stsc_runs[r].1,
                None => return invalid("stsc does not cover chunk 1"),
            };
            let mut off = chunk_off;
            for _ in 0..per {
                if idx >= n {
                    break 'outer;
                }
                samples.push(Sample { offset: off, size: size_of(idx), dts: 0, cts: 0, dur: 0, key: true });
                off += size_of(idx) as u64;
                idx += 1;
            }
        }
        if samples.len() != n {
            return invalid("stsz and stsc/stco disagree on sample count");
        }
        // Times.
        if let Some(stts) = find(&stbl, b"stts") {
            let (_, _, mut c) = full_box(stts)?;
            let entries = c.u32()? as usize;
            if entries > c.remaining() / 8 {
                return Err(Error::Truncated);
            }
            let (mut i, mut t) = (0usize, 0i64);
            for _ in 0..entries {
                let count = c.u32()? as usize;
                let delta = c.u32()?;
                for _ in 0..count.min(n - i) {
                    samples[i].dts = t;
                    samples[i].dur = delta;
                    t += delta as i64;
                    i += 1;
                }
            }
            while i < n {
                samples[i].dts = t;
                i += 1;
            }
        }
        if let Some(ctts) = find(&stbl, b"ctts") {
            let (_, _, mut c) = full_box(ctts)?;
            let entries = c.u32()? as usize;
            if entries > c.remaining() / 8 {
                return Err(Error::Truncated);
            }
            let mut i = 0usize;
            for _ in 0..entries {
                let count = c.u32()? as usize;
                let off = c.i32()?;
                for _ in 0..count.min(n - i) {
                    samples[i].cts = off;
                    i += 1;
                }
            }
        }
        if let Some(stss) = find(&stbl, b"stss") {
            let (_, _, mut c) = full_box(stss)?;
            let entries = c.u32()? as usize;
            if entries > c.remaining() / 4 {
                return Err(Error::Truncated);
            }
            for s in samples.iter_mut() {
                s.key = false;
            }
            for _ in 0..entries {
                let k = c.u32()? as usize;
                if k >= 1 && k <= n {
                    samples[k - 1].key = true;
                }
            }
        }
    }

    // Edit list.
    let mut shift = 0i64;
    // Playable length in media ticks according to the edit list (leading empty edit plus the first real edit).
    let mut edit_ticks: Option<i64> = None;
    if let Some(edts) = find(&kids, b"edts") {
        if let Some(elst) = find(&boxes(edts)?, b"elst") {
            let (v, _, mut c) = full_box(elst)?;
            let n = c.u32()? as usize;
            let mut empty = 0i64;
            for _ in 0..n {
                let (seg, media) = if v == 1 {
                    (c.u64()? as i64, c.u64()? as i64)
                } else {
                    (c.u32()? as i64, c.i32()? as i64)
                };
                c.skip(4)?;
                if media == -1 {
                    if movie_ts != 0 {
                        empty += seg * timescale as i64 / movie_ts as i64;
                    }
                } else {
                    shift = -media + empty;
                    if movie_ts != 0 && seg > 0 {
                        edit_ticks = Some(empty + seg * timescale as i64 / movie_ts as i64);
                    }
                    break;
                }
            }
            if shift == 0 {
                shift = empty;
            }
        }
    }

    let mut track = Track { id, time_base: tb, shift, samples, cursor: 0, keys: Vec::new() };
    track.rebuild_keys();
    let dur_ticks = match edit_ticks {
        // The edit list says how much of the media plays, which excludes encoder delay and end padding.
        Some(e) => e,
        None if mdhd_dur != 0 => mdhd_dur as i64,
        None => track.samples.iter().map(|s| s.dur as i64).sum(),
    };
    let info = StreamInfo {
        id,
        kind,
        codec,
        time_base: tb,
        language,
        extra_data,
        video,
        audio,
        duration_us: (dur_ticks > 0).then(|| tb.ticks_to_us(dur_ticks)),
    };
    Ok(Some(TrackBuild { info, track }))
}

/// Append the samples of one `moof` to the tracks.
fn parse_moof(moof: &[u8], moof_start: u64, tracks: &mut [Track], trex: &[(u32, Trex)]) -> Result<()> {
    for (ty, traf) in boxes(moof)? {
        if &ty != b"traf" {
            continue;
        }
        let kids = boxes(traf)?;
        let tfhd = find(&kids, b"tfhd").ok_or(Error::Invalid("traf without tfhd".to_string()))?;
        let (_, flags, mut c) = full_box(tfhd)?;
        let track_id = c.u32()?;
        let Some(track) = tracks.iter_mut().find(|t| t.id == track_id) else { continue };
        let defaults = trex.iter().find(|(id, _)| *id == track_id).map(|(_, t)| *t).unwrap_or_default();
        let base = if flags & 0x1 != 0 { c.u64()? } else { moof_start };
        if flags & 0x2 != 0 {
            c.skip(4)?;
        }
        let d_dur = if flags & 0x8 != 0 { c.u32()? } else { defaults.duration };
        let d_size = if flags & 0x10 != 0 { c.u32()? } else { defaults.size };
        let d_flags = if flags & 0x20 != 0 { c.u32()? } else { defaults.flags };

        let mut next_dts = match find(&kids, b"tfdt") {
            Some(b) => {
                let (v, _, mut c) = full_box(b)?;
                if v == 1 { c.u64()? as i64 } else { c.u32()? as i64 }
            }
            None => track.samples.last().map_or(0, |s| s.dts + s.dur as i64),
        };
        let mut data_pos = base;
        for (ty, trun) in &kids {
            if ty != b"trun" {
                continue;
            }
            let (_, f, mut c) = full_box(trun)?;
            let count = c.u32()? as usize;
            if f & 0x1 != 0 {
                data_pos = base.wrapping_add(c.i32()? as i64 as u64);
            }
            let first_flags = if f & 0x4 != 0 { Some(c.u32()?) } else { None };
            let per = 4 * [0x100, 0x200, 0x400, 0x800].iter().filter(|m| f & **m != 0).count();
            if per != 0 && count > c.remaining() / per {
                return Err(Error::Truncated);
            }
            if count > MAX_SAMPLES || track.samples.len() + count > MAX_SAMPLES {
                return invalid("too many samples in a track");
            }
            track.samples.reserve(count.min(1 << 20));
            for i in 0..count {
                let dur = if f & 0x100 != 0 { c.u32()? } else { d_dur };
                let size = if f & 0x200 != 0 { c.u32()? } else { d_size };
                let sflags = if f & 0x400 != 0 {
                    c.u32()?
                } else if i == 0 {
                    first_flags.unwrap_or(d_flags)
                } else {
                    d_flags
                };
                let cts = if f & 0x800 != 0 { c.i32()? } else { 0 };
                track.samples.push(Sample {
                    offset: data_pos,
                    size,
                    dts: next_dts,
                    cts,
                    dur,
                    key: sflags & 0x1_0000 == 0,
                });
                data_pos = data_pos.saturating_add(size as u64);
                next_dts = next_dts.saturating_add(dur as i64);
            }
        }
    }
    Ok(())
}

impl<S: Source> Mp4Demuxer<S> {
    /// Parse the container headers and sample tables.
    pub async fn open(src: S) -> Result<Self> {
        let mut rd = Reader::new(src).await;
        let size = rd.size();
        let mut pos = 0u64;
        let mut movie_ts = 0u32;
        let mut movie_dur = 0u64;
        let mut streams = Vec::new();
        let mut tracks: Vec<Track> = Vec::new();
        let mut trex: Vec<(u32, Trex)> = Vec::new();
        let mut have_moov = false;
        let mut meta = Metadata::default();
        let mut chapters: Vec<Chapter> = Vec::new();
        let mut have_ftyp = false;
        loop {
            let mut hdr = [0u8; 16];
            let n = rd.read_upto(pos, &mut hdr).await?;
            if n < 8 {
                break;
            }
            let mut size32 = u32::from_be_bytes(hdr[..4].try_into().unwrap()) as u64;
            let ty: Fourcc = hdr[4..8].try_into().unwrap();
            let mut hlen = 8u64;
            if size32 == 1 {
                if n < 16 {
                    return Err(Error::Truncated);
                }
                size32 = u64::from_be_bytes(hdr[8..16].try_into().unwrap());
                hlen = 16;
            } else if size32 == 0 {
                size32 = size.ok_or(Error::Unsupported(
                    "box to end of file on a source of unknown length".to_string(),
                ))? - pos;
            }
            if size32 < hlen {
                return invalid("box size smaller than its header");
            }
            let end = pos.checked_add(size32).ok_or(Error::Invalid("box size overflow".to_string()))?;
            match &ty {
                b"ftyp" => have_ftyp = true,
                b"moov" => {
                    let body = rd.read_vec(pos + hlen, size32 - hlen).await?;
                    let kids = boxes(&body)?;
                    if let Some(mvhd) = find(&kids, b"mvhd") {
                        let (v, _, mut c) = full_box(mvhd)?;
                        c.skip(if v == 1 { 16 } else { 8 })?;
                        movie_ts = c.u32()?;
                        movie_dur = if v == 1 { c.u64()? } else { c.u32()? as u64 };
                    }
                    for (t, b) in &kids {
                        if t == b"trak" {
                            if let Some(tb) = parse_trak(b, movie_ts)? {
                                streams.push(tb.info);
                                tracks.push(tb.track);
                            }
                        }
                    }
                    if let Some(udta) = find(&kids, b"udta") {
                        parse_udta(udta, &mut meta, &mut chapters);
                    }
                    if let Some(mvex) = find(&kids, b"mvex") {
                        for (t, b) in boxes(mvex)? {
                            if &t == b"trex" {
                                let (_, _, mut c) = full_box(b)?;
                                let id = c.u32()?;
                                c.skip(4)?;
                                trex.push((id, Trex { duration: c.u32()?, size: c.u32()?, flags: c.u32()? }));
                            }
                        }
                    }
                    have_moov = true;
                }
                b"moof" => {
                    if !have_moov {
                        return invalid("moof before moov");
                    }
                    // A fragment cut off at the end of the file is the end of what there is, not an error.
                    let body = match rd.read_vec(pos + hlen, size32 - hlen).await {
                        Ok(b) => b,
                        Err(Error::Truncated) => break,
                        Err(e) => return Err(e),
                    };
                    parse_moof(&body, pos, &mut tracks, &trex)?;
                }
                _ => {}
            }
            pos = end;
            if size.is_some_and(|s| pos >= s) {
                break;
            }
        }
        if !have_ftyp && !have_moov {
            return Err(Error::Unsupported("not an MP4 file".to_string()));
        }
        if !have_moov {
            return invalid("no moov box");
        }
        for t in &mut tracks {
            t.rebuild_keys();
        }
        let duration_us = if movie_dur != 0 && movie_ts != 0 {
            Some(Rational::new(1, movie_ts).ticks_to_us(movie_dur as i64))
        } else {
            tracks
                .iter()
                .filter_map(|t| {
                    let last = t.samples.len().checked_sub(1)?;
                    let s = &t.samples[last];
                    Some(t.time_base.ticks_to_us(s.dts + s.dur as i64 + t.shift))
                })
                .max()
        };
        // Fill durations for fragmented tracks that had none in mdhd.
        for (info, t) in streams.iter_mut().zip(&tracks) {
            if info.duration_us.is_none() {
                let ticks: i64 = t.samples.iter().map(|s| s.dur as i64).sum();
                info.duration_us = (ticks > 0).then(|| t.time_base.ticks_to_us(ticks));
            }
        }
        Ok(Self { rd, streams, tracks, duration_us, meta, chapters, trex, scan_pos: pos })
    }

    /// A file that is still being written: look for fragments appended since the last scan. True if there are new
    /// samples.
    async fn grow(&mut self) -> Result<bool> {
        let before = self.rd.size();
        let now = self.rd.refresh_size().await;
        if now.is_none() || now <= before && before.is_some_and(|b| self.scan_pos >= b) {
            return Ok(false);
        }
        let had: usize = self.tracks.iter().map(|t| t.samples.len()).sum();
        loop {
            let mut hdr = [0u8; 16];
            let n = self.rd.read_upto(self.scan_pos, &mut hdr).await?;
            if n < 8 {
                break;
            }
            let mut size32 = u32::from_be_bytes(hdr[..4].try_into().unwrap()) as u64;
            let ty: Fourcc = hdr[4..8].try_into().unwrap();
            let mut hlen = 8u64;
            if size32 == 1 && n >= 16 {
                size32 = u64::from_be_bytes(hdr[8..16].try_into().unwrap());
                hlen = 16;
            }
            if size32 < hlen {
                break;
            }
            if &ty == b"moof" {
                match self.rd.read_vec(self.scan_pos + hlen, size32 - hlen).await {
                    Ok(body) => {
                        let at = self.scan_pos;
                        parse_moof(&body, at, &mut self.tracks, &self.trex)?;
                    }
                    Err(Error::Truncated) => break, // not all there yet; look again later
                    Err(e) => return Err(e),
                }
            }
            self.scan_pos = self.scan_pos.saturating_add(size32);
        }
        for t in &mut self.tracks {
            t.rebuild_keys();
        }
        Ok(self.tracks.iter().map(|t| t.samples.len()).sum::<usize>() > had)
    }

    /// Index of the track seeks are keyed on: the first video track, else the first track.
    fn primary(&self) -> usize {
        self.streams.iter().position(|s| s.kind == StreamKind::Video).unwrap_or(0)
    }
}

impl<S: Source> Demuxer for Mp4Demuxer<S> {
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
        loop {
            // Interleave in file order: the track whose next sample sits earliest in the file goes first.
            let Some((ti, _)) = self
                .tracks
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.samples.get(t.cursor).map(|s| (i, s.offset)))
                .min_by_key(|&(_, off)| off)
            else {
                // Out of samples: a file that is still being written may have more by now.
                if self.grow().await? {
                    continue;
                }
                return Ok(None);
            };
            let t = &self.tracks[ti];
            let i = t.cursor;
            let s = t.samples[i];
            let data = self.rd.read_vec(s.offset, s.size as u64).await?;
            let t = &mut self.tracks[ti];
            let pkt = Packet {
                stream_id: t.id,
                pts: t.time_base.ticks_to_us(s.dts + s.cts as i64 + t.shift),
                dts: t.time_base.ticks_to_us(s.dts + t.shift),
                duration: t.time_base.ticks_to_us(s.dur as i64),
                keyframe: s.key,
                discard_end_us: 0,
                data,
            };
            t.cursor += 1;
            return Ok(Some(pkt));
        }
    }

    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        if self.tracks.is_empty() {
            return Ok(0);
        }
        let p = self.primary();
        let pt = &self.tracks[p];
        if pt.keys.is_empty() {
            return Ok(0);
        }
        let n = pt.keys.partition_point(|&k| pt.pts_us(k) <= target_us);
        let key = pt.keys[n.saturating_sub(1)];
        let landed = pt.pts_us(key);
        for (i, t) in self.tracks.iter_mut().enumerate() {
            if i == p {
                t.cursor = key;
            } else if t.keys.is_empty() {
                t.cursor = 0;
            } else {
                let m = t.keys.partition_point(|&k| t.pts_us(k) <= landed);
                t.cursor = t.keys[m.saturating_sub(1)];
            }
        }
        Ok(landed)
    }
}
