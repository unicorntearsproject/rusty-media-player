//! Decode an MP4 or MKV file with the software HEVC decoder and report speed: `cargo run --release -p rvp-codec-hevc --example bench --
//! file.mp4 [repeat] [max_frames]`. Prints a checksum of the output so runs can be compared.
use rvp_codec_hevc::sw::hevc_decoder;
use rvp_core::StreamKind;
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: bench <file> [repeat] [max_frames]");
    let repeat: usize = args.next().map(|a| a.parse().unwrap()).unwrap_or(3);
    let max: usize = args.next().map(|a| a.parse().unwrap()).unwrap_or(usize::MAX);
    let data = std::fs::read(&path).unwrap();
    let (info, packets) = block_on(async {
        let mut d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let mut v = Vec::new();
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id == info.id && v.len() < max {
                v.push(p);
            }
        }
        (info, v)
    });
    let mut best = f64::MAX;
    let (mut frames, mut sum) = (0usize, 0u64);
    for _ in 0..repeat {
        let mut dec = hevc_decoder(&info).unwrap();
        frames = 0;
        sum = 0;
        let t = Instant::now();
        let take = |dec: &mut Box<dyn rvp_core::VideoDecoder>, frames: &mut usize, sum: &mut u64| {
            while let Ok(Some(f)) = dec.receive_frame() {
                *frames += 1;
                for p in &f.planes {
                    *sum = p.iter().step_by(97).fold(*sum, |a, b| a.wrapping_mul(31).wrapping_add(*b as u64));
                }
            }
        };
        for p in &packets {
            dec.send_packet(p).unwrap();
            take(&mut dec, &mut frames, &mut sum);
        }
        dec.drain().unwrap();
        take(&mut dec, &mut frames, &mut sum);
        best = best.min(t.elapsed().as_secs_f64());
    }
    println!(
        "{path}: {frames} frames, best {best:.2} s = {:.1} fps, checksum {sum:016x}",
        frames as f64 / best
    );
}
