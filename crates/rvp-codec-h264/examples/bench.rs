//! Decode an MP4 or MKV file with the H.264 decoder and report speed: `cargo run --release -p rvp-codec-h264
//! --example bench -- file.mp4 [repeat]`. Prints the MD5-free checksum of the output so runs can be compared.
use rvp_codec_h264::decoder::recon::{ReconEvent, ReconExecutor, Reconstructor};
use rvp_codec_h264::decoder::{Decoder, Frame};
use rvp_core::StreamKind;
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

/// Runs reconstruction inline but times it, so the cost of parsing and of reconstruction can be told apart
/// (`bench file.mp4 3 stages`): the slower of the two bounds the speed of a decoder with a second thread.
struct Timed {
    rec: Reconstructor,
    out: VecDeque<Frame>,
    spent: Rc<Cell<f64>>,
}

impl ReconExecutor for Timed {
    fn submit(&mut self, ev: ReconEvent) {
        let t = Instant::now();
        self.rec.handle(ev, &mut self.out);
        self.spent.set(self.spent.get() + t.elapsed().as_secs_f64());
    }
    fn take_frames(&mut self, out: &mut VecDeque<Frame>) {
        out.append(&mut self.out);
    }
    fn pending(&self) -> usize {
        0
    }
    fn wait_idle(&mut self) {}
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: bench <file> [repeat]");
    let repeat: usize = args.next().map(|a| a.parse().unwrap()).unwrap_or(3);
    let stages = args.next().as_deref() == Some("stages");
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
    let (mut best_recon, mut best_parse) = (f64::MAX, f64::MAX);
    let mut frames = 0;
    let mut sum = 0u64;
    for _ in 0..repeat {
        let mut dec = Decoder::new();
        let spent = Rc::new(Cell::new(0.0));
        if stages {
            dec.set_recon_executor(Box::new(Timed {
                rec: Reconstructor::new(),
                out: VecDeque::new(),
                spent: spent.clone(),
            }));
        }
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
        let total = t.elapsed().as_secs_f64();
        best = best.min(total);
        if stages {
            best_recon = best_recon.min(spent.get());
            best_parse = best_parse.min(total - spent.get());
        }
    }
    if stages {
        println!(
            "parse {:.2} ms/frame, reconstruct (prediction, residual, deblocking) {:.2} ms/frame",
            best_parse * 1000.0 / frames as f64,
            best_recon * 1000.0 / frames as f64
        );
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
