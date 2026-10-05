//! Audio decoders behind [`rvp_core::AudioDecoder`]: AAC-LC, MP3, FLAC and Vorbis through the unmodified
//! `symphonia` codec crates (MPL-2.0), Opus through `ropus` (BSD-3-Clause), and PCM (WAV) in this crate.
//!
//! Output is always interleaved `f32`. Limits: AAC is LC only (no SBR/HE-AAC, at most 2 channels, a symphonia
//! limit); Opus is channel-mapping family 0 (mono or stereo).
#![forbid(unsafe_code)]

mod opus;
mod pcm;
mod sym;

use rvp_core::{AudioCodec, AudioDecoder, Error, Result, StreamInfo, StreamKind};

/// Codecs this crate can decode.
pub fn supported() -> &'static [AudioCodec] {
    &[
        AudioCodec::Aac,
        AudioCodec::Mp3,
        AudioCodec::Flac,
        AudioCodec::Vorbis,
        AudioCodec::Opus,
        AudioCodec::Pcm,
    ]
}

/// Create a decoder for an audio stream described by a demuxer.
pub fn audio_decoder(info: &StreamInfo) -> Result<Box<dyn AudioDecoder>> {
    if info.kind != StreamKind::Audio {
        return Err(Error::Invalid("not an audio stream".into()));
    }
    match info.codec.as_str() {
        "opus" => Ok(Box::new(opus::OpusDec::new(info)?)),
        c if c.starts_with("pcm_") => Ok(Box::new(pcm::PcmDec::new(info)?)),
        "aac" | "mp3" | "flac" | "vorbis" => Ok(Box::new(sym::SymphoniaDec::new(info)?)),
        other => Err(Error::Unsupported(format!("audio codec `{other}`"))),
    }
}
