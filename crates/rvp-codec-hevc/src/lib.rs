//! HEVC / H.265: everything about a stream except the pixels. Written from the ITU-T H.265 text (clean-room, like the H.264 decoder).
//!
//! * [`bits`]: the RBSP bit reader and Exp-Golomb codes.
//! * [`nal`]: NAL unit headers, splitting (Annex B and length-prefixed) and emulation prevention.
//! * [`ps`]: video, sequence and picture parameter sets (Main and Main 10 profiles).
//! * [`slice`]: slice segment headers.
//! * [`dpb`]: picture order count, reference picture sets, reference lists, the decoded picture buffer and output order.
//! * [`stream`]: [`stream::HevcStream`], which turns samples into pictures for a [`stream::Backend`] (a hardware decoder today, a
//!   software one later) and hands the frames back in output order.
#![no_std]
#![forbid(unsafe_code)]
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod bits;
pub mod dpb;
pub mod nal;
pub mod ps;
pub mod slice;
pub mod stream;

mod error;
pub use error::{Error, Result};
