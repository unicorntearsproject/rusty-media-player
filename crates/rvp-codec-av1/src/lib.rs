//! AV1 decoding via rav1d (BSD-2-Clause). rav1d 1.1.0 does not build for wasm32-unknown-unknown yet
//! (docs/PLAN.md risk R1); Milestone 4 vendors a patched copy and fills this crate in.
#![forbid(unsafe_code)]

use rvp_core::VideoCodec;

/// The codec this crate decodes.
pub const CODEC: VideoCodec = VideoCodec::Av1;

#[cfg(test)]
mod tests {
    #[test]
    fn codec_id() {
        assert_eq!(super::CODEC, rvp_core::VideoCodec::Av1);
    }
}
