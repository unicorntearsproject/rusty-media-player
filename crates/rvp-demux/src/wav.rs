//! WAV (RIFF/WAVE and RF64) with integer or floating point PCM, plus `LIST`/`INFO` and `id3 ` tags.
//!
//! The stream's codec is one of `pcm_u8`, `pcm_s16le`, `pcm_s24le`, `pcm_s32le`, `pcm_f32le`, `pcm_f64le`; packets are
//! whole sample frames, [`FRAMES_PER_PACKET`] of them (the last one shorter).
use crate::Demuxer;
use crate::framed::audio_stream;
use crate::io::Reader;
use crate::tags;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_core::{Chapter, Error, Metadata, Packet, Result, StreamInfo, Timestamp};
use rvp_host::Source;

/// Sample frames in one packet.
pub const FRAMES_PER_PACKET: u64 = 4096;

/// A WAV file.
pub struct WavDemuxer<S: Source> {
    rd: Reader<S>,
    streams: Vec<StreamInfo>,
    meta: Metadata,
    duration: Option<Timestamp>,
    /// File offset of the first sample frame.
    data: u64,
    /// Bytes of sample data the header claims (clamped to the file as far as it is known).
    data_len: u64,
    block: u64,
    rate: u64,
    /// Next sample frame to hand out.
    frame: u64,
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7]])
}

/// The sample format of a `fmt ` chunk: (codec name, bytes per sample).
fn format(tag: u16, bits: u16, ext: Option<&[u8]>) -> Result<(&'static str, u64)> {
    // WAVE_FORMAT_EXTENSIBLE: the real tag is the first two bytes of the sub-format GUID.
    let tag = if tag == 0xFFFE { ext.filter(|e| e.len() >= 24).map_or(tag, |e| le16(e, 8)) } else { tag };
    Ok(match (tag, bits) {
        (1, 8) => ("pcm_u8", 1),
        (1, 16) => ("pcm_s16le", 2),
        (1, 24) => ("pcm_s24le", 3),
        (1, 32) => ("pcm_s32le", 4),
        (3, 32) => ("pcm_f32le", 4),
        (3, 64) => ("pcm_f64le", 8),
        _ => return Err(Error::Unsupported(alloc::format!("WAV format tag {tag:#x} with {bits} bits"))),
    })
}

fn info_text(b: &[u8]) -> Option<String> {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let s = String::from_utf8_lossy(&b[..end]).trim().to_string();
    (!s.is_empty()).then_some(s)
}

