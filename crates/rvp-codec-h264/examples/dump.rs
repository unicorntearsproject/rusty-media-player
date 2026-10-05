//! Decode an Annex B (`.h264`/`.264`) stream or an MP4/MKV file and write raw planar yuv420p to stdout (for diffing
//! against `ffmpeg -f rawvideo`): `cargo run --release -p rvp-codec-h264 --example dump -- file > out.yuv`.
use rvp_codec_h264::decoder::Decoder;
use rvp_core::StreamKind;
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::io::Write;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump <file>");
    let data = std::fs::read(&path).unwrap();
    let mut dec = Decoder::new();
    let mut out = std::io::stdout().lock();
    let mut emit = |dec: &mut Decoder| {
        while let Some(f) = dec.next_frame() {
            for p in &f.planes {
                out.write_all(p).unwrap();
            }
        }
    };
    if path.ends_with(".h264") || path.ends_with(".264") {
        let _ = dec.decode_annexb(&data, 0);
        emit(&mut dec);
    } else {
        block_on(async {
            let mut d = open(MemSource::new(data)).await.unwrap();
            let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
            dec.set_avcc(&info.extra_data).unwrap();
            while let Some(p) = d.next_packet().await.unwrap() {
                if p.stream_id == info.id {
                    let _ = dec.decode_sample(&p.data, p.pts);
                    emit(&mut dec);
                }
            }
        });
    }
    let _ = dec.flush();
    emit(&mut dec);
    eprintln!("{:?}", dec.stats());
}
