//! The H.264 decoder with its reconstruction on a second thread, and on its own decoder thread, produces exactly the
//! frames of the inline decoder, also across flushes (seeks).
use rvp_core::task::block_on;
use rvp_core::{Packet, StreamInfo, StreamKind, VideoDecoder, VideoFrame};
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use rvp_par::{ThreadedVideoDecoder, h264::h264_pipelined};
use std::path::Path;

fn fixture(name: &str) -> Option<Vec<u8>> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures").join(name);
    std::fs::read(p).ok()
}

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

/// Feed `pk` and collect every frame, waiting for a decoder with threads to finish.
fn run(dec: &mut dyn VideoDecoder, pk: &[Packet]) -> Vec<VideoFrame> {
    let mut out = Vec::new();
    let take = |dec: &mut dyn VideoDecoder, out: &mut Vec<VideoFrame>| {
        while let Some(f) = dec.receive_frame().unwrap() {
            out.push(f);
        }
    };
    for p in pk {
        dec.send_packet(p).unwrap();
        take(dec, &mut out);
        // Do not flood a threaded decoder: it has its own queue limits, but keep the test's memory small.
        while dec.pending() > 8 {
            std::thread::sleep(std::time::Duration::from_micros(200));
            take(dec, &mut out);
        }
    }
    dec.drain().unwrap();
    while dec.pending() > 0 {
        std::thread::sleep(std::time::Duration::from_micros(200));
        take(dec, &mut out);
    }
    take(dec, &mut out);
    out
}

fn same(a: &[VideoFrame], b: &[VideoFrame]) {
    assert_eq!(a.len(), b.len(), "frame counts differ");
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        assert_eq!((x.pts, x.width, x.height), (y.pts, y.width, y.height), "frame {i}");
        assert!(x.planes == y.planes, "frame {i}: samples differ");
    }
}

const FILES: &[&str] = &[
    "h264/c_main_b.mp4",
    "h264/b_cavlc_pyramid.mp4",
    "h264/h_high_cqm_jvt.mp4",
    "h264/h_high_slices.mp4",
    "h264/h_high_odd.mp4",
];

/// The big stream is only worth its time in an optimised build.
const BIG: &str = "h264/h_high_1080p.mp4";

#[test]
fn pipelined_equals_inline() {
    let mut ran = 0;
    let big = (!cfg!(debug_assertions)).then_some(BIG);
    for name in FILES.iter().copied().chain(big) {
        let Some(file) = fixture(name) else { continue };
        ran += 1;
        let (info, pk) = packets(file);
        let mut inline = rvp_codec_h264::h264_decoder(&info).unwrap();
        let want = run(&mut *inline, &pk);
        assert!(!want.is_empty(), "{name}");
        // Reconstruction on a second thread, driven from this thread.
        let mut two = h264_pipelined(&info).unwrap();
        same(&want, &run(&mut *two, &pk));
        // And the whole decoder on its own thread as the player uses it.
        let info2 = info.clone();
        let mut three = ThreadedVideoDecoder::new(Box::new(move || h264_pipelined(&info2)));
        same(&want, &run(&mut three, &pk));
        eprintln!("{name}: {} frames identical", want.len());
    }
    if ran == 0 {
        eprintln!("no fixtures: run `cargo xtask fixtures`");
    }
}

#[test]
fn flush_and_restart_in_the_middle() {
    let Some(file) = fixture("h264/h_high_mbtree.mp4") else { return };
    let (info, pk) = packets(file);
    // Decode the first 25 packets, flush, decode from a keyframe again: the second run must equal a fresh decode.
    let mut dec = ThreadedVideoDecoder::new({
        let info = info.clone();
        Box::new(move || h264_pipelined(&info))
    });
    for p in &pk[..25] {
        dec.send_packet(p).unwrap();
    }
    dec.flush();
    let again = run(&mut dec, &pk);
    let mut fresh = rvp_codec_h264::h264_decoder(&info).unwrap();
    same(&run(&mut *fresh, &pk), &again);
}
