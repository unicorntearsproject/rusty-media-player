//! Decode an MP4 or MKV file with the H.264 decoder and report speed: `cargo run --release -p rvp-codec-h264
//! --example bench -- file.mp4 [repeat]`. Prints the MD5-free checksum of the output so runs can be compared.
use rvp_codec_h264::decoder::Decoder;
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
    // Demux once.
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
    let mut best = f64::MAX;
    let mut frames = 0;
    let mut sum = 0u64;
    for _ in 0..repeat {
        let mut dec = Decoder::new();
        dec.set_avcc(&info.extra_data).unwrap();
        frames = 0;
        sum = 0;
        let t = Instant::now();
        for p in &packets {
            let _ = dec.decode_sample(&p.data, p.pts);
            while let Some(f) = dec.next_frame() {
                frames += 1;
                sum = sum
                    .wrapping_mul(31)
                    .wrapping_add(f.planes[0].iter().step_by(97).map(|&b| b as u64).sum::<u64>());
            }
        }
        let _ = dec.flush();
        while let Some(f) = dec.next_frame() {
            frames += 1;
            sum = sum
                .wrapping_mul(31)
                .wrapping_add(f.planes[0].iter().step_by(97).map(|&b| b as u64).sum::<u64>());
        }
        best = best.min(t.elapsed().as_secs_f64());
    }
    let v = info.video.unwrap();
    println!(
        "{path}: {}x{} {frames} frames, best {:.1} ms total, {:.2} ms/frame, {:.1} fps, checksum {sum:x}",
        v.width,
        v.height,
        best * 1000.0,
        best * 1000.0 / frames as f64,
        frames as f64 / best
    );
}
