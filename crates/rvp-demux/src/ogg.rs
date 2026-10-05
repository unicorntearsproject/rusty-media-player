//! Ogg: Vorbis, Opus and FLAC-in-Ogg (`.ogg`, `.oga`, `.opus`). The first supported logical stream is played; other streams
//! multiplexed with it are skipped, and a chained stream (a new beginning after the end) ends the file.
//!
//! Timestamps come from the page granule positions. Vorbis packets have no length of their own, so their durations come from
//! the block sizes of the stream (the setup header's mode table says which packets are long and which are short); Opus
//! packets say theirs in the TOC byte; FLAC frames carry their sample numbers.
use crate::Demuxer;
use crate::flac;
use crate::io::{Reader, Win};
use crate::tags;
use alloc::collections::VecDeque;
use alloc::string::ToString;
use alloc::vec::Vec;
use rvp_core::{
    AudioInfo, Chapter, Error, Metadata, Packet, Rational, Result, StreamInfo, StreamKind, Timestamp,
};
use rvp_host::Source;

/// Opus decoders need this much audio before the point to land on to be in tune (80 ms at 48 kHz).
const OPUS_PREROLL: i64 = 3840;

struct Page {
    off: u64,
    htype: u8,
    granule: i64,
    serial: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
    next: u64,
}

enum Codec {
    Vorbis { blocks: [u32; 2], modes: Vec<bool>, mode_bits: u32, prev: Option<u32> },
    Opus { pre_skip: i64 },
    Flac { fixed: Option<u32> },
}

/// An Ogg file.
pub struct OggDemuxer<S: Source> {
    rd: Reader<S>,
    win: Win,
    streams: Vec<StreamInfo>,
    meta: Metadata,
    duration: Option<Timestamp>,
    codec: Codec,
    serial: u32,
    rate: i64,
    /// First page after the header pages.
    first_audio: u64,
    /// Next page to read.
    pos: u64,
    /// Bytes of a packet that continues on the next page.
    carry: Vec<u8>,
    /// Drop the packet that is already under way on the first page read (after a seek).
    skip_continued: bool,
    /// Sample count at the end of the last page read.
    cursor: Option<i64>,
    /// The end of the playable audio in samples (the last granule position), if known.
    limit: Option<i64>,
    queue: VecDeque<Packet>,
    done: bool,
}

fn ilog(mut v: u32) -> u32 {
    let mut n = 0;
    while v > 0 {
        n += 1;
        v >>= 1;
    }
    n
}

fn bit_at(b: &[u8], i: usize) -> u32 {
    ((b.get(i / 8).copied().unwrap_or(0) >> (i % 8)) & 1) as u32
}

fn bits_at(b: &[u8], pos: usize, n: usize) -> u32 {
    (0..n).fold(0, |a, k| a | bit_at(b, pos + k) << k)
}

/// The block flag (long or short) of every mode in a Vorbis setup header. The mode table is the last thing in the header, so
/// it is found from the end: 41 bits per mode, with the two 16-bit type fields zero, and a 6-bit count before the first.
fn vorbis_modes(setup: &[u8]) -> Option<Vec<bool>> {
    let last = setup.iter().rposition(|&b| b != 0)?;
    let frame_bit = last * 8 + 7 - setup[last].leading_zeros() as usize; // the framing bit
    for k in (1..=64usize).rev() {
        let first = frame_bit.checked_sub(41 * k)?;
        if first < 7 * 8 + 6 {
            continue;
        }
        if bits_at(setup, first - 6, 6) as usize != k - 1 {
            continue;
        }
        let ok = (0..k).all(|j| {
            let m = first + 41 * j;
            bits_at(setup, m + 1, 16) == 0 && bits_at(setup, m + 17, 16) == 0
        });
        if ok {
            return Some((0..k).map(|j| bit_at(setup, first + 41 * j) == 1).collect());
        }
    }
    None
}

/// Samples (at 48 kHz) in an Opus packet, from its TOC byte and frame count.
fn opus_samples(p: &[u8]) -> Option<i64> {
    let toc = *p.first()?;
    let cfg = toc >> 3;
    // Frame length in microseconds: SILK (0-11), hybrid (12-15) and CELT (16-31) configurations.
    let frame_us: i64 = match cfg {
        0..=11 => [10_000, 20_000, 40_000, 60_000][(cfg & 3) as usize],
        12..=15 => [10_000, 20_000][(cfg & 1) as usize],
        _ => [2_500, 5_000, 10_000, 20_000][(cfg & 3) as usize],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (*p.get(1)? & 0x3F) as i64,
    };
    Some(frames * frame_us * 48 / 1000)
}

