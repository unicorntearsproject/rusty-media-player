//! Demuxers for MP4 (ISO BMFF), Matroska/WebM and the raw audio formats (MP3, FLAC, Ogg, WAV, ADTS AAC), incremental over a
//! host [`Source`].
//!
//! ```ignore
//! let mut d = rvp_demux::open(source).await?;        // sniffs the container
//! for s in d.streams() { /* codec, time base, extra_data ... */ }
//! while let Some(pkt) = d.next_packet().await? { /* route to a decoder */ }
//! let landed = d.seek(30_000_000).await?;           // keyframe at or before 30 s
//! ```
//!
//! Packets come out in file order (interleaved as muxed). `pts` and `dts` are microseconds; Matroska has no
//! decode timestamps, so there `dts == pts`.
#![no_std]
#![allow(async_fn_in_trait)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod flac;
mod framed;
mod io;
pub mod mkv;
pub mod mp4;
pub mod ogg;
mod tags;
pub mod wav;

use alloc::string::ToString;
use rvp_core::{Chapter, Error, Metadata, Packet, Result, StreamInfo, Timestamp};
use rvp_host::Source;

pub use flac::FlacDemuxer;
pub use framed::{AdtsDemuxer, Mp3Demuxer};
pub use mkv::MkvDemuxer;
pub use mp4::Mp4Demuxer;
pub use ogg::OggDemuxer;
pub use wav::WavDemuxer;

/// Container families we can demux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// ISO base media file format: `.mp4`, `.m4a`, `.m4v`, `.mov`-compatible.
    Mp4,
    /// EBML: Matroska (`.mkv`) or WebM (`.webm`).
    Matroska,
    /// MPEG audio layer III, raw (`.mp3`), with or without ID3 tags.
    Mp3,
    /// Native FLAC (`.flac`).
    Flac,
    /// Ogg: Vorbis, Opus or FLAC (`.ogg`, `.oga`, `.opus`).
    Ogg,
    /// RIFF WAVE (`.wav`).
    Wav,
    /// AAC in ADTS frames (`.aac`).
    Adts,
}

/// Guess the container from the first bytes of a file (at least 12 bytes recommended).
pub fn sniff(head: &[u8]) -> Option<Container> {
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return Some(Container::Mp4);
    }
    if head.len() >= 4 && head[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some(Container::Matroska);
    }
    if head.len() >= 12
        && (&head[..4] == b"RIFF" || &head[..4] == b"RF64" || &head[..4] == b"BW64")
        && &head[8..12] == b"WAVE"
    {
        return Some(Container::Wav);
    }
    if head.starts_with(b"fLaC") {
        return Some(Container::Flac);
    }
    if head.starts_with(b"OggS") {
        return Some(Container::Ogg);
    }
    // An ID3v2 tag in front: MP3 unless what follows the tag (see `open`) says otherwise.
    if head.starts_with(b"ID3") {
        return Some(Container::Mp3);
    }
    if head.len() >= 2 && head[0] == 0xFF {
        // ADTS has layer bits 00; MPEG audio has layer III (01), II (10) or I (11).
        if head[1] & 0xF6 == 0xF0 {
            return Some(Container::Adts);
        }
        if head[1] & 0xE0 == 0xE0 && (head[1] >> 1) & 3 != 0 {
            return Some(Container::Mp3);
        }
    }
    None
}

/// An incremental demuxer over a host `Source`.
pub trait Demuxer {
    /// Streams found in the container, in container order.
    fn streams(&self) -> &[StreamInfo];
    /// Duration in microseconds, if known.
    fn duration_us(&self) -> Option<Timestamp>;
    /// Title, artist, album and cover art, as far as the container has them.
    fn metadata(&self) -> &Metadata;
    /// Chapter marks in time order (empty when there are none).
    fn chapters(&self) -> &[Chapter];
    /// Next packet in file order, `None` at the end.
    async fn next_packet(&mut self) -> Result<Option<Packet>>;
    /// Seek so the next packets start at the keyframe (of the first video stream, else the first stream) at or
    /// before `target_us`, clamped to the first keyframe; returns the time landed on.
    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp>;
}