impl<S: Source> WavDemuxer<S> {
    /// Read the header chunks of `src`.
    pub async fn open(src: S) -> Result<Self> {
        let mut rd = Reader::new(src).await;
        let mut head = [0u8; 12];
        if rd.read_upto(0, &mut head).await? < 12
            || !(&head[..4] == b"RIFF" || &head[..4] == b"RF64" || &head[..4] == b"BW64")
            || &head[8..12] != b"WAVE"
        {
            return Err(Error::Invalid("not a WAV file".to_string()));
        }
        let mut meta = Metadata::default();
        let mut fmt: Option<(&'static str, u64, u16, u32)> = None;
        let mut ds64_data: Option<u64> = None;
        let mut data: Option<(u64, u64)> = None;
        let mut at = 12u64;
        // Walk the chunks. `data` is usually in the middle and tags come after it, so keep going past it, but never read a
        // chunk list further than a few thousand entries.
        for _ in 0..4096 {
            let mut ch = [0u8; 8];
            if rd.read_upto(at, &mut ch).await? < 8 {
                break;
            }
            let id = [ch[0], ch[1], ch[2], ch[3]];
            let mut size = le32(&ch, 4) as u64;
            let body = at + 8;
            let mut data_chunk = false;
            match &id {
                b"ds64" => {
                    if let Ok(b) = rd.read_vec(body, size.min(64)).await {
                        if b.len() >= 16 {
                            ds64_data = Some(le64(&b, 8));
                        }
                    }
                }
                b"fmt " => {
                    let b = rd.read_vec(body, size.min(256)).await?;
                    if b.len() < 16 {
                        return Err(Error::Invalid("short WAV fmt chunk".to_string()));
                    }
                    let (tag, ch_n, rate, bits) = (le16(&b, 0), le16(&b, 2), le32(&b, 4), le16(&b, 14));
                    let ext = (b.len() >= 40).then(|| &b[16..]);
                    let (codec, bps) = format(tag, bits, ext)?;
                    if ch_n == 0 || rate == 0 {
                        return Err(Error::Invalid("WAV with no channels or no sample rate".to_string()));
                    }
                    fmt = Some((codec, bps, ch_n, rate));
                }
                b"data" => {
                    if size == 0xFFFF_FFFF || (size == 0 && ds64_data.is_some()) {
                        size = ds64_data.unwrap_or(u64::MAX);
                    }
                    data = Some((body, size));
                    data_chunk = true;
                }
                b"LIST" => {
                    if let Ok(b) = rd.read_vec(body, size.min(1 << 20)).await {
                        if b.starts_with(b"INFO") {
                            let mut o = 4usize;
                            while o + 8 <= b.len() {
                                let n = le32(&b, o + 4) as usize;
                                let Some(v) = b.get(o + 8..o + 8 + n) else { break };
                                let t = info_text(v);
                                match (&b[o..o + 4], t) {
                                    (b"INAM", t @ Some(_)) => meta.title = meta.title.take().or(t),
                                    (b"IART", t @ Some(_)) => meta.artist = meta.artist.take().or(t),
                                    (b"IPRD", t @ Some(_)) => meta.album = meta.album.take().or(t),
                                    (b"IGNR", t @ Some(_)) => meta.genre = meta.genre.take().or(t),
                                    (b"ICRD", Some(t)) => {
                                        meta.year = meta.year.or_else(|| {
                                            let d: String =
                                                t.chars().take_while(char::is_ascii_digit).collect();
                                            if d.len() == 4 { d.parse().ok() } else { None }
                                        })
                                    }
                                    (b"ITRK" | b"IPRT", Some(t)) => {
                                        let mut it = t.split('/');
                                        meta.track =
                                            meta.track.or(it.next().and_then(|v| v.trim().parse().ok()));
                                        meta.track_total = meta
                                            .track_total
                                            .or(it.next().and_then(|v| v.trim().parse().ok()));
                                    }
                                    _ => {}
                                }
                                o += 8 + n + (n & 1);
                            }
                        }
                    }
                }
                b"id3 " | b"ID3 " if size <= tags::MAX_ID3 => {
                    if let Ok(b) = rd.read_vec(body, size).await {
                        tags::parse_id3v2(&b, &mut meta);
                    }
                }
                _ => {}
            }
            // A data chunk of unknown or oversized length runs to the end of the file; after it nothing else can be found.
            if data_chunk
                && (size == u64::MAX
                    || body.saturating_add(size) > rd.refresh_size().await.unwrap_or(u64::MAX))
            {
                break;
            }
            // Chunks are padded to an even size.
            at = body.saturating_add(size).saturating_add(size & 1);
        }
        let (codec, bps, channels, rate) =
            fmt.ok_or_else(|| Error::Invalid("WAV without a fmt chunk".to_string()))?;
        let (data_off, mut data_len) =
            data.ok_or_else(|| Error::Invalid("WAV without a data chunk".to_string()))?;
        let block = bps * channels as u64;
        // The header's length is a claim: the file may be shorter (cut off) or the length left open (streamed).
        data_len =
            data_len.min(rd.refresh_size().await.map_or(u64::MAX, |size| size.saturating_sub(data_off)));
        let frames = data_len / block;
        let duration = (frames as u128 * 1_000_000 / rate as u128).min(i64::MAX as u128) as i64;
        Ok(Self {
            rd,
            streams: alloc::vec![audio_stream(codec, rate, channels, Vec::new(), Some(duration))],
            meta,
            duration: Some(duration),
            data: data_off,
            data_len: frames * block,
            block,
            rate: rate as u64,
            frame: 0,
        })
    }

    fn us(&self, frame: u64) -> i64 {
        (frame as u128 * 1_000_000 / self.rate as u128) as i64
    }
}

impl<S: Source> Demuxer for WavDemuxer<S> {
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
        let total = self.data_len / self.block;
        if self.frame >= total {
            // A file that is still being written may have more by now.
            if let Some(size) = self.rd.refresh_size().await {
                let have = size.saturating_sub(self.data) / self.block;
                if have > total && self.data_len < u64::MAX {
                    self.data_len = have * self.block;
                } else {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        }
        let total = self.data_len / self.block;
        let n = FRAMES_PER_PACKET.min(total - self.frame);
        let data =
            self.rd.read_vec(self.data.saturating_add(self.frame * self.block), n * self.block).await?;
        let pts = self.us(self.frame);
        let dur = self.us(self.frame + n) - pts;
        self.frame += n;
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
        let f = (target_us.max(0) as u128 * self.rate as u128 / 1_000_000) as u64;
        // On a packet boundary, like the packets of a straight read.
        self.frame = (f / FRAMES_PER_PACKET * FRAMES_PER_PACKET).min(self.data_len / self.block);
        Ok(self.us(self.frame))
    }
}
