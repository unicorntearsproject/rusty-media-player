//! Inputs the fuzzer found: each must return an error or nothing, never panic or abort.
use rvp_core::{Packet, Rational, StreamInfo, StreamKind};

fn decoder() -> Box<dyn rvp_core::VideoDecoder> {
    rvp_codec_av1::av1_decoder(&StreamInfo {
        id: 1,
        kind: StreamKind::Video,
        codec: "av1".into(),
        time_base: Rational::new(1, 1_000_000),
        language: None,
        extra_data: Vec::new(),
        video: None,
        audio: None,
        duration_us: None,
    })
    .unwrap()
}

fn packet(data: &[u8]) -> Packet {
    Packet {
        stream_id: 1,
        pts: 0,
        dts: 0,
        duration: 0,
        keyframe: true,
        discard_end_us: 0,
        data: data.to_vec(),
    }
}

/// rav1d treats zero-length data as an input-validation failure and aborts the process.
#[test]
fn an_empty_packet_is_ignored() {
    let mut d = decoder();
    assert!(d.send_packet(&packet(&[])).is_ok());
    let _ = d.send_packet(&packet(&[0x12, 0x00]));
    assert!(d.send_packet(&packet(&[])).is_ok());
    while let Ok(Some(_)) = d.receive_frame() {}
    d.flush();
    let _ = d.drain();
}
