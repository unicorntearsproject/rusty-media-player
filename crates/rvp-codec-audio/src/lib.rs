//! Audio decoders behind `rvp_core::AudioDecoder`: AAC, MP3, FLAC, Vorbis (symphonia codec crates,
//! unmodified) and Opus (`opus-decoder`). Milestone 3; this is the stub.
#![forbid(unsafe_code)]

use rvp_core::AudioCodec;

/// Codecs this crate can currently decode.
pub fn supported() -> &'static [AudioCodec] {
    &[]
}

#[cfg(test)]
mod tests {
    #[test]
    fn nothing_supported_yet() {
        assert!(super::supported().is_empty());
    }
}
