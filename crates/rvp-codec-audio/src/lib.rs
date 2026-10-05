//! Audio decoders behind [`rvp_core::AudioDecoder`]: AAC-LC, MPEG audio layers I to III (MP1, MP2, MP3), FLAC and
//! Vorbis through the unmodified `symphonia` codec crates (MPL-2.0), Opus through `ropus` (BSD-3-Clause), and PCM
//! (WAV) in this crate.
//!
//! Output is interleaved `f32` with the stream's own channel count (the player mixes more than two channels down to the
//! sink). Limits: AAC is LC only, mono or stereo. Symphonia 0.6 has no SBR synthesis and no AAC with more than two channels:
//! such a track (an object type other than LC, so HE-AAC with explicit signalling, or a channel configuration above two)
//! is reported as `Unsupported` ("aac too complex") and a video plays without sound; HE-AAC signalled backward compatibly
//! (the usual way) decodes as its LC core, at the core's sample rate, band-limited. Opus is channel-mapping family 0
//! (mono or stereo); PCM, FLAC and Vorbis carry up to eight channels.
#![forbid(unsafe_code)]

mod chain;
mod opus;
mod pcm;
mod sym;

use rvp_core::{AudioCodec, AudioDecoder, Error, Result, StreamInfo, StreamKind};

/// Codecs this crate can decode.
pub fn supported() -> &'static [AudioCodec] {
    &[
        AudioCodec::Aac,
        AudioCodec::Mp3,
        AudioCodec::Mp2,
        AudioCodec::Mp1,
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
    let dec: Box<dyn AudioDecoder> = match info.codec.as_str() {
        "opus" => Box::new(opus::OpusDec::new(info)?),
        c if c.starts_with("pcm_") => Box::new(pcm::PcmDec::new(info)?),
        "aac" | "mp1" | "mp2" | "mp3" | "flac" | "vorbis" => Box::new(sym::SymphoniaDec::new(info)?),
        other => return Err(Error::Unsupported(format!("audio codec `{other}`"))),
    };
    // Codecs that Ogg chains can carry rebuild themselves from headers that arrive in the packet flow.
    Ok(match info.codec.as_str() {
        "opus" | "vorbis" | "flac" => Box::new(chain::ChainDec::new(info, dec)),
        _ => dec,
    })
}
