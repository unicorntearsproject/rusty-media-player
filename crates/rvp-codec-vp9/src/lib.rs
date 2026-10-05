//! VP9 decoding behind `rvp_core::VideoDecoder`. Milestone 7: benchmark `rusty_vp9` (Apache-2.0) and
//! `vp9dec` (MIT) and wrap the winner here.
#![forbid(unsafe_code)]

use rvp_core::VideoCodec;

/// The codec this crate decodes.
pub const CODEC: VideoCodec = VideoCodec::Vp9;

#[cfg(test)]
mod tests {
    #[test]
    fn codec_id() {
        assert_eq!(super::CODEC, rvp_core::VideoCodec::Vp9);
    }
}
