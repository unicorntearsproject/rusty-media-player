//! Ogg: Vorbis, Opus and FLAC-in-Ogg (`.ogg`, `.oga`, `.opus`). The first supported logical stream is played; other streams
//! multiplexed with it are skipped.
//!
//! A chained file (logical streams one after the other, each with its own headers) plays on through all its links of the same
//! codec. Timestamps continue across the join. The new link's headers go to the decoder as ordinary packets (a Vorbis
//! identification, comment and setup header, an `OpusHead`, a FLAC mapping header: none of them can be mistaken for audio) so
//! it can rebuild itself, and the decoder takes care of the Opus pre-skip, which in a chain is no longer at the start of the file.
//! When the end of the file belongs to another stream than the start, the whole file is scanned once for its links, so the
//! duration is the sum and seeks work across the links; otherwise (a chain that reuses one serial number and ends in a long link)
//! the joins are found while playing and only the length shown is that of the last link.
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

#[derive(Clone)]
enum Codec {
    Vorbis { blocks: [u32; 2], modes: Vec<bool>, mode_bits: u32, prev: Option<u32> },
    Opus { pre_skip: i64 },
    Flac { fixed: Option<u32> },
}

/// One logical stream of a chained file (a plain file has one).
struct Link {
    /// The page that begins it.
    start: u64,
    first_audio: u64,
    serial: u32,
    kind: u8,
    /// Its header packets, as the decoder wants them again when the link changes.
    hdrs: Vec<Vec<u8>>,
    codec: Codec,
    rate: i64,
    /// Time of its first sample in the timeline of the whole file.
    base_us: i64,
    /// Playable samples (the last granule position less the pre-skip), once known.
    len: Option<i64>,
}

/// The header packets of a logical stream and where its audio starts.
struct LinkHdrs {
    kind: u8,
    serial: u32,
    hdrs: Vec<Vec<u8>>,
    first_audio: u64,
}

/// What a beginning-of-stream page in the middle of the file means.
enum Link2 {
    /// The next link of the chain: reading goes on there.
    Switched,
    /// A stream we do not play: ignore its pages.
    Skip,
    /// A stream of another codec: the file ends here.
    End,
}

/// Most pages looked at in the scan for the links of a chained file.
const MAX_SCAN_PAGES: u32 = 4_000_000;

