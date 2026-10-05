//! The AV1 wrapper (rav1d) on arbitrary packets, framed as (u16 little-endian length, bytes) records: no panic or abort,
//! bounded memory.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rvp_core::{Packet, StreamInfo, StreamKind};

fuzz_target!(|data: &[u8]| {
    let info = StreamInfo {
        id: 1,
        kind: StreamKind::Video,
        codec: "av1".into(),
        time_base: rvp_core::Rational::new(1, 1_000_000),
        language: None,
        extra_data: Vec::new(),
        video: None,
        audio: None,
        duration_us: None,
    };
    let Ok(mut dec) = rvp_codec_av1::av1_decoder(&info) else { return };
    let mut rest = data;
    let mut i = 0usize;
    while rest.len() >= 2 && i < 32 {
        let n = u16::from_le_bytes([rest[0], rest[1]]) as usize;
        let chunk = &rest[2..(2 + n).min(rest.len())];
        rest = &rest[(2 + n).min(rest.len())..];
        let p = Packet {
            stream_id: 1,
            pts: i as i64 * 40_000,
            dts: i as i64 * 40_000,
            duration: 40_000,
            keyframe: i == 0,
            discard_end_us: 0,
            data: chunk.to_vec(),
        };
        let _ = dec.send_packet(&p);
        while let Ok(Some(_)) = dec.receive_frame() {}
        i += 1;
    }
    dec.flush();
    let _ = dec.drain();
});
