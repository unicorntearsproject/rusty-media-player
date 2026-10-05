//! Malformed input: truncation, bit flips, garbage, shuffled and dropped packets. The decoder must never fail
//! the test process, and we count how often `rusty_vp9` had to contain a panic with `catch_unwind` (it cannot on
//! wasm32, where panics abort), so a regression in the up-front checks shows up here.
mod common;
use common::*;
use rvp_core::Packet;
use std::sync::atomic::{AtomicUsize, Ordering};

static PANICS: AtomicUsize = AtomicUsize::new(0);

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn run(info: &rvp_core::StreamInfo, packets: &[Packet]) {
    let mut dec = rvp_codec_vp9::vp9_decoder(info).unwrap();
    for p in packets {
        let _ = dec.send_packet(p);
        while let Ok(Some(_)) = dec.receive_frame() {}
    }
    let _ = dec.drain();
}

#[test]
fn malformed_streams_never_panic_through_the_wrapper() {
    if skip() {
        return;
    }
    std::panic::set_hook(Box::new(|_| {
        PANICS.fetch_add(1, Ordering::Relaxed);
    }));
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut total = 0usize;
    for name in [
        "s_64x64.webm",
        "s_176x144.webm",
        "t_altref.webm",
        "t_aq_cyclic.webm",
        "t_lossless.webm",
        "r_keyframe.webm",
    ] {
        let (info, packets) = read_packets(&fixture(name));
        let rounds: usize = std::env::var("RVP_FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
        for round in 0..rounds {
            let mut p: Vec<Packet> = packets.clone();
            match round % 6 {
                0 => {
                    // Truncate random packets.
                    for _ in 0..4 {
                        let i = rng.below(p.len());
                        let n = rng.below(p[i].data.len());
                        p[i].data.truncate(n);
                    }
                }
                1 => {
                    // Flip bits in random packets.
                    for _ in 0..6 {
                        let i = rng.below(p.len());
                        if !p[i].data.is_empty() {
                            let b = rng.below(p[i].data.len());
                            p[i].data[b] ^= 1 << rng.below(8);
                        }
                    }
                }
                2 => {
                    // Flip bits in the headers only (first 24 bytes), where sizes and modes live.
                    for _ in 0..8 {
                        let i = rng.below(p.len());
                        let n = p[i].data.len().min(24);
                        if n > 0 {
                            let b = rng.below(n);
                            p[i].data[b] ^= 1 << rng.below(8);
                        }
                    }
                }
                3 => {
                    // Replace a packet with garbage.
                    let i = rng.below(p.len());
                    let n = 1 + rng.below(400);
                    p[i].data = (0..n).map(|_| rng.next() as u8).collect();
                }
                4 => {
                    // Drop and swap packets (missing references).
                    let i = rng.below(p.len());
                    p.remove(i);
                    let (a, b) = (rng.below(p.len()), rng.below(p.len()));
                    p.swap(a, b);
                }
                _ => {
                    // Start mid-stream (no key frame first).
                    let i = 1 + rng.below(p.len() - 1);
                    p.drain(..i);
                }
            }
            run(&info, &p);
            total += 1;
        }
    }
    let _ = std::panic::take_hook();
    let caught = PANICS.load(Ordering::Relaxed);
    eprintln!("{total} mutated streams, {caught} contained panics");
    assert_eq!(
        caught, 0,
        "rusty_vp9 had to catch a panic; add a check to the wrapper (panics abort on wasm32)"
    );
}

#[test]
fn oversized_pictures_are_refused_without_allocating() {
    if skip() {
        return;
    }
    // A key frame header claiming 65536 x 65536: profile 0, key frame, show_frame, sync code, BT.601, size.
    let mut bits: Vec<bool> = Vec::new();
    let mut put = |v: u32, n: usize| (0..n).rev().for_each(|i| bits.push(v >> i & 1 == 1));
    put(2, 2); // marker
    put(0, 2); // profile 0
    put(0, 1); // show_existing_frame
    put(0, 1); // key frame
    put(1, 1); // show_frame
    put(0, 1); // error_resilient
    put(0x49_8342, 24);
    put(2, 3); // colour space BT.709
    put(0, 1); // range
    put(0xffff, 16);
    put(0xffff, 16);
    let mut data = vec![0u8; bits.len().div_ceil(8) + 32];
    for (i, b) in bits.iter().enumerate() {
        data[i / 8] |= (*b as u8) << (7 - i % 8);
    }
    let info = rvp_core::StreamInfo {
        id: 1,
        kind: rvp_core::StreamKind::Video,
        codec: "vp9".into(),
        ..stream_info_template()
    };
    let mut dec = rvp_codec_vp9::vp9_decoder(&info).unwrap();
    let pkt = Packet { stream_id: 1, pts: 0, dts: 0, duration: 0, keyframe: true, discard_end_us: 0, data };
    assert!(matches!(dec.send_packet(&pkt), Err(rvp_core::Error::Unsupported(_))));
}

fn stream_info_template() -> rvp_core::StreamInfo {
    // Any real stream's info will do as a template.
    read_packets(&fixture("s_64x64.webm")).0
}