/// An Ogg file.
pub struct OggDemuxer<S: Source> {
    rd: Reader<S>,
    win: Win,
    streams: Vec<StreamInfo>,
    meta: Metadata,
    duration: Option<Timestamp>,
    codec: Codec,
    /// The first link's header packets until they are in `links`.
    first_hdrs: Vec<Vec<u8>>,
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
    /// The links seen so far (all of them after a scan), the one being read, and its start in the file's timeline.
    links: Vec<Link>,
    cur: usize,
    base_us: i64,
    /// Header packets of a new link, handed out before its audio.
    headers: VecDeque<Packet>,
    /// Serial numbers of the streams that began at the start of the file.
    start_serials: Vec<u32>,
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

/// What the header packets of a logical stream say.
struct Params {
    codec: Codec,
    name: &'static str,
    rate: u32,
    channels: u16,
    /// The codec configuration as the decoder takes it.
    extra: Vec<u8>,
}

/// Interpret the header packets of a logical stream of `kind` (1 Vorbis, 2 Opus, 3 FLAC); tags go to `meta`.
fn parse_params(kind: u8, hdrs: &[Vec<u8>], meta: &mut Metadata) -> Result<Params> {
    let bad = || Error::Invalid("damaged Ogg stream headers".to_string());
    let p = match kind {
        1 => {
            let id = hdrs.first().filter(|h| h.len() >= 30).ok_or_else(bad)?;
            let setup = hdrs.get(2).ok_or_else(bad)?;
            let rate = u32::from_le_bytes([id[12], id[13], id[14], id[15]]);
            let blocks = [1u32 << (id[28] & 15), 1u32 << (id[28] >> 4)];
            let modes = vorbis_modes(setup)
                .ok_or_else(|| Error::Invalid("Vorbis setup header without modes".to_string()))?;
            let mode_bits = ilog(modes.len() as u32 - 1);
            if let Some(c) = hdrs.get(1).and_then(|c| c.strip_prefix(b"\x03vorbis")) {
                tags::parse_vorbis_comments(c, meta);
            }
            // Xiph lacing of the three headers, as Matroska stores them (and the decoder takes them).
            let mut x = alloc::vec![2u8];
            for n in [hdrs[0].len(), hdrs[1].len()] {
                x.extend(core::iter::repeat_n(255u8, n / 255));
                x.push((n % 255) as u8);
            }
            for h in hdrs {
                x.extend_from_slice(h);
            }
            Params {
                codec: Codec::Vorbis { blocks, modes, mode_bits, prev: None },
                name: "vorbis",
                rate,
                channels: id[11] as u16,
                extra: x,
            }
        }
        2 => {
            let head = hdrs.first().filter(|h| h.len() >= 19).ok_or_else(bad)?;
            let pre_skip = u16::from_le_bytes([head[10], head[11]]) as i64;
            if let Some(c) = hdrs.get(1).and_then(|c| c.strip_prefix(b"OpusTags")) {
                tags::parse_vorbis_comments(c, meta);
            }
            Params {
                codec: Codec::Opus { pre_skip },
                name: "opus",
                rate: 48_000,
                channels: head[9] as u16,
                extra: head.clone(),
            }
        }
        _ => {
            let first = hdrs.first().filter(|h| h.len() >= 9 + 4 + 4 + 34).ok_or_else(bad)?;
            let si = &first[9..9 + 4 + 4 + 34]; // "fLaC", block header, STREAMINFO
            let packed = u64::from_be_bytes([si[18], si[19], si[20], si[21], si[22], si[23], si[24], si[25]]);
            let (min_b, max_b) =
                (u16::from_be_bytes([si[8], si[9]]) as u32, u16::from_be_bytes([si[10], si[11]]) as u32);
            let mut best_art: Option<(u32, rvp_core::Art)> = None;
            for h in &hdrs[1..] {
                let (Some(&t), Some(body)) = (h.first(), h.get(4..)) else { continue };
                match t & 0x7F {
                    4 => tags::parse_vorbis_comments(body, meta),
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
            if meta.art.is_none() {
                meta.art = best_art.map(|(_, a)| a);
            }
            Params {
                codec: Codec::Flac { fixed: (min_b == max_b && min_b > 0).then_some(min_b) },
                name: "flac",
                rate: (packed >> 44) as u32,
                channels: (((packed >> 41) & 7) + 1) as u16,
                extra: si.to_vec(),
            }
        }
    };
    if p.rate == 0 || p.channels == 0 {
        return Err(bad());
    }
    Ok(p)
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
            first_hdrs: Vec::new(),
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
            links: Vec::new(),
            cur: 0,
            base_us: 0,
            headers: VecDeque::new(),
            start_serials: Vec::new(),
        };
        me.read_headers().await?;
        me.find_end().await;
        me.links.push(Link {
            start: 0,
            first_audio: me.first_audio,
            serial: me.serial,
            kind: me.kind_of_codec(),
            hdrs: core::mem::take(&mut me.first_hdrs),
            codec: me.codec.clone(),
            rate: me.rate,
            base_us: 0,
            len: me.limit,
        });
        if me.tail_has_other_stream().await {
            me.scan_links().await;
        }
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
            // -1 means no packet ends on the page; anything else outside a sane range (2^48 samples is nine thousand years
            // at 48 kHz) is damage and counts as that too, which keeps every later sum in range.
            let granule = if (0..1 << 48).contains(&granule) { granule } else { -1 };
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
        let Some(h) = self.read_link_headers(0, true).await? else {
            return Err(Error::Unsupported("no Vorbis, Opus or FLAC stream in the Ogg file".to_string()));
        };
        self.serial = h.serial;
        self.first_audio = h.first_audio;
        self.pos = h.first_audio;
        self.first_hdrs = h.hdrs.clone();
        self.finish_headers(h.kind, h.hdrs)
    }

    /// Read the headers of the first supported logical stream that begins at `at`. With `note` the serial numbers of every
    /// stream that begins there are kept (the file's start), to tell later on a stream that does not belong to it.
    async fn read_link_headers(&mut self, mut at: u64, note: bool) -> Result<Option<LinkHdrs>> {
        // The logical stream being read, and its header packets so far.
        let mut chosen = false;
        let mut serial = 0u32;
        let mut hdrs: Vec<Vec<u8>> = Vec::new();
        let mut want = 0usize; // header packets expected (0: not known)
        let mut kind = 0u8; // 1 vorbis, 2 opus, 3 flac
        let mut first_audio: Option<u64> = None;
        self.carry.clear();
        self.skip_continued = false;
        for _ in 0..4096 {
            let Some(pg) = self.page(at).await? else { break };
            at = pg.next;
            if note && pg.htype & 2 != 0 && !self.start_serials.contains(&pg.serial) {
                self.start_serials.push(pg.serial);
            }
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
                serial = pg.serial;
                want = match kind {
                    1 => 3,
                    2 => 2,
                    _ => first.get(7..9).map_or(0, |n| u16::from_be_bytes([n[0], n[1]]) as usize + 1),
                };
            }
            if pg.serial != serial {
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
                    first_audio = Some(pg.off);
                    break;
                }
                hdrs.push(p);
                if want > 0 && hdrs.len() >= want {
                    complete = true;
                }
            }
            if complete {
                return Ok(Some(LinkHdrs {
                    kind,
                    serial,
                    hdrs,
                    first_audio: first_audio.unwrap_or(pg.next),
                }));
            }
        }
        Ok(None)
    }

    fn kind_of_codec(&self) -> u8 {
        match self.codec {
            Codec::Vorbis { .. } => 1,
            Codec::Opus { .. } => 2,
            Codec::Flac { .. } => 3,
        }
    }

    fn finish_headers(&mut self, kind: u8, hdrs: Vec<Vec<u8>>) -> Result<()> {
        let mut meta = core::mem::take(&mut self.meta);
        let p = parse_params(kind, &hdrs, &mut meta);
        self.meta = meta;
        let p = p?;
        self.codec = p.codec;
        self.cursor = None;
        self.rate = p.rate as i64;
        self.streams = alloc::vec![StreamInfo {
            id: 1,
            kind: StreamKind::Audio,
            codec: p.name.to_string(),
            time_base: Rational::new(1, 1_000_000),
            language: None,
            extra_data: p.extra,
            video: None,
            audio: Some(AudioInfo { sample_rate: p.rate, channels: p.channels }),
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

    /// True when the end of the file holds a stream that did not begin at the start of the file, or a stream beginning later
    /// (a chain): then the whole file has to be scanned for its links.
    async fn tail_has_other_stream(&mut self) -> bool {
        let Some(size) = self.rd.refresh_size().await else { return false };
        let mut at = size.saturating_sub(128 << 10).max(self.first_audio);
        while let Ok(Some(pg)) = self.page(at).await {
            at = pg.next;
            if !self.start_serials.contains(&pg.serial) || (pg.htype & 2 != 0 && pg.off > self.first_audio) {
                return true;
            }
        }
        false
    }

    fn to_us(&self, samples: i64, rate: i64) -> i64 {
        (samples as i128 * 1_000_000 / rate.max(1) as i128) as i64
    }

    /// Walk the pages of the file once to find every link of a chain, with its length. Gives up quietly on damage.
    async fn scan_links(&mut self) {
        let Some(first) = self.links.pop() else { return };
        let kind = first.kind;
        let mut links = alloc::vec![first];
        let mut last_g: i64 = -1;
        let mut at = links[0].first_audio;
        let mut n = 0u32;
        let pre_of = |c: &Codec| if let Codec::Opus { pre_skip } = c { *pre_skip } else { 0 };
        while let Ok(Some(pg)) = self.page(at).await {
            n += 1;
            if n > MAX_SCAN_PAGES {
                break;
            }
            at = pg.next;
            let cur = links.len() - 1;
            if pg.htype & 2 != 0 && pg.off > links[cur].first_audio {
                if let Ok(Some(h)) = self.read_link_headers(pg.off, false).await {
                    let mut m = Metadata::default();
                    if let (true, Ok(p)) = (h.kind == kind, parse_params(h.kind, &h.hdrs, &mut m)) {
                        let len = (last_g >= 0).then(|| (last_g - pre_of(&links[cur].codec)).max(0));
                        links[cur].len = len;
                        let base = links[cur].base_us + self.to_us(len.unwrap_or(0), links[cur].rate);
                        at = h.first_audio;
                        last_g = -1;
                        links.push(Link {
                            start: pg.off,
                            first_audio: h.first_audio,
                            serial: h.serial,
                            kind,
                            hdrs: h.hdrs,
                            codec: p.codec,
                            rate: p.rate as i64,
                            base_us: base,
                            len: None,
                        });
                        continue;
                    }
                }
            }
            if pg.serial == links[cur].serial && pg.granule >= 0 {
                last_g = pg.granule;
            }
        }
        let cur = links.len() - 1;
        if last_g >= 0 {
            links[cur].len = Some((last_g - pre_of(&links[cur].codec)).max(0));
        }
        if links.len() > 1 {
            let total: i64 = links.iter().map(|l| self.to_us(l.len.unwrap_or(0), l.rate)).sum();
            self.duration = Some(total);
            if let Some(st) = self.streams.first_mut() {
                st.duration_us = Some(total);
            }
            self.limit = links[0].len;
        }
        self.links = links;
        self.cur = 0;
    }

    /// Make link `i` the one being read (the codec's state, the serial, the clock offset).
    fn apply_link(&mut self, i: usize) {
        let l = &self.links[i];
        self.cur = i;
        self.serial = l.serial;
        self.codec = l.codec.clone();
        self.rate = l.rate;
        self.limit = l.len;
        self.base_us = l.base_us;
        self.first_audio = l.first_audio;
        self.cursor = None;
        self.carry.clear();
        self.skip_continued = false;
    }

    /// Queue the current link's header packets for the decoder, ahead of its audio. With `from_start` the audio that follows is
    /// the start of the link, and the `OpusHead` packet's duration says how much of it (the pre-skip, in microseconds) the
    /// decoder is to drop; at the start of the file the player trims that itself, by the negative first timestamp.
    fn queue_headers(&mut self, from_start: bool) {
        let l = &self.links[self.cur];
        let n = if l.kind == 1 { l.hdrs.len() } else { 1 };
        let skip_us = match l.codec {
            Codec::Opus { pre_skip } if from_start && self.cur > 0 => self.to_us(pre_skip, 48_000),
            _ => 0,
        };
        for h in l.hdrs.iter().take(n) {
            self.headers.push_back(Packet {
                stream_id: 1,
                pts: l.base_us,
                dts: l.base_us,
                duration: if l.kind == 2 { skip_us } else { 0 },
                keyframe: true,
                discard_end_us: 0,
                data: h.clone(),
            });
        }
    }

    /// A beginning-of-stream page at `off` while a link is being read: move on to the link it starts.
    async fn next_link(&mut self, off: u64) -> Result<Link2> {
        let idx = match self.links.iter().position(|l| l.start == off) {
            Some(i) => i,
            None => {
                let Some(h) = self.read_link_headers(off, false).await? else { return Ok(Link2::Skip) };
                if h.kind != self.links[self.cur].kind {
                    return Ok(Link2::End);
                }
                let mut m = Metadata::default();
                let Ok(p) = parse_params(h.kind, &h.hdrs, &mut m) else { return Ok(Link2::End) };
                // The new link starts where the one that just ended did.
                let prev = &self.links[self.cur];
                let pre = if let Codec::Opus { pre_skip } = prev.codec { pre_skip } else { 0 };
                let len = self.cursor.map_or(prev.len.unwrap_or(0), |c| (c - pre).max(0));
                let base = prev.base_us + self.to_us(len, prev.rate);
                self.links.push(Link {
                    start: off,
                    first_audio: h.first_audio,
                    serial: h.serial,
                    kind: h.kind,
                    hdrs: h.hdrs,
                    codec: p.codec,
                    rate: p.rate as i64,
                    base_us: base,
                    len: None,
                });
                self.links.len() - 1
            }
        };
        self.apply_link(idx);
        self.pos = self.links[idx].first_audio;
        self.queue_headers(true);
        Ok(Link2::Switched)
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
            let (pts, dur) = (self.base_us + to_us(s - pre), to_us(s - pre + d) - to_us(s - pre));
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
            if let Some(p) = self.headers.pop_front() {
                return Ok(Some(p));
            }
            if let Some(p) = self.queue.pop_front() {
                return Ok(Some(p));
            }
            if self.done {
                return Ok(None);
            }
            let Some(pg) = self.page(self.pos).await? else { return Ok(None) };
            self.pos = pg.next;
            if pg.htype & 2 != 0 && self.cursor.is_some_and(|c| c > 0) {
                // A new logical stream begins after audio of ours: the next link of a chained file.
                match self.next_link(pg.off).await? {
                    Link2::Switched => continue,
                    Link2::Skip if pg.serial != self.serial => continue,
                    Link2::Skip | Link2::End => {
                        self.done = true;
                        return Ok(None);
                    }
                }
            }
            if pg.serial != self.serial {
                continue;
            }
            let pk = self.packets(&pg);
            self.queue_audio(&pg, pk);
        }
    }

    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        self.queue.clear();
        self.headers.clear();
        self.carry.clear();
        self.done = false;
        if self.links.len() > 1 {
            // A chain: the link that holds the target, and its headers again (the decoder may be set up for another one).
            let i = self.links.iter().rposition(|l| l.base_us <= target_us).unwrap_or(0);
            self.apply_link(i);
        }
        let pre = if let Codec::Opus { pre_skip } = self.codec { pre_skip } else { 0 };
        let preroll = if matches!(self.codec, Codec::Opus { .. }) { OPUS_PREROLL } else { 0 };
        let local_us = (target_us - self.base_us).max(0);
        let want = (local_us as i128 * self.rate as i128 / 1_000_000) as i64 + pre - preroll;
        // Bisect on byte offsets for the last page whose granule position is at or before `want`.
        let size = self.rd.refresh_size().await.unwrap_or(self.first_audio);
        let end = self.links.get(self.cur + 1).map_or(size, |l| l.start);
        let (mut lo, mut hi) = (self.first_audio, end);
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
                if prev_g > want || pg.off >= end {
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
        if self.links.len() > 1 {
            // The decoder may be set up for another link: give it this one's headers again.
            self.queue_headers(off <= self.first_audio);
        }
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
