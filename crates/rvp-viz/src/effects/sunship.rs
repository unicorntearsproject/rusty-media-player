//! **Sun Ship 3000**: a cosmic space battle where enemies and lasers dance to the beat, ported in spirit from Unicorn Viz's effect of the same name
//! (MIT; its GPU shader is not used, this is written from scratch for the CPU). Four rows of colour-cycling enemy discs sweep across in sine
//! formations driven by the mids and fire laser streams down, a player ship at the bottom is pushed from side to side by the bass, its counter
//! volley (a stream of lasers going up) fires on the beat, and each beat sets off an expanding explosion ring with a warm bloom.
//!
//! Audio: bass moves the ship, mids shape the formations and the enemy fire, treble sets the spark and the laser density, beats fire the volley and the
//! explosion. As for every effect the picture never flashes: the explosion's glow is local and the whole-picture lift is the shared 12 percent pulse;
//! in calm mode there are no explosions or volleys, only the slow, quiet battle.
use super::Viz;
use alloc::vec::Vec;
use libm::{cosf, floorf, sinf};

const TAU: f32 = core::f32::consts::TAU;

/// What the battle remembers between frames.
#[derive(Debug, Default)]
pub(super) struct Ship {
    /// Seconds of battle (calm mode is slow).
    t: f32,
    /// Phases integrated from the audio on this side, so a loud passage never makes the formations jump.
    mid_phase: f32,
    treb_phase: f32,
    expl_phase: f32,
    /// The explosion envelope: 1 on the beat, decaying.
    expl: f32,
    /// Rolled once per run: shifts the enemies' hues and where the formation starts.
    seed: f32,
    seeded: bool,
    /// Vignette per pixel (0..256) for the current picture size, and the tone curve over 0..2 in 1/256ths.
    vig: Vec<u16>,
    tone: Vec<u8>,
}

