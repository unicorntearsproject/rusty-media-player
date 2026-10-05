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
pub mod error;
pub mod media;
pub mod ring;
pub mod task;
pub mod time;

pub use clock::{ClockSource, MasterClock};
pub use codec::{AudioDecoder, VideoDecoder};
pub use error::{Error, Result};
pub use media::{
    AudioBuffer, AudioCodec, AudioParams, ColorMatrix, ColorRange, Packet, PixelFormat, StreamInfo,
    StreamKind, VideoCodec, VideoFrame,
};
pub use ring::RingBuffer;
pub use time::{Rational, Timestamp};
