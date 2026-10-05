//! Uncompressed audio (WAV): 8-bit unsigned, 16/24/32-bit signed and 32/64-bit float, little endian, mono or stereo.
use rvp_core::{AudioBuffer, AudioDecoder, AudioParams, Error, Packet, Result, StreamInfo};
use std::collections::VecDeque;

#[derive(Clone, Copy)]
enum Fmt {
    U8,
    S16,
    S24,
    S32,
    F32,
    F64,
}

impl Fmt {
    fn bytes(self) -> usize {
        match self {
            Fmt::U8 => 1,
            Fmt::S16 => 2,
            Fmt::S24 => 3,
            Fmt::S32 | Fmt::F32 => 4,
            Fmt::F64 => 8,
        }
    }

    fn sample(self, b: &[u8]) -> f32 {
        match self {
            Fmt::U8 => (b[0] as f32 - 128.0) / 128.0,
            Fmt::S16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            // Sign-extend by placing the three bytes at the top of an i32.
            Fmt::S24 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            Fmt::S32 => (i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64 / 2_147_483_648.0) as f32,
            Fmt::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Fmt::F64 => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
        }
    }
}

pub(crate) struct PcmDec {
    fmt: Fmt,
    params: AudioParams,
    out: VecDeque<AudioBuffer>,
}

impl PcmDec {
    pub(crate) fn new(info: &StreamInfo) -> Result<Self> {
        let a = info.audio.ok_or_else(|| Error::Invalid("PCM stream without audio parameters".into()))?;
        if a.channels == 0 || a.channels > 2 || a.sample_rate == 0 {
            return Err(Error::Unsupported(format!("PCM with {} channels", a.channels)));
        }
        let fmt = match info.codec.as_str() {
            "pcm_u8" => Fmt::U8,
            "pcm_s16le" => Fmt::S16,
            "pcm_s24le" => Fmt::S24,
            "pcm_s32le" => Fmt::S32,
            "pcm_f32le" => Fmt::F32,
            "pcm_f64le" => Fmt::F64,
            other => return Err(Error::Unsupported(format!("audio codec `{other}`"))),
        };
        Ok(Self {
            fmt,
            params: AudioParams { sample_rate: a.sample_rate, channels: a.channels },
            out: VecDeque::new(),
        })
    }
}

impl AudioDecoder for PcmDec {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        let w = self.fmt.bytes();
        let samples: Vec<f32> = packet
            .data
            .chunks_exact(w * self.params.channels as usize)
            .flat_map(|f| f.chunks_exact(w))
            .map(|s| self.fmt.sample(s))
            .collect();
        if !samples.is_empty() {
            self.out.push_back(AudioBuffer { params: self.params, samples, pts: packet.pts });
        }
        Ok(())
    }

    fn receive_buffer(&mut self) -> Result<Option<AudioBuffer>> {
        Ok(self.out.pop_front())
    }

    fn flush(&mut self) {
        self.out.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvp_core::{AudioInfo, Rational, StreamKind};

    fn dec(codec: &str, channels: u16) -> Result<PcmDec> {
        PcmDec::new(&StreamInfo {
            id: 1,
            kind: StreamKind::Audio,
            codec: codec.into(),
            time_base: Rational::new(1, 1_000_000),
            language: None,
            extra_data: vec![],
            video: None,
            audio: Some(AudioInfo { sample_rate: 8000, channels }),
            duration_us: None,
        })
    }

    fn run(codec: &str, channels: u16, data: Vec<u8>) -> Vec<f32> {
        let mut d = dec(codec, channels).unwrap();
        d.send_packet(&Packet {
            stream_id: 1,
            pts: 0,
            dts: 0,
            duration: 0,
            keyframe: true,
            discard_end_us: 0,
            data,
        })
        .unwrap();
        d.receive_buffer().unwrap().unwrap().samples
    }

    #[test]
    fn converts_every_format_to_float() {
        assert_eq!(run("pcm_u8", 1, vec![0, 128, 255]), [-1.0, 0.0, 127.0 / 128.0]);
        assert_eq!(run("pcm_s16le", 1, vec![0x00, 0x80, 0xFF, 0x7F, 0, 0]), [-1.0, 32767.0 / 32768.0, 0.0]);
        assert_eq!(
            run("pcm_s24le", 1, vec![0, 0, 0x80, 0xFF, 0xFF, 0x7F]),
            [-1.0, 8_388_607.0 / 8_388_608.0]
        );
        assert_eq!(run("pcm_s32le", 1, vec![0, 0, 0, 0x80]), [-1.0]);
        assert_eq!(
            run("pcm_f32le", 2, [0.25f32.to_le_bytes(), (-0.5f32).to_le_bytes()].concat()),
            [0.25, -0.5]
        );
        assert_eq!(run("pcm_f64le", 1, 0.5f64.to_le_bytes().to_vec()), [0.5]);
        // A partial frame at the end of a packet is dropped, not misread.
        assert_eq!(run("pcm_s16le", 2, vec![1, 0, 2, 0, 3]), [1.0 / 32768.0, 2.0 / 32768.0]);
    }

    #[test]
    fn more_than_two_channels_are_refused() {
        assert!(dec("pcm_s16le", 6).is_err());
    }
}