/// Length of the first packet of a page (the lacing values up to the first one below 255).
fn first_packet_len(lacing: &[u8]) -> usize {
    let mut n = 0usize;
    for &l in lacing {
        n += l as usize;
        if l < 255 {
            break;
        }
    }
    n
}

impl<S: Source> OggDemuxer<S> {
    /// Read the header packets of `src`.
    pub async fn open(src: S) -> Result<Self> {
        let rd = Reader::new(src).await;
        let mut me = Self {
            rd,
            win: Win::new(),
            streams: Vec::new(),
            meta: Metadata::default(),
            duration: None,
            codec: Codec::Opus { pre_skip: 0 },
            serial: 0,
            rate: 48_000,
            first_audio: 0,
            pos: 0,
            carry: Vec::new(),
            skip_continued: false,
            cursor: None,
            limit: None,
            queue: VecDeque::new(),
            done: false,
        };
        me.read_headers().await?;
        me.find_end().await;
        Ok(me)
    }

    /// Read the page at `at` (or the next one if there is junk before it). `None` at the end of the data.
    async fn page(&mut self, mut at: u64) -> Result<Option<Page>> {
        loop {
            let w = self.win.get(&mut self.rd, at, 27 + 255).await?;
            if w.len() < 27 {
                return Ok(None);
            }
            if &w[..4] != b"OggS" || w[4] != 0 {
                // Junk or damage: look for the next capture pattern.
                at += w
                    .windows(4)
                    .skip(1)
                    .position(|x| x == b"OggS")
                    .map_or(w.len() as u64 - 3, |p| p as u64 + 1);
                continue;
            }
            let nseg = w[26] as usize;
            if w.len() < 27 + nseg {
                self.rd.refresh_size().await;
                return Err(Error::Truncated);
            }
            let lacing = w[27..27 + nseg].to_vec();
            let (htype, granule) =
                (w[5], i64::from_le_bytes([w[6], w[7], w[8], w[9], w[10], w[11], w[12], w[13]]));
            let serial = u32::from_le_bytes([w[14], w[15], w[16], w[17]]);
            let blen: usize = lacing.iter().map(|&l| l as usize).sum();
            let body_at = at + 27 + nseg as u64;
            let body = self.win.get(&mut self.rd, body_at, blen).await?;
            if body.len() < blen {
                self.rd.refresh_size().await;
                return Err(Error::Truncated);
            }
            return Ok(Some(Page {
                off: at,
                htype,
                granule,
                serial,
                lacing,
                body: body.to_vec(),
                next: body_at + blen as u64,
            }));
        }
    }

