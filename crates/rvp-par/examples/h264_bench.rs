//! Speed of the pipelined H.264 decoder: `cargo run --release -p rvp-par --example h264_bench -- file.mp4 [parse threads...]`.
//! Prints the decode time per frame with the decoder, the reconstruction and the given number of parse threads each on
//! its own thread.
use rvp_core::task::block_on;
use rvp_core::{Packet, StreamInfo, StreamKind};
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use rvp_par::h264::h264_pipelined_with;
use std::time::{Duration, Instant};

fn packets(file: Vec<u8>) -> (StreamInfo, Vec<Packet>) {
    block_on(async {
        let mut d = open(MemSource::new(file)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let mut v = Vec::new();
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id == info.id {
                v.push(p);
            }
        }
        (info, v)
    })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: h264_bench <file> [parse threads...]");
    let (info, pk) = packets(std::fs::read(&path).unwrap());
    // RVP_POOL=n: a pool of n threads for the kernels (reconstruction bands, deblocking).
    if let Some(n) = std::env::var("RVP_POOL").ok().and_then(|v| v.parse().ok()) {
        rvp_par::Pool::new(n).install();
    }
    let ks: Vec<usize> = args.map(|a| a.parse().unwrap()).collect();
    for k in if ks.is_empty() { vec![0, 1, 2, 3] } else { ks } {
        let mut best = f64::MAX;
        let mut frames = 0;
        for _ in 0..3 {
            let mut dec = h264_pipelined_with(&info, k).unwrap();
            let t = Instant::now();
            frames = 0;
            for p in &pk {
                dec.send_packet(p).unwrap();
                while dec.receive_frame().unwrap().is_some() {
                    frames += 1;
                }
            }
            dec.drain().unwrap();
            while dec.pending() > 0 {
                std::thread::sleep(Duration::from_micros(100));
                while dec.receive_frame().unwrap().is_some() {
                    frames += 1;
                }
            }
            while dec.receive_frame().unwrap().is_some() {
                frames += 1;
            }
            best = best.min(t.elapsed().as_secs_f64());
        }
        println!(
            "{path}: {k} parse threads: {frames} frames, {:.2} ms/frame, {:.1} fps",
            best * 1000.0 / frames as f64,
            frames as f64 / best
        );
    }
}
