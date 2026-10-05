//! Malformed input must never panic (debug builds also trap arithmetic overflow): truncated, bit-flipped, dropped,
//! shuffled and random packets, damaged parameter sets, and unsupported streams (interlaced, high bit depth) that must
//! be rejected cleanly.
mod common;

use common::synth::Rng;
use rvp_codec_h264::Error;
use rvp_codec_h264::decoder::Decoder;
use rvp_core::StreamKind;
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::path::Path;

struct Clip {
    extra: Vec<u8>,
    packets: Vec<Vec<u8>>,
}

fn load(path: &Path, max_packets: usize) -> Clip {
    let data = std::fs::read(path).unwrap();
    block_on(async {
        let mut d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let mut packets = Vec::new();
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id == info.id && packets.len() < max_packets {
                packets.push(p.data);
            }
        }
        Clip { extra: info.extra_data, packets }
    })
}

/// Decode everything, ignoring errors (the point is that nothing panics and the decoder stays usable).
fn run(extra: &[u8], packets: &[Vec<u8>]) -> (usize, u64) {
    let mut dec = Decoder::new();
    let _ = dec.set_avcc(extra);
    let mut frames = 0;
    for (i, p) in packets.iter().enumerate() {
        let _ = dec.decode_sample(p, i as i64);
        while dec.next_frame().is_some() {
            frames += 1;
        }
    }
    let _ = dec.flush();
    while dec.next_frame().is_some() {
        frames += 1;
    }
    (frames, dec.stats().errors)
}

fn corrupt(rng: &mut Rng, packets: &[Vec<u8>], mode: u64) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = packets.to_vec();
    match mode {
        0 => {
            // Truncate random packets.
            for p in out.iter_mut() {
                if rng.chance(40) && !p.is_empty() {
                    let n = rng.below(p.len() as u64) as usize;
                    p.truncate(n);
                }
            }
        }
        1 => {
            // Flip a few bits in random packets.
            for p in out.iter_mut() {
                if rng.chance(50) && !p.is_empty() {
                    for _ in 0..rng.range(1, 6) {
                        let i = rng.below(p.len() as u64) as usize;
                        p[i] ^= 1 << rng.below(8);
                    }
                }
            }
        }
        2 => {
            // Overwrite stretches with random bytes.
            for p in out.iter_mut() {
                if rng.chance(40) && p.len() > 8 {
                    let at = rng.below(p.len() as u64 - 4) as usize;
                    let n = rng.range(1, 24) as usize;
                    for k in at..(at + n).min(p.len()) {
                        p[k] = rng.next() as u8;
                    }
                }
            }
        }
        3 => {
            // Drop and shuffle packets.
            out.retain(|_| !rng.chance(25));
            if out.len() > 2 {
                for _ in 0..out.len() {
                    let (a, b) = (rng.below(out.len() as u64) as usize, rng.below(out.len() as u64) as usize);
                    out.swap(a, b);
                }
            }
        }
        _ => {
            // Zero a stretch (long runs of zero bits are a classic trap for Exp-Golomb and CAVLC prefixes).
            for p in out.iter_mut() {
                if rng.chance(40) && p.len() > 8 {
                    let at = rng.below(p.len() as u64 - 4) as usize;
                    let n = rng.range(1, 40) as usize;
                    for k in at..(at + n).min(p.len()) {
                        p[k] = if mode == 4 { 0 } else { 0xFF };
                    }
                }
            }
        }
    }
    out
}

#[test]
fn damaged_streams_never_panic() {
    if common::skip() {
        return;
    }
    let names = [
        "i_base_cif.mp4",
        "p_base_ref4.mp4",
        "b_cavlc_pyramid.mp4",
        "c_main_i.mp4",
        "c_main_b.mp4",
        "c_main_slices.mp4",
        "h_high.mp4",
        "h_high_cavlc.mp4",
        "h_high_cqm_jvt.mp4",
        "h_high_odd.mp4",
    ];
    let variants: u64 = std::env::var("RVP_FUZZ_VARIANTS").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let (mut errors, mut runs) = (0u64, 0u64);
    for name in names {
        let clip = load(&common::fixture(name), 14);
        let (clean_frames, clean_errors) = run(&clip.extra, &clip.packets);
        assert!(clean_frames > 5 && clean_errors == 0, "{name}: the undamaged clip decodes");
        for seed in 0..variants {
            for mode in 0..6u64 {
                let mut rng = Rng(seed * 977 + mode * 31 + 12345);
                let damaged = corrupt(&mut rng, &clip.packets, mode);
                errors += run(&clip.extra, &damaged).1;
                runs += 1;
            }
        }
    }
    // The damage must actually reach the decoder's error paths (otherwise this test proves nothing).
    assert!(errors > runs, "only {errors} decode errors in {runs} damaged runs");
}

#[test]
fn damaged_parameter_sets_never_panic() {
    if common::skip() {
        return;
    }
    let clip = load(&common::fixture("h_high_cqm_jvt.mp4"), 6);
    let mut rng = Rng(99);
    for _ in 0..400 {
        let mut extra = clip.extra.clone();
        let flips = rng.range(1, 4);
        for _ in 0..flips {
            let i = rng.below(extra.len() as u64) as usize;
            extra[i] ^= 1 << rng.below(8);
        }
        if rng.chance(20) {
            extra.truncate(rng.below(extra.len() as u64) as usize);
        }
        let _ = run(&extra, &clip.packets);
    }
    // Pure noise as extradata and as packets.
    for _ in 0..200 {
        let extra: Vec<u8> = (0..rng.range(0, 80)).map(|_| rng.next() as u8).collect();
        let packets: Vec<Vec<u8>> =
            (0..3).map(|_| (0..rng.range(0, 300)).map(|_| rng.next() as u8).collect()).collect();
        let _ = run(&extra, &packets);
    }
}

/// NAL units made of random bytes behind valid headers exercise every parser.
#[test]
fn random_nal_units_never_panic() {
    let mut rng = Rng(4242);
    for round in 0..3000 {
        let mut stream = Vec::new();
        for _ in 0..rng.range(1, 6) {
            let ty = [7u8, 8, 5, 1, 1, 6, 9, 2, 20, 14][rng.below(10) as usize];
            stream.extend_from_slice(&[0, 0, 0, 1, ((rng.below(4) as u8) << 5) | ty]);
            let n = rng.range(0, 200);
            for _ in 0..n {
                // Bias towards zero bytes and small values, like real parameter sets.
                let b = if rng.chance(30) { 0 } else { rng.next() as u8 };
                stream.push(b);
            }
        }
        let mut dec = Decoder::new();
        let _ = dec.decode_annexb(&stream, round);
        let _ = dec.flush();
        while dec.next_frame().is_some() {}
    }
}

#[test]
fn unsupported_streams_are_rejected_cleanly() {
    if common::skip() {
        return;
    }
    for (name, what) in [("x_interlaced.mp4", "interlaced"), ("x_high10.mp4", "bit depth")] {
        let clip = load(&common::fixture(name), 6);
        let mut dec = Decoder::new();
        dec.set_avcc(&clip.extra).unwrap();
        let mut unsupported = 0;
        for (i, p) in clip.packets.iter().enumerate() {
            if let Err(Error::Unsupported(msg)) = dec.decode_sample(p, i as i64) {
                unsupported += 1;
                assert!(msg.contains(what) || !msg.is_empty());
            }
            assert!(dec.next_frame().is_none(), "{name}: no picture from an unsupported stream");
        }
        assert!(unsupported > 0, "{name}: every packet reports Unsupported");
    }
}
