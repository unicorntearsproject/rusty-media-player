//! Decode a WebM or MKV file with the AV1 decoder and report speed, then convert the pictures to RGBA the way the player does:
//! `cargo run --release -p rvp-codec-av1 --example av1_bench -- file.webm [repeat]`. Pin one core (`taskset -c 3`) for
//! single-thread numbers. Prints a checksum of every decoded frame, so a change that must not alter the output can be checked.
use rvp_core::StreamKind;
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::time::Instant;

/// A quick order-sensitive hash over 8-byte words (the planes are megabytes: byte-wise FNV would cost as much as the decode).
fn fnv(h: &mut u64, b: &[u8]) {
    let mut it = b.chunks_exact(8);
    for c in &mut it {
        *h = (h.rotate_left(5) ^ u64::from_le_bytes(c.try_into().unwrap()))
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    for &x in it.remainder() {
        *h = (*h ^ x as u64).wrapping_mul(0x100_0000_01b3);
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: av1_bench <file> [repeat]");
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
    let (mut best, mut best_conv, mut frames, mut sum) = (f64::MAX, f64::MAX, 0, 0u64);
    for _ in 0..repeat {
        let mut dec = rvp_codec_av1::av1_decoder(&info).unwrap();
        frames = 0;
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut conv = 0.0;
        let mut out: Vec<u8> = Vec::new();
        let t = Instant::now();
        for p in &packets {
            dec.send_packet(p).unwrap();
            while let Some(f) = dec.receive_frame().unwrap() {
                frames += 1;
                for pl in &f.planes {
                    fnv(&mut h, pl);
                }
                let c = Instant::now();
                out.resize(f.width as usize * f.height as usize * 4, 0);
                rvp_core::color::yuv420_to_rgba(&f, &mut out);
                conv += c.elapsed().as_secs_f64();
                fnv(&mut h, &out[..64]);
            }
        }
        let total = t.elapsed().as_secs_f64();
        best = best.min(total - conv);
        best_conv = best_conv.min(conv);
        sum = h;
    }
    println!(
        "{path}: {frames} frames, decode {:.2} ms/frame ({:.0} fps), convert to RGBA {:.2} ms/frame, checksum {sum:016x}",
        best * 1e3 / frames as f64,
        frames as f64 / best,
        best_conv * 1e3 / frames as f64
    );
}
