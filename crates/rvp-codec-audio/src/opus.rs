//! Opus decoding with `ropus` (a bit-exact Rust port of libopus). The header's output gain is applied by the
//! decoder. The pre-skip is not handled here: containers express it as a negative start timestamp (Matroska
//! `CodecDelay`, MP4 edit list) and the player trims audio before stream time zero, which is exactly the
//! pre-skip.
use ropus::{Channels, DecodeMode, Decoder};
use rvp_core::{AudioBuffer, AudioDecoder, AudioParams, Error, Packet, Result, StreamInfo};
use std::collections::VecDeque;

pub(crate) struct OpusDec {
    dec: Decoder,
    channels: Channels,
    gain_q8: i32,
    out: VecDeque<AudioBuffer>,
}

/// Parse `OpusHead` (Matroska CodecPrivate, little endian) or `dOps` (MP4, big endian): (channels, output gain
/// in Q7.8 dB, mapping family).
fn parse_head(d: &[u8]) -> Option<(usize, i16, u8)> {
    if let Some(h) = d.strip_prefix(b"OpusHead") {
        let h = h.get(..11)?;
        Some((h[1] as usize, i16::from_le_bytes([h[8], h[9]]), h[10]))
    } else {
        let h = d.get(..11)?; // version, channels, pre-skip u16, rate u32, gain i16, family
        Some((h[1] as usize, i16::from_be_bytes([h[8], h[9]]), h[10]))
    }
}

fn new_decoder(channels: Channels, gain_q8: i32) -> Result<Decoder> {
    let mut dec = Decoder::new(48_000, channels).map_err(|e| Error::Invalid(format!("opus: {e}")))?;
    if gain_q8 != 0 {
        dec.set_gain(gain_q8).map_err(|e| Error::Invalid(format!("opus gain: {e}")))?;
    }
    Ok(dec)
}

impl OpusDec {
    pub(crate) fn new(info: &StreamInfo) -> Result<Self> {
        let (n, gain, family) =
            parse_head(&info.extra_data).ok_or_else(|| Error::Invalid("bad Opus header".into()))?;
        let channels = match (family, n) {
            (0, 1) => Channels::Mono,
            (0, 2) => Channels::Stereo,
            _ => return Err(Error::Unsupported("Opus multistream / more than 2 channels".into())),
        };
        let gain_q8 = gain as i32;
        Ok(Self { dec: new_decoder(channels, gain_q8)?, channels, gain_q8, out: VecDeque::new() })
    }
}

impl AudioDecoder for OpusDec {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        let n = self.channels.count();
        let mut pcm = vec![0f32; 5760 * n];
        let frames = self
            .dec
            .decode_float(&packet.data, &mut pcm, DecodeMode::Normal)
            .map_err(|e| Error::Invalid(format!("opus: {e}")))?;
        pcm.truncate(frames * n);
        if !pcm.is_empty() {
            self.out.push_back(AudioBuffer {
                params: AudioParams { sample_rate: 48_000, channels: n as u16 },
                samples: pcm,
                pts: packet.pts,
            });
        }
        Ok(())
    }

    fn receive_buffer(&mut self) -> Result<Option<AudioBuffer>> {
        Ok(self.out.pop_front())
    }

    fn flush(&mut self) {
        self.out.clear();
        if let Ok(d) = new_decoder(self.channels, self.gain_q8) {
            self.dec = d;
        }
    }
}
