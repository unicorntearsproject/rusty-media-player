//! Demuxers for MP4 (ISO BMFF) and Matroska/WebM, incremental over a host [`Source`].
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

mod io;
pub mod mkv;
pub mod mp4;

use alloc::string::ToString;
use rvp_core::{Error, Packet, Result, StreamInfo, Timestamp};
use rvp_host::Source;

pub use mkv::MkvDemuxer;
pub use mp4::Mp4Demuxer;

/// Container families we can demux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// ISO base media file format: `.mp4`, `.m4a`, `.m4v`, `.mov`-compatible.
    Mp4,
    /// EBML: Matroska (`.mkv`) or WebM (`.webm`).
    Matroska,
}

/// Guess the container from the first bytes of a file (at least 12 bytes recommended).
pub fn sniff(head: &[u8]) -> Option<Container> {
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return Some(Container::Mp4);
    }
    if head.len() >= 4 && head[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some(Container::Matroska);
    }
    None
}

/// An incremental demuxer over a host `Source`.
pub trait Demuxer {
    /// Streams found in the container, in container order.
    fn streams(&self) -> &[StreamInfo];
    /// Duration in microseconds, if known.
    fn duration_us(&self) -> Option<Timestamp>;
    /// Next packet in file order, `None` at the end.
    async fn next_packet(&mut self) -> Result<Option<Packet>>;
    /// Seek so the next packets start at the keyframe (of the first video stream, else the first stream) at or
    /// before `target_us`, clamped to the first keyframe; returns the time landed on.
    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp>;
}

/// Either demuxer, chosen by [`open`].
pub enum AnyDemuxer<S: Source> {
    /// MP4 / ISO BMFF.
    Mp4(Mp4Demuxer<S>),
    /// Matroska / WebM.
    Mkv(MkvDemuxer<S>),
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
    match sniff(&head[..n]) {
        Some(Container::Mp4) => Ok(AnyDemuxer::Mp4(Mp4Demuxer::open(src).await?)),
        Some(Container::Matroska) => Ok(AnyDemuxer::Mkv(MkvDemuxer::open(src).await?)),
        None => Err(Error::Unsupported("unrecognised container".to_string())),
    }
}

impl<S: Source> Demuxer for AnyDemuxer<S> {
    fn streams(&self) -> &[StreamInfo] {
        match self {
            Self::Mp4(d) => d.streams(),
            Self::Mkv(d) => d.streams(),
        }
    }

    fn duration_us(&self) -> Option<Timestamp> {
        match self {
            Self::Mp4(d) => d.duration_us(),
            Self::Mkv(d) => d.duration_us(),
        }
    }

    async fn next_packet(&mut self) -> Result<Option<Packet>> {
        match self {
            Self::Mp4(d) => d.next_packet().await,
            Self::Mkv(d) => d.next_packet().await,
        }
    }

    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp> {
        match self {
            Self::Mp4(d) => d.seek(target_us).await,
            Self::Mkv(d) => d.seek(target_us).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_containers() {
        assert_eq!(sniff(b"\0\0\0\x18ftypisom\0\0\0\0"), Some(Container::Mp4));
        assert_eq!(sniff(&[0x1A, 0x45, 0xDF, 0xA3, 0x9F]), Some(Container::Matroska));
        assert_eq!(sniff(b"RIFF....WAVE"), None);
    }
}
