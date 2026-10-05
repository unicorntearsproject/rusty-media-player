//! Symphonia-backed decoders (AAC, MPEG audio layers I to III, FLAC, Vorbis).
use rvp_core::{AudioBuffer, AudioDecoder, AudioParams, Error, Packet, Result, StreamInfo};
use std::collections::VecDeque;
use symphonia_core::audio::Channels;
use symphonia_core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_FLAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_VORBIS,
};
use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoder as SymDecoder, AudioDecoderOptions};
use symphonia_core::packet::Packet as SymPacket;
use symphonia_core::units::{Duration, Timestamp};

pub(crate) struct SymphoniaDec {
    dec: Box<dyn SymDecoder>,
    out: VecDeque<AudioBuffer>,
    /// MPEG audio: the parameters and the layer (1 to 3) the decoder was built for. Symphonia has one decoder per layer and
    /// the layer is in every frame header, so the decoder follows the stream (a Matroska or MP4 track says "mp3" for any layer).
    mpa: Option<(AudioCodecParameters, u8)>,
}

fn mpa_decoder(p: &mut AudioCodecParameters, layer: u8) -> Result<Box<dyn SymDecoder>> {
    p.for_codec(match layer {
        1 => CODEC_ID_MP1,
        2 => CODEC_ID_MP2,
        _ => CODEC_ID_MP3,
    });
    Ok(Box::new(
        symphonia_bundle_mp3::MpaDecoder::try_new(p, &AudioDecoderOptions::default()).map_err(sym_err)?,
    ))
}

/// FLAC: the decoder wants the 34-byte STREAMINFO body. Matroska stores `fLaC` + metadata blocks, MP4's `dfLa`
/// box stores version/flags + metadata blocks; both put STREAMINFO (block type 0) first.
fn flac_streaminfo(extra: &[u8]) -> Option<&[u8]> {
    let blocks = if extra.starts_with(b"fLaC") { &extra[4..] } else { extra.get(4..)? };
    let (hdr, rest) = blocks.split_at_checked(4)?;
    (hdr[0] & 0x7F == 0 && rest.len() >= 34).then(|| &rest[..34])
}

fn sym_err(e: symphonia_core::errors::Error) -> Error {
    match e {
        // HE-AAC and AAC with more than two channels end up here: say so, so the player can go on without sound.
        symphonia_core::errors::Error::Unsupported(what) => Error::Unsupported(format!("decoder: {what}")),
        e => Error::Invalid(format!("decoder: {e}")),
    }
}

impl SymphoniaDec {
    pub(crate) fn new(info: &StreamInfo) -> Result<Self> {
        let mut p = AudioCodecParameters::new();
        let audio = info.audio;
        if let Some(a) = audio {
            p.with_sample_rate(a.sample_rate);
            p.with_channels(match a.channels {
                1 => Channels::Positioned(symphonia_core::audio::Position::FRONT_CENTER),
                _ => Channels::Positioned(
                    symphonia_core::audio::Position::FRONT_LEFT
                        | symphonia_core::audio::Position::FRONT_RIGHT,
                ),
            });
        }
        let extra: Option<Box<[u8]>> = match info.codec.as_str() {
            "flac" => Some(
                flac_streaminfo(&info.extra_data)
                    .ok_or_else(|| Error::Invalid("FLAC stream without STREAMINFO".into()))?
                    .into(),
            ),
            _ if info.extra_data.is_empty() => None,
            _ => Some(info.extra_data.clone().into()),
        };
        if let Some(e) = extra {
            p.with_extra_data(e);
        }
        p.for_codec(match info.codec.as_str() {
            "aac" => CODEC_ID_AAC,
            "mp1" => CODEC_ID_MP1,
            "mp2" => CODEC_ID_MP2,
            "mp3" => CODEC_ID_MP3,
            "flac" => CODEC_ID_FLAC,
            "vorbis" => CODEC_ID_VORBIS,
            other => return Err(Error::Unsupported(format!("audio codec `{other}`"))),
        });
        let opts = AudioDecoderOptions::default();
        let dec: Box<dyn SymDecoder> = match info.codec.as_str() {
            "aac" => Box::new(symphonia_codec_aac::AacDecoder::try_new(&p, &opts).map_err(sym_err)?),
            "mp1" | "mp2" | "mp3" => {
                let layer = match info.codec.as_str() {
                    "mp1" => 1,
                    "mp2" => 2,
                    _ => 3,
                };
                let dec = mpa_decoder(&mut p, layer)?;
                return Ok(Self { dec, out: VecDeque::new(), mpa: Some((p, layer)) });
            }
            "flac" => Box::new(symphonia_bundle_flac::FlacDecoder::try_new(&p, &opts).map_err(sym_err)?),
            _ => Box::new(symphonia_codec_vorbis::VorbisDecoder::try_new(&p, &opts).map_err(sym_err)?),
        };
        Ok(Self { dec, out: VecDeque::new(), mpa: None })
    }
}

impl AudioDecoder for SymphoniaDec {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if let (Some((p, layer)), Some(b)) = (&mut self.mpa, packet.data.get(1)) {
            // Layer bits: 3 = I, 2 = II, 1 = III.
            let l = 4 - ((b >> 1) & 3);
            if (1..=3).contains(&l) && l != *layer {
                self.dec = mpa_decoder(p, l)?;
                *layer = l;
            }
        }
        let sp = SymPacket::new(
            packet.stream_id,
            Timestamp::new(packet.pts),
            Duration::new(0),
            packet.data.as_slice(),
        );
        let buf = self.dec.decode(&sp).map_err(sym_err)?;
        if buf.frames() == 0 {
            return Ok(());
        }
        let spec = buf.spec();
        let channels = spec.channels().count() as u16;
        let mut samples: Vec<f32> = Vec::with_capacity(buf.frames() * channels as usize);
        buf.copy_to_vec_interleaved::<f32>(&mut samples);
        self.out.push_back(AudioBuffer {
            params: AudioParams { sample_rate: spec.rate(), channels },
            samples,
            pts: packet.pts,
        });
        Ok(())
    }

    fn receive_buffer(&mut self) -> Result<Option<AudioBuffer>> {
        Ok(self.out.pop_front())
    }

    fn flush(&mut self) {
        self.out.clear();
        self.dec.reset();
    }
}
