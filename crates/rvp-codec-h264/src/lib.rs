//! Our own H.264 decoder. Milestone 6, staged: 6a CAVLC I-frames, 6b P, 6c B, 6d CABAC, 6e High profile.
//! Pure Rust, `no_std + alloc`, no `unsafe`.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use rvp_core::VideoCodec;

/// The codec this crate decodes.
pub const CODEC: VideoCodec = VideoCodec::H264;

/// Highest implemented stage (see `docs/PLAN.md` M6); 0 means nothing yet.
pub const STAGE: u8 = 0;

#[cfg(test)]
mod tests {
    #[test]
    fn stage_zero() {
        assert_eq!(super::STAGE, 0);
    }
}
