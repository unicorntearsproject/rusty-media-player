//! Core types for rusty-video-player: time, media descriptors, the master clock and a ring buffer.
//!
//! `no_std + alloc`. No I/O, no global clock, no threads: everything time-dependent takes the
//! current time as an argument so it is deterministic and testable (see `docs/PLAN.md` sections 4-6).
#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod clock;
pub mod codec;
pub mod color;
pub mod dynamics;
pub mod error;
pub mod loudness;
pub mod media;
pub mod par;
pub mod platform;
pub mod resample;
pub mod ring;
pub mod settings;
pub mod simd;
pub mod stretch;
pub mod task;
pub mod time;

pub use clock::{ClockSource, MasterClock};
pub use codec::{AudioDecoder, CodecFactory, VideoDecoder};
pub use error::{Error, Result};
pub use loudness::{LoudnessMeter, Measurement};
pub use media::{
    Art, AudioBuffer, AudioCodec, AudioInfo, AudioParams, Chapter, ColorMatrix, ColorRange, LoudnessTags,
    Metadata, Packet, PixelFormat, StreamInfo, StreamKind, VideoCodec, VideoFrame, VideoInfo,
};
pub use platform::{
    FallbackVideo, PlatformSupport, PlatformVideo, avcc_bit_depth, codec_string, hevc_profile, open_video,
    ours_refuses, screened,
};
pub use resample::Resampler;
pub use ring::RingBuffer;
pub use settings::{AudioSettings, LevelMode};
pub use stretch::TimeStretcher;
pub use time::{Rational, Timestamp};
