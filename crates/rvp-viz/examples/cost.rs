//! What each effect costs per frame on this machine, at the window sizes people use: `cargo run --release -p rvp-viz --example cost`.
//! (Release only: a debug build says nothing.) Prints milliseconds per frame, average and worst, over a few hundred frames of a synthetic
//! beat, for 1920x1080 and 1280x720 windows; the player's budget is a frame (33 ms at 30 fps), of which the effect should take a small part.
use rvp_host::{VIZ_BANDS, VizSummary};
use rvp_viz::{EFFECTS, FrameInput, Palette, Viz};
use std::time::Instant;

fn summary(pts: i64, t: f32) -> VizSummary {
    let mut bands = [0.0f32; VIZ_BANDS];
    for (i, b) in bands.iter_mut().enumerate() {
        let x = i as f32 / VIZ_BANDS as f32;
        *b = ((0.8 - x * 0.7) * (0.6 + 0.4 * (t * 3.0 + i as f32 * 0.7).sin())).clamp(0.0, 1.0);
    }
    VizSummary {
        pts_us: pts,
        level: 0.35,
        peak: 0.6,
        bands,
        bass: 0.7,
        mid: 0.5,
        treble: 0.4,
        onset: (t * 2.0).fract() < 0.05,
        onset_strength: 1.0,
        tempo_bpm: 120.0,
    }
}

fn main() {
    let scope: Vec<f32> = (0..2048).map(|i| ((i as f32 * 0.09).sin() * 0.6) * 0.9).collect();
    for (w, h) in [(1920usize, 1080usize), (1280, 720)] {
        println!("{w}x{h}");
        for effect in EFFECTS {
            let mut v = Viz::new();
            v.effect = effect;
            v.palette = Palette::Tears;
            let (mut now, mut total, mut worst) = (0i64, 0.0f64, 0.0f64);
            let frames = 240;
            for f in 0..frames {
                let t = f as f32 / 30.0;
                for k in 0..3 {
                    v.feed(&summary(now + k * 10_700, t), false);
                }
                let s = Instant::now();
                v.render(
                    w,
                    h,
                    &FrameInput { now_us: now, playing: true, reduce_motion: false, scope: &scope },
                );
                let ms = s.elapsed().as_secs_f64() * 1000.0;
                if f >= 10 {
                    total += ms;
                    worst = worst.max(ms);
                }
                now += 33_000;
            }
            let (_, bw, bh) = v.picture();
            println!(
                "  {:<14} {:>6.2} ms avg  {:>6.2} ms worst   ({bw}x{bh})",
                effect.name(),
                total / (frames - 10) as f64,
                worst
            );
        }
    }
}
