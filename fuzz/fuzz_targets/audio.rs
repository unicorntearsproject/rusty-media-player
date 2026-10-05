//! The audio decoders (AAC, MP3, FLAC, Vorbis, Opus, PCM) on arbitrary codec configuration and packets: first byte picks the
//! codec, then a length byte and that many bytes of extra data, then (u16 little-endian length, bytes) packets.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rvp_core::{AudioInfo, Packet, StreamInfo, StreamKind};

const CODECS: [&str; 8] = ["aac", "mp3", "flac", "vorbis", "opus", "pcm_s16le", "pcm_s24le", "pcm_f32le"];

fuzz_target!(|data: &[u8]| {
    let [codec, extra_len, rest @ ..] = data else { return };
    let extra_len = *extra_len as usize;
    if rest.len() < extra_len {
        return;
    }
    let (extra, mut rest) = rest.split_at(extra_len);
    let info = StreamInfo {
        id: 1,
        kind: StreamKind::Audio,
        codec: CODECS[*codec as usize % CODECS.len()].into(),
        time_base: rvp_core::Rational::new(1, 1_000_000),
        language: None,
        extra_data: extra.to_vec(),
        video: None,
        audio: Some(AudioInfo { sample_rate: 48_000, channels: 2 }),
        duration_us: None,
    };
    let Ok(mut dec) = rvp_codec_audio::audio_decoder(&info) else { return };
    let mut i = 0usize;
    while rest.len() >= 2 && i < 64 {
        let n = u16::from_le_bytes([rest[0], rest[1]]) as usize;
        let chunk = &rest[2..(2 + n).min(rest.len())];
        rest = &rest[(2 + n).min(rest.len())..];
        let p = Packet {
            stream_id: 1,
            pts: i as i64 * 20_000,
            dts: i as i64 * 20_000,
            duration: 20_000,
            keyframe: true,
            discard_end_us: 0,
            data: chunk.to_vec(),
        };
        let _ = dec.send_packet(&p);
        let mut guard = 0;
        while let Ok(Some(b)) = dec.receive_buffer() {
            assert!(b.samples.len() <= 16 << 20, "an absurdly large buffer");
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        i += 1;
    }
    dec.flush();
});