    /// Split a page into the packets that end on it; `carry` holds the start of one that does not.
    fn packets(&mut self, pg: &Page) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut at = 0usize;
        let mut cur = core::mem::take(&mut self.carry);
        if pg.htype & 1 == 0 {
            cur.clear(); // a packet that should continue does not (lost page): start fresh
        }
        let mut dropping = self.skip_continued && pg.htype & 1 != 0;
        self.skip_continued = false;
        for &l in &pg.lacing {
            cur.extend_from_slice(&pg.body[at..at + l as usize]);
            at += l as usize;
            if l < 255 {
                if dropping {
                    dropping = false;
                } else {
                    out.push(core::mem::take(&mut cur));
                }
                cur.clear();
            }
        }
        if dropping {
            cur.clear();
        }
        self.carry = cur;
        out
    }

    async fn read_headers(&mut self) -> Result<()> {
        let mut at = 0u64;
        // The logical stream being read, and its header packets so far.
        let mut chosen = false;
        let mut hdrs: Vec<Vec<u8>> = Vec::new();
        let mut want = 0usize; // header packets expected (0: not known)
        let mut kind = 0u8; // 1 vorbis, 2 opus, 3 flac
        for _ in 0..4096 {
            let Some(pg) = self.page(at).await? else { break };
            at = pg.next;
            if !chosen {
                if pg.htype & 2 == 0 {
                    break; // the beginning-of-stream pages are over and none was ours
                }
                // The first packet of a beginning-of-stream page names the codec.
                let first = &pg.body[..first_packet_len(&pg.lacing).min(pg.body.len())];
                kind = if first.starts_with(b"\x01vorbis") {
                    1
                } else if first.starts_with(b"OpusHead") {
                    2
                } else if first.starts_with(b"\x7fFLAC") {
                    3
                } else {
                    continue; // another stream's beginning (video, Speex...): keep looking
                };
                chosen = true;
                self.serial = pg.serial;
                want = match kind {
                    1 => 3,
                    2 => 2,
                    _ => first.get(7..9).map_or(0, |n| u16::from_be_bytes([n[0], n[1]]) as usize + 1),
                };
            }
            if pg.serial != self.serial {
                continue;
            }
            let mut complete = false;
            for p in self.packets(&pg) {
                if complete {
                    break; // audio sharing the last header page is not supported
                }
                if kind == 3 && !hdrs.is_empty() && p.first() == Some(&0xFF) {
                    // The first FLAC frame: the headers were over (their number was not given).
                    complete = true;
                    self.carry.clear();
                    self.first_audio = pg.off;
                    self.pos = pg.off;
                    break;
                }
                hdrs.push(p);
                if want > 0 && hdrs.len() >= want {
                    complete = true;
                }
            }
            if complete {
                if self.first_audio == 0 {
                    self.first_audio = pg.next;
                    self.pos = pg.next;
                }
                return self.finish_headers(kind, hdrs);
            }
        }
        Err(Error::Unsupported("no Vorbis, Opus or FLAC stream in the Ogg file".to_string()))
    }

    fn finish_headers(&mut self, kind: u8, hdrs: Vec<Vec<u8>>) -> Result<()> {
        let bad = || Error::Invalid("damaged Ogg stream headers".to_string());
        let (codec, rate, channels, extra): (&str, u32, u16, Vec<u8>) = match kind {
            1 => {
                let id = hdrs.first().filter(|h| h.len() >= 30).ok_or_else(bad)?;
                let setup = hdrs.get(2).ok_or_else(bad)?;
                let rate = u32::from_le_bytes([id[12], id[13], id[14], id[15]]);
                let blocks = [1u32 << (id[28] & 15), 1u32 << (id[28] >> 4)];
                let modes = vorbis_modes(setup)
                    .ok_or_else(|| Error::Invalid("Vorbis setup header without modes".to_string()))?;
                let mode_bits = ilog(modes.len() as u32 - 1);
                if let Some(c) = hdrs.get(1).and_then(|c| c.strip_prefix(b"\x03vorbis")) {
                    tags::parse_vorbis_comments(c, &mut self.meta);
                }
                self.codec = Codec::Vorbis { blocks, modes, mode_bits, prev: None };
                // Xiph lacing of the three headers, as Matroska stores them (and the decoder takes them).
                let mut x = alloc::vec![2u8];
                for n in [hdrs[0].len(), hdrs[1].len()] {
                    x.extend(core::iter::repeat_n(255u8, n / 255));
                    x.push((n % 255) as u8);
                }
                for h in &hdrs {
                    x.extend_from_slice(h);
                }
                ("vorbis", rate, id[11] as u16, x)
            }
            2 => {
                let head = hdrs.first().filter(|h| h.len() >= 19).ok_or_else(bad)?;
                let pre_skip = u16::from_le_bytes([head[10], head[11]]) as i64;
                if let Some(c) = hdrs.get(1).and_then(|c| c.strip_prefix(b"OpusTags")) {
                    tags::parse_vorbis_comments(c, &mut self.meta);
                }
                self.codec = Codec::Opus { pre_skip };
                self.cursor = None;
                ("opus", 48_000, head[9] as u16, head.clone())
            }
            _ => {
                let first = hdrs.first().filter(|h| h.len() >= 9 + 4 + 4 + 34).ok_or_else(bad)?;
                let si = &first[9..9 + 4 + 4 + 34]; // "fLaC", block header, STREAMINFO
                let packed =
                    u64::from_be_bytes([si[18], si[19], si[20], si[21], si[22], si[23], si[24], si[25]]);
                let (min_b, max_b) =
                    (u16::from_be_bytes([si[8], si[9]]) as u32, u16::from_be_bytes([si[10], si[11]]) as u32);
                self.codec = Codec::Flac { fixed: (min_b == max_b && min_b > 0).then_some(min_b) };
                let mut best_art: Option<(u32, rvp_core::Art)> = None;
                for h in &hdrs[1..] {
                    let (Some(&t), Some(body)) = (h.first(), h.get(4..)) else { continue };
                    match t & 0x7F {
                        4 => tags::parse_vorbis_comments(body, &mut self.meta),
                        6 => {
                            if let Some((pt, art)) = tags::parse_flac_picture(body) {
                                if best_art.as_ref().is_none_or(|(b, _)| pt == 3 && *b != 3) {
                                    best_art = Some((pt, art));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if self.meta.art.is_none() {
                    self.meta.art = best_art.map(|(_, a)| a);
                }
                ("flac", (packed >> 44) as u32, (((packed >> 41) & 7) + 1) as u16, si.to_vec())
            }
        };
        if rate == 0 || channels == 0 {
            return Err(bad());
        }
        self.rate = rate as i64;
        self.streams = alloc::vec![StreamInfo {
            id: 1,
            kind: StreamKind::Audio,
            codec: codec.to_string(),
            time_base: Rational::new(1, 1_000_000),
            language: None,
            extra_data: extra,
            video: None,
            audio: Some(AudioInfo { sample_rate: rate, channels }),
            duration_us: None,
        }];
        Ok(())
    }

    /// The last granule position of the stream gives its length.
    async fn find_end(&mut self) {
        let Some(size) = self.rd.refresh_size().await else { return };
        let mut from = size.saturating_sub(128 << 10).max(self.first_audio);
        loop {
            let mut best: Option<i64> = None;
            let mut at = from;
            while let Ok(Some(pg)) = self.page(at).await {
                at = pg.next;
                if pg.serial == self.serial && pg.granule >= 0 {
                    best = Some(pg.granule);
                }
            }
            if let Some(g) = best {
                let pre = if let Codec::Opus { pre_skip } = self.codec { pre_skip } else { 0 };
                self.limit = Some(g - pre);
                self.duration = Some(((g - pre).max(0) as i128 * 1_000_000 / self.rate as i128) as i64);
                if let Some(s) = self.streams.first_mut() {
                    s.duration_us = self.duration;
                }
                return;
            }
            if from <= self.first_audio || size - from > 4 << 20 {
                return;
            }
            from = from.saturating_sub(1 << 20).max(self.first_audio);
        }
    }

    /// Turn the packets that end on `pg` into timestamped packets in the queue.
    fn queue_audio(&mut self, pg: &Page, pk: Vec<Vec<u8>>) {
        // Duration of each packet, in samples.
        let mut durs: Vec<i64> = Vec::with_capacity(pk.len());
        let mut firsts: Vec<i64> = Vec::new(); // FLAC: sample numbers from the frame headers
        for p in &pk {
            let d = match &mut self.codec {
                Codec::Vorbis { blocks, modes, mode_bits, prev } => {
                    if p.first().is_none_or(|&b| b & 1 != 0) {
                        0
                    } else {
                        let mode = bits_at(p, 1, *mode_bits as usize) as usize;
                        let bs = blocks[modes.get(mode).copied().unwrap_or(false) as usize];
                        let d = prev.map_or(0, |pb| (pb / 4 + bs / 4) as i64);
                        *prev = Some(bs);
                        d
                    }
                }
                Codec::Opus { .. } => opus_samples(p).unwrap_or(0),
                Codec::Flac { fixed } => match flac::parse_frame_header(p, *fixed) {
                    Some(fi) => {
                        firsts.push(fi.sample as i64);
                        fi.block as i64
                    }
                    None => {
                        firsts.push(-1);
                        0
                    }
                },
            };
            durs.push(d);
        }
        let sum: i64 = durs.iter().sum();
        // Where the page's packets start: the granule position is the end of the last packet that completes here.
        let eos = pg.htype & 4 != 0;
        let start = match (pg.granule >= 0 && !eos, self.cursor) {
            (true, _) => pg.granule - sum,
            (false, Some(c)) => c,
            (false, None) => (pg.granule - sum).max(0),
        };
        let pre = if let Codec::Opus { pre_skip } = self.codec { pre_skip } else { 0 };
        let mut at = start;
        for (i, (p, d)) in pk.into_iter().zip(durs).enumerate() {
            let s = if matches!(self.codec, Codec::Flac { .. }) {
                firsts.get(i).copied().filter(|&f| f >= 0).unwrap_or(at)
            } else {
                at
            };
            let to_us = |x: i64| (x as i128 * 1_000_000 / self.rate as i128) as i64;
            let (pts, dur) = (to_us(s - pre), to_us(s - pre + d) - to_us(s - pre));
            let discard = self.limit.map_or(0, |l| (to_us(s - pre + d) - to_us(l)).clamp(0, dur));
            at = s + d;
            if p.is_empty() {
                continue;
            }
            self.queue.push_back(Packet {
                stream_id: 1,
                pts,
                dts: pts,
                duration: dur,
                keyframe: true,
                discard_end_us: discard,
                data: p,
            });
        }
        self.cursor = Some(at);
    }
}

impl<S: Source> Demuxer for OggDemuxer<S> {
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
        loop {
            if let Some(p) = self.queue.pop_front() {
                return Ok(Some(p));
            }
            if self.done {
                return Ok(None);
            }
            let Some(pg) = self.page(self.pos).await? else { return Ok(None) };
            self.pos = pg.next;
            if pg.serial != self.serial {
                if pg.htype & 2 != 0 && self.cursor.is_some() {
                    // A new logical stream begins after ours: a chained file. Ours is over.
                    self.done = true;
                    return Ok(None);
                }
                continue;
            }
            if pg.htype & 2 != 0 && self.cursor.is_some_and(|c| c > 0) {
                self.done = true;
                return Ok(None);
            }
            let pk = self.packets(&pg);
            self.queue_audio(&pg, pk);
        }
    }

    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        self.queue.clear();
        self.carry.clear();
        self.done = false;
        let pre = if let Codec::Opus { pre_skip } = self.codec { pre_skip } else { 0 };
        let preroll = if matches!(self.codec, Codec::Opus { .. }) { OPUS_PREROLL } else { 0 };
        let want = (target_us.max(0) as i128 * self.rate as i128 / 1_000_000) as i64 + pre - preroll;
        // Bisect on byte offsets for the last page whose granule position is at or before `want`.
        let (mut lo, mut hi) = (self.first_audio, self.rd.refresh_size().await.unwrap_or(self.first_audio));
        while hi.saturating_sub(lo) > 64 << 10 {
            let mid = lo + (hi - lo) / 2;
            let mut at = mid;
            let mut found = None;
            while let Some(pg) = self.page(at).await? {
                at = pg.next;
                if pg.serial == self.serial && pg.granule >= 0 {
                    found = Some(pg);
                    break;
                }
                if at >= hi {
                    break;
                }
            }
            match found {
                Some(pg) if pg.granule <= want => lo = pg.off,
                _ => hi = mid,
            }
        }
        // Walk on from `lo` to the latest page that starts a fresh packet and begins at or before `want` (a page that continues
        // a packet cannot be decoded from its start). A run of continued pages falls back to walking from the beginning.
        let mut land = None;
        for from in [lo, self.first_audio] {
            let mut at = from;
            let mut prev_g = i64::MIN;
            while let Some(pg) = self.page(at).await? {
                at = pg.next;
                if pg.serial != self.serial {
                    continue;
                }
                if prev_g > want {
                    break;
                }
                if pg.htype & 1 == 0 {
                    land = Some(pg.off);
                }
                if pg.granule >= 0 {
                    prev_g = pg.granule;
                }
            }
            if land.is_some() {
                break;
            }
        }
        let off = land.unwrap_or(self.first_audio);
        self.pos = off;
        self.cursor = None;
        if let Codec::Vorbis { prev, .. } = &mut self.codec {
            *prev = None;
        }
        // Read the landing page so the time landed on is known.
        while self.queue.is_empty() {
            let Some(pg) = self.page(self.pos).await? else { break };
            self.pos = pg.next;
            if pg.serial != self.serial {
                continue;
            }
            let pk = self.packets(&pg);
            self.queue_audio(&pg, pk);
        }
        Ok(self.queue.front().map_or(target_us, |p| p.pts))
    }
}
