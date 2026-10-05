//! Our own H.264 / AVC decoder (Milestone 6): pure Rust, `no_std + alloc`, no `unsafe`.
//!
//! The crate is layered so that an encoder can share everything that is not decoder specific:
//!
//! * [`bitstream`]: NAL units, RBSP escaping, [`bitstream::BitReader`] and [`bitstream::BitWriter`], Exp-Golomb.
//! * [`params`]: SPS, PPS, scaling lists and slice headers, each with `parse` and `write`.
//! * [`transform`]: forward and inverse integer transforms, quantisation, the zig-zag scans.
//! * [`cavlc`]: residual coding tables with a reader and a writer.
//! * [`cabac`]: the context tables and the arithmetic decoding and encoding engines.
//! * [`decoder`]: the picture decoder itself (macroblock layer, prediction, deblocking, DPB).
//!
//! Scope: progressive 8-bit 4:2:0, Baseline, Main and High profiles, including multiple slices, deblocking,
//! weighted prediction, direct modes, long-term references, all three POC types and output reordering.
//! Interlaced coding (PAFF, MBAFF), FMO/ASO, data partitioning, SP/SI slices and bit depths above 8 are
//! rejected with a clean error.
#![no_std]
#![forbid(unsafe_code)]
// Codec code is index arithmetic over small blocks; iterator rewrites of these loops read worse.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod bitstream;
pub mod cabac;
pub mod cavlc;
pub mod decoder;
pub mod error;
pub mod params;
pub mod transform;

pub use error::{Error, Result};
use rvp_core::VideoCodec;

/// The codec this crate decodes.
pub const CODEC: VideoCodec = VideoCodec::H264;

/// Highest implemented stage (see `docs/PLAN.md` M6); 0 means nothing yet.
pub const STAGE: u8 = 6;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use rvp_core::{
    ColorMatrix, ColorRange, Packet, PixelFormat, StreamInfo, StreamKind, VideoDecoder, VideoFrame,
};

/// Create an H.264 decoder for a video stream (`info.codec == "h264"`, `info.extra_data` an avcC record).
pub fn h264_decoder(info: &StreamInfo) -> rvp_core::Result<Box<dyn VideoDecoder>> {
    if info.kind != StreamKind::Video || info.codec != "h264" {
        return Err(rvp_core::Error::Unsupported(alloc::format!("not an H.264 stream: {}", info.codec)));
    }
    let mut dec = decoder::Decoder::new();
    if !info.extra_data.is_empty() {
        dec.set_avcc(&info.extra_data)?;
    }
    Ok(Box::new(H264VideoDecoder { dec, out: VecDeque::new() }))
}

/// [`rvp_core::VideoDecoder`] adapter around [`decoder::Decoder`].
pub struct H264VideoDecoder {
    dec: decoder::Decoder,
    out: VecDeque<VideoFrame>,
}

fn to_video_frame(f: decoder::Frame) -> VideoFrame {
    let matrix = match f.matrix_coefficients {
        1 => ColorMatrix::Bt709,
        5 | 6 => ColorMatrix::Bt601,
        9 | 10 => ColorMatrix::Bt2020,
        _ if f.height >= 720 => ColorMatrix::Bt709,
        _ => ColorMatrix::Bt601,
    };
    VideoFrame {
        width: f.width as u32,
        height: f.height as u32,
        format: PixelFormat::Yuv420p8,
        matrix,
        range: if f.full_range { ColorRange::Full } else { ColorRange::Limited },
        planes: f.planes,
        strides: f.strides,
        pts: f.pts,
    }
}

impl H264VideoDecoder {
    fn collect(&mut self) {
        while let Some(f) = self.dec.next_frame() {
            self.out.push_back(to_video_frame(f));
        }
    }
}

impl VideoDecoder for H264VideoDecoder {
    fn send_packet(&mut self, packet: &Packet) -> rvp_core::Result<()> {
        let r = self.dec.decode_sample(&packet.data, packet.pts);
        self.collect();
        match r {
            // Damaged data is concealed and counted in the decoder statistics; only streams we cannot decode at
            // all are reported.
            Err(e @ Error::Unsupported(_)) => Err(e.into()),
            _ => Ok(()),
        }
    }

    fn receive_frame(&mut self) -> rvp_core::Result<Option<VideoFrame>> {
        Ok(self.out.pop_front())
    }

    fn flush(&mut self) {
        self.dec.reset();
        self.out.clear();
    }

    fn drain(&mut self) -> rvp_core::Result<()> {
        let _ = self.dec.flush();
        self.collect();
        Ok(())
    }
}
