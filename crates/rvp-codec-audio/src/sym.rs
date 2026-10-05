//! Symphonia-backed decoders (AAC, MP3, FLAC, Vorbis).
use rvp_core::{AudioBuffer, AudioDecoder, AudioParams, Error, Packet, Result, StreamInfo};
use std::collections::VecDeque;
use symphonia_core::audio::Channels;
use symphonia_core::codecs::audio::well_known::{CODEC_ID_AAC, CODEC_ID_FLAC, CODEC_ID_MP3, CODEC_ID_VORBIS};
use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoder as SymDecoder, AudioDecoderOptions};
use symphonia_core::packet::Packet as SymPacket;
use symphonia_core::units::{Duration, Timestamp};

pub(crate) struct SymphoniaDec {
    dec: Box<dyn SymDecoder>,
    out: VecDeque<AudioBuffer>,
}

/// FLAC: the decoder wants the 34-byte STREAMINFO body. Matroska stores `fLaC` + metadata blocks, MP4's `dfLa`
/// box stores version/flags + metadata blocks; both put STREAMINFO (block type 0) first.
fn flac_streaminfo(extra: &[u8]) -> Option<&[u8]> {
    let blocks = if extra.starts_with(b"fLaC") { &extra[4..] } else { extra.get(4..)? };
    let (hdr, rest) = blocks.split_at_checked(4)?;
    (hdr[0] & 0x7F == 0 && rest.len() >= 34).then(|| &rest[..34])
}

fn sym_err(e: symphonia_core::errors::Error) -> Error {
    Error::Invalid(format!("decoder: {e}"))
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
            "mp3" => CODEC_ID_MP3,
            "flac" => CODEC_ID_FLAC,
            "vorbis" => CODEC_ID_VORBIS,
            other => return Err(Error::Unsupported(format!("audio codec `{other}`"))),
        });
        let opts = AudioDecoderOptions::default();
        let dec: Box<dyn SymDecoder> = match info.codec.as_str() {
            "aac" => Box::new(symphonia_codec_aac::AacDecoder::try_new(&p, &opts).map_err(sym_err)?),
            "mp3" => Box::new(symphonia_bundle_mp3::MpaDecoder::try_new(&p, &opts).map_err(sym_err)?),
            "flac" => Box::new(symphonia_bundle_flac::FlacDecoder::try_new(&p, &opts).map_err(sym_err)?),
            _ => Box::new(symphonia_codec_vorbis::VorbisDecoder::try_new(&p, &opts).map_err(sym_err)?),
        };
        Ok(Self { dec, out: VecDeque::new() })
    }
}

impl AudioDecoder for SymphoniaDec {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
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
