//! Opus decoding with the header's output gain applied. The pre-skip is not handled here: containers express
//! it as a negative start timestamp (Matroska `CodecDelay`, MP4 edit list) and the player trims audio before
//! stream time zero, which is exactly the pre-skip.
use opus_decoder::OpusDecoder;
use rvp_core::{AudioBuffer, AudioDecoder, AudioParams, Error, Packet, Result, StreamInfo};
use std::collections::VecDeque;

pub(crate) struct OpusDec {
    dec: OpusDecoder,
    channels: usize,
    gain: f32,
    out: VecDeque<AudioBuffer>,
}

/// Parse `OpusHead` (Matroska CodecPrivate, little endian) or `dOps` (MP4, big endian): (channels, pre-skip,
/// output gain in Q7.8 dB, mapping family).
fn parse_head(d: &[u8]) -> Option<(usize, usize, i16, u8)> {
    if let Some(h) = d.strip_prefix(b"OpusHead") {
        let h = h.get(..11)?;
        Some((
            h[1] as usize,
            u16::from_le_bytes([h[2], h[3]]) as usize,
            i16::from_le_bytes([h[8], h[9]]),
            h[10],
        ))
    } else {
        let h = d.get(..11)?; // version, channels, pre-skip u16, rate u32, gain i16, family
        Some((
            h[1] as usize,
            u16::from_be_bytes([h[2], h[3]]) as usize,
            i16::from_be_bytes([h[8], h[9]]),
            h[10],
        ))
    }
}

impl OpusDec {
    pub(crate) fn new(info: &StreamInfo) -> Result<Self> {
        let (channels, _pre_skip, gain_q8, family) =
            parse_head(&info.extra_data).ok_or_else(|| Error::Invalid("bad Opus header".into()))?;
        if family != 0 || !(1..=2).contains(&channels) {
            return Err(Error::Unsupported("Opus multistream / more than 2 channels".into()));
        }
        let dec = OpusDecoder::new(48_000, channels).map_err(|e| Error::Invalid(format!("opus: {e:?}")))?;
        let gain = 10f32.powf(gain_q8 as f32 / (20.0 * 256.0));
        Ok(Self { dec, channels, gain, out: VecDeque::new() })
    }
}

impl AudioDecoder for OpusDec {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        let mut pcm = vec![0f32; 5760 * self.channels];
        let n = self
            .dec
            .decode_float(&packet.data, &mut pcm, false)
            .map_err(|e| Error::Invalid(format!("opus: {e:?}")))?;
        pcm.truncate(n * self.channels);
        let pts = packet.pts;
        if (self.gain - 1.0).abs() > 1e-6 {
            for s in &mut pcm {
                *s *= self.gain;
            }
        }
        if !pcm.is_empty() {
            self.out.push_back(AudioBuffer {
                params: AudioParams { sample_rate: 48_000, channels: self.channels as u16 },
                samples: pcm,
                pts,
            });
        }
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