fn fract(x: f32) -> f32 {
    x - floorf(x)
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Viz {
    pub(super) fn draw_sun_ship(&mut self, dt: f32, moving: bool, calm: f32, reduce_motion: bool) {
        let fresh = self.pulse > 0.0 && self.pulse > 0.95 * self.pulse_set;
        {
            let seed = (self.rand() * 100.0) + 1.0;
            let st = &mut self.scenes.ship;
            if !st.seeded {
                st.seeded = true;
                st.seed = seed;
                st.mid_phase = seed * 0.37;
                st.treb_phase = seed * 0.61;
            }
        }
        let (mid, treble, bass) = (self.mid, self.treble, self.bass);
        if moving {
            let st = &mut self.scenes.ship;
            st.t += dt * calm;
            st.expl = (st.expl - dt * 1.8).max(0.0);
            if fresh && !reduce_motion {
                st.expl = 1.0;
            }
            if !reduce_motion {
                st.expl = st.expl.max(bass * 0.25);
            }
            st.mid_phase = (st.mid_phase + dt * calm * 0.6 * mid) % 1000.0;
            st.treb_phase = (st.treb_phase + dt * calm * 0.6 * treble) % 1000.0;
            st.expl_phase = (st.expl_phase + dt * calm * 0.6 * st.expl) % 1000.0;
        }
        let (t, mp, tp, ep, expl, seed) = {
            let s = &self.scenes.ship;
            (s.t * 0.6, s.mid_phase, s.treb_phase, s.expl_phase, s.expl, s.seed)
        };
        let g = self.gain();
        // Deep-space backdrop with the faintest blue.
        for p in self.buf.chunks_exact_mut(4) {
            p[0] = 1;
            p[1] = 3;
            p[2] = 6;
            p[3] = 255;
        }
        let (bw, bh) = (self.bw as f32, self.bh as f32);
        let (cx, cy) = (bw * 0.5, bh * 0.5);
        // Height units to pixels: x right, y up.
        let px = |x: f32| cx + x * bh;
        let py = |y: f32| cy - y * bh;
        let enemy_r = bh * 0.04;
        let wave_t = t * 0.3 + mp * 0.5;
        let hue_lead = (t * 0.1 + tp * 0.2 + seed * 0.07) % 1.0;
        // The enemy formation: 24 discs in four rows of six, undulating.
        for i in 0..24usize {
            let fi = i as f32;
            let row = floorf(fi / 6.0);
            let col_idx = fi - row * 6.0;
            let x = col_idx / 6.0 - 0.5 + sinf(row * 0.5 + wave_t) * (0.15 + mid * 0.2);
            let y = 0.4 - row * 0.08 + cosf(wave_t * 0.7 + fi * 0.2) * (0.05 + mid * 0.1);
            let hue = (hue_lead + fi * 0.1) % 1.0;
            let c = self.col((hue * 255.0) as usize);
            let k = (0.7 + mid * 1.2) * g;
            let (ex, ey) = (px(x), py(y));
            self.disc_add(ex, ey, enemy_r, c, k);
            // The spark bloom in the middle, brighter with the treble.
            self.disc_add(ex, ey, enemy_r * 0.55, [255, 255, 255], (0.35 + treble * 0.7) * g);
        }
        // Enemy fire: laser streams down the screen, their brightness a flicker driven by the mids.
        let fire_w = (bh * 0.006).max(1.0);
        let fire_gain = (0.6 + mid) * (0.7 + treble * 0.7) * g;
        for f in 0..16usize {
            let ff = f as f32;
            let fx = cosf(ff * 0.4) * 0.3;
            let fy = fract(t * 0.5 + mp * 1.2 + ff * 0.1) - 0.5;
            let k = smooth(0.0, -0.5, fy) * fire_gain * 0.55;
            if k > 0.02 {
                self.vline(px(fx), fire_w, [255, 26, 77], k);
            }
        }
        // The player's ship: pushed sideways by the bass.
        let ship_x = sinf(t * 0.1) * 0.1 + (bass - 0.5) * 0.3;
        let ship_y = -0.42;
        let (sx, sy) = (px(ship_x), py(ship_y));
        let (hw, hh) = (bh * 0.035, bh * 0.07);
        self.rect_add(
            (sx - hw) as i32,
            (sy - hh) as i32,
            (sx + hw) as i32,
            (sy + hh) as i32,
            [51, 255, 230],
            0.9 * g,
        );
        self.disc_add(sx, sy, hh * 1.8, [77, 204, 255], 0.5 * g);
        // The counter-volley: eight lasers from the ship on the beat.
        if expl > 0.01 {
            let laser_gain = (0.7 + treble * 0.7) * g * expl.min(1.0);
            for l in 0..8usize {
                let fl = l as f32;
                let lx = ship_x + (fl - 4.0) * 0.02;
                let ly = fract(t * 1.5 + ep * 2.0 - fl * 0.08);
                let k = smooth(0.0, 0.5, ly) * laser_gain * 0.6;
                self.vline(px(lx), (bh * 0.004).max(1.0), [51, 255, 102], k);
            }
        }
        // The explosion: a warm bloom that swells out from the middle with an edge ring. It is local light, kept soft; the whole picture only
        // ever lifts by the shared pulse (12 percent).
        if expl > 0.01 {
            let r = (0.5 + expl * 0.6) * bh;
            self.glow_blocks(cx, cy, r, [255, 153, 26], 0.22 * expl * g, 3);
            self.ring_add(
                cx,
                cy,
                expl_ring(expl) * bh,
                1.5,
                [255, 204, 51].map(|c| (c as f32 * expl * g).min(255.0) as u8),
            );
        }
        // Vignette and tone, then the shared beat lift (tables: nothing is computed per pixel but a multiply and a lookup).
        let (w, h) = (self.bw, self.bh);
        if self.scenes.ship.vig.len() != w * h {
            let mut vig = Vec::with_capacity(w * h);
            for y in 0..h {
                let dy = y as f32 / bh - 0.5;
                for x in 0..w {
                    let dx = x as f32 / bw - 0.5;
                    let v = 1.0 - smooth(0.25, 1.4, libm::sqrtf(dx * dx + dy * dy));
                    vig.push(((0.35 + 0.65 * v) * 256.0) as u16);
                }
            }
            self.scenes.ship.vig = vig;
        }
        if self.scenes.ship.tone.is_empty() {
            self.scenes.ship.tone = (0..512)
                .map(|i| {
                    let c = i as f32 / 256.0;
                    (c / (1.0 + c * 0.5) * 255.0).min(255.0) as u8
                })
                .collect();
        }
        let lift = ((1.0 + 0.12 * self.pulse) * 256.0) as u32;
        let vig = core::mem::take(&mut self.scenes.ship.vig);
        let tone = core::mem::take(&mut self.scenes.ship.tone);
        for (p, v) in self.buf.chunks_exact_mut(4).zip(vig.iter()) {
            let m = *v as u32 * lift; // 16 fractional bits
            for c in &mut p[..3] {
                let q = ((*c as u32 * m) >> 16) as usize; // 0..~287, in 1/256ths of 1.0 scaled by 255/256
                *c = tone[(q * 256 / 255).min(511)];
            }
        }
        self.scenes.ship.vig = vig;
        self.scenes.ship.tone = tone;
        let _ = TAU;
    }

    /// A vertical line of light across the whole picture at pixel column `x`, `w` wide (additive).
    fn vline(&mut self, x: f32, w: f32, c: [u8; 3], k: f32) {
        let half = (w * 0.5) as i32 + 1;
        for ox in -half..=half {
            let f = 1.0 - (ox as f32).abs() / (half as f32 + 1.0);
            for y in 0..self.bh as i32 {
                self.add(x as i32 + ox, y, c, k * f);
            }
        }
    }

    /// A soft glow of radius `r` drawn in `step` x `step` pixel blocks (cheap for big, smooth things).
    fn glow_blocks(&mut self, cx: f32, cy: f32, r: f32, c: [u8; 3], k: f32, step: i32) {
        let inv = 1.0 / (r * r);
        let (x0, x1) = (((cx - r) as i32).max(0), ((cx + r) as i32 + 1).min(self.bw as i32 - 1));
        let (y0, y1) = (((cy - r) as i32).max(0), ((cy + r) as i32 + 1).min(self.bh as i32 - 1));
        let mut y = y0;
        while y <= y1 {
            let dy = y as f32 + step as f32 * 0.5 - cy;
            let mut x = x0;
            while x <= x1 {
                let dx = x as f32 + step as f32 * 0.5 - cx;
                let f = 1.0 - (dx * dx + dy * dy) * inv;
                if f > 0.0 {
                    let a = k * f * f;
                    for oy in 0..step {
                        for ox in 0..step {
                            self.add(x + ox, y + oy, c, a);
                        }
                    }
                }
                x += step;
            }
            y += step;
        }
    }
}

/// Radius of the explosion's edge ring, in picture heights: out from the middle as the envelope falls.
fn expl_ring(expl: f32) -> f32 {
    (1.0 - expl) * 0.55 + 0.05
}
