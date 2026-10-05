//! Demuxers for MP4 (ISO BMFF) and Matroska/WebM. Milestone 2 fills these in; for now this crate has
//! container sniffing and the `Demuxer` interface.
#![no_std]
#![allow(async_fn_in_trait)]

extern crate alloc;
#[cfg(test)]
extern crate std;

use rvp_core::{Packet, Result, StreamInfo, Timestamp};

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

/// An incremental demuxer over a host `Source`. Implemented in M2.
pub trait Demuxer {
    /// Streams found in the container.
    fn streams(&self) -> &[StreamInfo];
    /// Duration in microseconds, if known.
    fn duration_us(&self) -> Option<Timestamp>;
    /// Next packet in file order, `None` at the end.
    async fn next_packet(&mut self) -> Result<Option<Packet>>;
    /// Seek so the next packets start at the keyframe at or before `target_us`; returns the landed time.
    async fn seek(&mut self, target_us: Timestamp) -> Result<Timestamp>;
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
