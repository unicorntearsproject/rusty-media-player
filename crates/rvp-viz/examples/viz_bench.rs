//! Time the analyzer and the WSOLA time stretcher on 60 s of stereo audio: `cargo run --release -p rvp-viz --example viz_bench`.
use rvp_core::TimeStretcher;
use rvp_viz::Analyzer;
use std::time::Instant;

fn main() {
    let sr = 48_000;
    let secs = 60;
    let mut x = Vec::with_capacity(sr * secs * 2);
    let mut seed = 1u32;
    for i in 0..sr * secs {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let n = (seed >> 16) as f32 / 32768.0 - 1.0;
        let s = 0.3 * (i as f32 * 0.05).sin() + 0.1 * n;
        x.push(s);
        x.push(s);
    }
    let t = Instant::now();
    let mut a = Analyzer::new(sr as u32);
    let mut out = Vec::new();
    for c in x.chunks(2 * 800) {
        a.process(c, 2, 0, &mut out);
    }
    let el = t.elapsed().as_secs_f64();
    println!(
        "analyzer: {} summaries, {:.1} ms for {secs} s of audio ({:.2}% of one core)",
        out.len(),
        el * 1e3,
        el / secs as f64 * 100.0
    );
    for rate in [0.5, 1.5, 2.0] {
        let t = Instant::now();
        let mut st = TimeStretcher::new(sr as u32, 2, rate);
        let mut o = Vec::new();
        for c in x.chunks(2 * 800) {
            st.process(c, &mut o);
        }
        let el = t.elapsed().as_secs_f64();
        println!(
            "wsola {rate}x: {:.1} ms for {secs} s of audio ({:.2}% of one core)",
            el * 1e3,
            el / secs as f64 * 100.0
        );
    }
}
