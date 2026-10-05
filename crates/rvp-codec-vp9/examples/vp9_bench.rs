//! Decode a WebM or MKV file with the VP9 decoder and report speed: `cargo run --release -p rvp-codec-vp9
//! --example vp9_bench -- file.webm [repeat]`. Pin one core (`taskset -c 3`) for single-thread numbers.
use rvp_core::StreamKind;
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: bench <file> [repeat]");
    let repeat: usize = args.next().map(|a| a.parse().unwrap()).unwrap_or(3);
    let data = std::fs::read(&path).unwrap();
    let (info, packets) = block_on(async {
        let mut d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let mut v = Vec::new();
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id == info.id {
                v.push(p);
            }
        }
        (info, v)
    });
    let (mut best, mut frames) = (f64::MAX, 0);
    for _ in 0..repeat {
        let mut dec = rvp_codec_vp9::vp9_decoder(&info).unwrap();
        frames = 0;
        let t = Instant::now();
        for p in &packets {
            dec.send_packet(p).unwrap();
            while dec.receive_frame().unwrap().is_some() {
                frames += 1;
            }
        }
        best = best.min(t.elapsed().as_secs_f64());
    }
    println!(
        "{path}: {frames} frames, best {:.0} ms, {:.2} ms/frame, {:.0} fps",
        best * 1e3,
        best * 1e3 / frames as f64,
        frames as f64 / best
    );
}
