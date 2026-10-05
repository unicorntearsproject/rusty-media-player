//! Writes one picture of every effect as a PPM into the directory given as the first argument (default `/tmp/rvp-viz`), driven
//! by a synthetic beat: `cargo run -p rvp-viz --example frames -- /tmp/rvp-viz`.
use rvp_host::{VIZ_BANDS, VizSummary};
use rvp_viz::{EFFECTS, FrameInput, Palette, Viz};

fn summary(pts: i64, t: f32) -> VizSummary {
    let mut bands = [0.0f32; VIZ_BANDS];
    for (i, b) in bands.iter_mut().enumerate() {
        let x = i as f32 / VIZ_BANDS as f32;
        *b = ((0.8 - x * 0.7) * (0.6 + 0.4 * (t * 3.0 + i as f32 * 0.7).sin())).clamp(0.0, 1.0);
    }
    let beat = (t * 2.0).fract() < 0.1;
    VizSummary {
        pts_us: pts,
        level: 0.35,
        peak: 0.6,
        bands,
        bass: 0.7,
        mid: 0.5,
        treble: 0.3,
        onset: beat,
        onset_strength: 1.0,
        tempo_bpm: 120.0,
    }
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/rvp-viz".into());
    std::fs::create_dir_all(&dir).unwrap();
    let scope: Vec<f32> =
        (0..2048).map(|i| ((i as f32 * 0.09).sin() * 0.6 + (i as f32 * 0.31).sin() * 0.2) * 0.9).collect();
    for effect in EFFECTS {
        let mut v = Viz::new();
        v.effect = effect;
        v.palette = Palette::Tears;
        let mut now = 0i64;
        for f in 0..90 {
            let t = f as f32 / 30.0;
            for k in 0..3 {
                v.feed(&summary(now + k * 10_700, t), false);
            }
            v.render(
                1280,
                720,
                &FrameInput { now_us: now, playing: true, reduce_motion: false, scope: &scope },
            );
            now += 33_000;
        }
        let (px, w, h) = v.picture();
        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
        for p in px.chunks_exact(4) {
            out.extend_from_slice(&p[..3]);
        }
        std::fs::write(format!("{dir}/{}.ppm", effect.name().to_lowercase()), out).unwrap();
    }
}