/// Any of the demuxers, chosen by [`open`].
pub enum AnyDemuxer<S: Source> {
    /// MP4 / ISO BMFF.
    Mp4(Mp4Demuxer<S>),
    /// Matroska / WebM.
    Mkv(MkvDemuxer<S>),
    /// Raw MP3.
    Mp3(Mp3Demuxer<S>),
    /// Native FLAC.
    Flac(FlacDemuxer<S>),
    /// Ogg.
    Ogg(OggDemuxer<S>),
    /// WAV.
    Wav(WavDemuxer<S>),
    /// ADTS AAC.
    Adts(AdtsDemuxer<S>),
}

/// Sniff `src` and open the matching demuxer.
pub async fn open<S: Source>(mut src: S) -> Result<AnyDemuxer<S>> {
    let mut head = [0u8; 12];
    let mut n = 0;
    while n < head.len() {
        let r = src.read_at(n as u64, &mut head[n..]).await?;
        if r == 0 {
            break;
        }
        n += r;
    }
    let mut kind = sniff(&head[..n]);
    if kind == Some(Container::Mp3) && head.starts_with(b"ID3") {
        // Tags in front: what follows them decides (an ID3v2 tag is also found before FLAC and AAC).
        let mut at = 0u64;
        let mut after = [0u8; 12];
        loop {
            let mut h = [0u8; 10];
            if src.read_at(at, &mut h).await? < 10 {
                break;
            }
            match tags::id3v2_len(&h) {
                Some(len) => at += len,
                None => {
                    let mut got = 0;
                    while got < after.len() {
                        let r = src.read_at(at + got as u64, &mut after[got..]).await?;
                        if r == 0 {
                            break;
                        }
                        got += r;
                    }
                    if let Some(k) = sniff(&after[..got]) {
                        kind = Some(k);
                    }
                    break;
                }
            }
        }
    }
    match kind {
        Some(Container::Mp4) => Ok(AnyDemuxer::Mp4(Mp4Demuxer::open(src).await?)),
        Some(Container::Matroska) => Ok(AnyDemuxer::Mkv(MkvDemuxer::open(src).await?)),
        Some(Container::Mp3) => Ok(AnyDemuxer::Mp3(Mp3Demuxer::open(src).await?)),
        Some(Container::Flac) => Ok(AnyDemuxer::Flac(FlacDemuxer::open(src).await?)),
        Some(Container::Ogg) => Ok(AnyDemuxer::Ogg(OggDemuxer::open(src).await?)),
        Some(Container::Wav) => Ok(AnyDemuxer::Wav(WavDemuxer::open(src).await?)),
        Some(Container::Adts) => Ok(AnyDemuxer::Adts(AdtsDemuxer::open(src).await?)),
        None => Err(Error::Unsupported("unrecognised container".to_string())),
    }
}

macro_rules! each {
    ($self:ident, $d:ident => $e:expr) => {
        match $self {
            AnyDemuxer::Mp4($d) => $e,
            AnyDemuxer::Mkv($d) => $e,
            AnyDemuxer::Mp3($d) => $e,
            AnyDemuxer::Flac($d) => $e,
            AnyDemuxer::Ogg($d) => $e,
            AnyDemuxer::Wav($d) => $e,
            AnyDemuxer::Adts($d) => $e,
        }
    };
}

impl<S: Source> Demuxer for AnyDemuxer<S> {
    fn streams(&self) -> &[StreamInfo] {
        each!(self, d => d.streams())
    }

    fn duration_us(&self) -> Option<Timestamp> {
        each!(self, d => d.duration_us())
    }

    fn metadata(&self) -> &Metadata {
        each!(self, d => d.metadata())
    }

    fn chapters(&self) -> &[Chapter] {
        each!(self, d => d.chapters())
    }

    async fn next_packet(&mut self) -> Result<Option<Packet>> {
        each!(self, d => d.next_packet().await)
    }

    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        each!(self, d => d.seek(target_us).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_containers() {
        assert_eq!(sniff(b"\0\0\0\x18ftypisom\0\0\0\0"), Some(Container::Mp4));
        assert_eq!(sniff(&[0x1A, 0x45, 0xDF, 0xA3, 0x9F]), Some(Container::Matroska));
        assert_eq!(sniff(b"RIFF....WAVE"), Some(Container::Wav));
        assert_eq!(sniff(b"fLaC\0\0\0\0"), Some(Container::Flac));
        assert_eq!(sniff(b"OggS\0\x02"), Some(Container::Ogg));
        assert_eq!(sniff(&[0xFF, 0xF1, 0x50]), Some(Container::Adts));
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90]), Some(Container::Mp3));
        assert_eq!(sniff(b"RIFF....AVI "), None);
    }
}
