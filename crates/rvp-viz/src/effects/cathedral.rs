//! **Bass Cathedral**: a flight down an endless nave, ported in spirit from Unicorn Viz's *Cathedral of Bass* (MIT; its GPU shader is not used,
//! this is written from scratch for the CPU). A corridor of angular panels and longitudinal bricks, bright arch pillars that flex with the bass, a
//! core light at the vanishing point that surges with the low end, godray streaks that sparkle with the treble, a radial chromatic fringe, and
//! on the beat a camera recoil and an expanding shockwave ring.
//!
//! Everything that depends only on the pixel (its distance and angle from the middle, the fog, the glow, the vignette) is computed once when
//! the picture size changes; a frame is then a table-driven pass of a few multiplies per pixel and channel, so it stays cheap in a browser.
//! The effect draws at a fifth of the window size (it is all soft light). Motion safety as for every effect: the beat brightens by at most the
//! shared pulse (12 percent, three a second), the recoil is a few pixels, and in calm mode there is no recoil and no ring, only a slow drift.
use super::Viz;
use alloc::vec::Vec;
use libm::{atan2f, expf, powf, sqrtf};

const TAU: f32 = core::f32::consts::TAU;

/// What one pixel of the nave knows before any audio arrives.
#[derive(Clone, Copy, Default)]
struct Px {
    /// 1 / distance from the middle (depth along the nave), in units of half the picture height.
    inv: f32,
    /// The angle around the middle, 0..256.
    ang: f32,
    /// Fog, 0..1: dark far away, clear at the rim.
    fog: f32,
    /// The core glow's falloff.
    core: f32,
    /// Fades the pillars out towards the middle and the rim.
    rib: f32,
    /// Where the godray streaks show.
    ray: f32,
    /// Vignette, 0.5..1.
    vig: f32,
    /// The distance, in units of the shockwave ring's sine table.
    r14: f32,
    /// Where the shockwave shows (strongest near the middle).
    shock: f32,
}

/// What the nave remembers between frames.
#[derive(Debug, Default)]
pub(super) struct Cath {
    map: Vec<Px>,
    /// Tone-mapping table: light in 1/256ths, 0..4, to a byte.
    tone: Vec<u8>,
    /// Seconds of flight (calm mode flies slowly).
    t: f32,
    /// The beat's recoil, 1 on the hit, decaying.
    shake: f32,
    /// Mean brightness of the last frame (the beat may not change it by more than the flash limit).
    mean: f32,
}

impl core::fmt::Debug for Px {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Px")
    }
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Viz {
    /// Make the per-pixel tables for the current picture size.
    pub(super) fn build_cathedral(&mut self) {
        let (bw, bh) = (self.bw, self.bh);
        let (cx, cy) = (bw as f32 * 0.5, bh as f32 * 0.5);
        let scale = bh as f32 * 0.5;
        let c = &mut self.scenes.cath;
        c.map.clear();
        c.map.reserve(bw * bh);
        for y in 0..bh {
            for x in 0..bw {
                let (dx, dy) = ((x as f32 + 0.5 - cx) / scale, (y as f32 + 0.5 - cy) / scale);
                let r = sqrtf(dx * dx + dy * dy).max(0.004);
                let ang = (atan2f(dy, dx) / TAU + 0.5) * 256.0;
                c.map.push(Px {
                    inv: 1.0 / r,
                    ang,
                    fog: expf(-r * (2.2 - 0.5 * 1.5)),
                    core: expf(-r * 6.0),
                    rib: smooth(0.95, 0.2, r),
                    ray: smooth(0.0, 0.6, r) * smooth(1.1, 0.4, r),
                    vig: 1.0 - 0.5 * smooth(0.6, 1.5, r),
                    r14: r * 14.0 / TAU * 256.0,
                    shock: smooth(1.2, 0.1, r),
                });
            }
        }
        if c.tone.is_empty() {
            c.tone = (0..1024)
                .map(|i| {
                    let v = i as f32 / 256.0;
                    (powf(v / (1.0 + v), 0.85) * 255.0).min(255.0) as u8
                })
                .collect();
        }
    }

    pub(super) fn draw_bass_cathedral(&mut self, dt: f32, moving: bool, calm: f32, reduce_motion: bool) {
        let fresh = self.pulse > 0.0 && self.pulse > 0.95 * self.pulse_set;
        if moving {
            let c = &mut self.scenes.cath;
            c.t += dt * calm;
            c.shake = (c.shake - dt * 3.6).max(0.0);
            if fresh && !reduce_motion {
                c.shake = 1.0;
            }
        }
        let (bw, bh) = (self.bw, self.bh);
        if self.scenes.cath.map.len() != bw * bh {
            self.build_cathedral();
        }
        let (bass, mid, treble) = (self.bass, self.mid, self.treble);
        let shake = if reduce_motion { 0.0 } else { (self.scenes.cath.shake).max(bass * 0.35) };
        let flash = if reduce_motion { 0.0 } else { self.pulse };
        let t = self.scenes.cath.t;
        // Camera recoil: a few pixels, never more than a hundredth or so of the picture.
        let reach = (bh as f32 * 0.03).max(1.0);
        let (sx, sy) =
            ((libm::sinf(t * 63.0) * shake * reach) as i32, (libm::cosf(t * 57.0) * shake * reach) as i32);
        let zoom = 1.0 - (bass * 0.10 + shake * 0.18 * 0.5);
        let inv_zoom = 1.0 / zoom;
        let fly = t * 2.2;
        let twist = 0.35 * libm::sinf(t * 0.2) / TAU * 256.0;
        let ca = (0.004 + shake * 0.02) * 1.2; // the chromatic fringe, in nave depth
        let wall_gain = 0.6 + mid * 0.9;
        let rib_gain = 0.8 + bass * 1.8;
        let core_gain = 1.4 + bass * 3.2;
        let ray_gain = 0.15 + treble * 0.7;
        let lift = 0.10 * flash;
        let ray_phase = t * 2.0 / TAU * 256.0;
        let ring_phase = flash * 10.0 / TAU * 256.0;
        let ring_gain = flash * 0.9;
        let ring_col = self.col(38);
        let ray_col = self.col(153);
        let sin_lut = self.sin_lut;
        let to01 = |v: i16| (v as f32 + 1024.0) * (0.5 / 1024.0);
        let map = core::mem::take(&mut self.scenes.cath.map);
        let tone = core::mem::take(&mut self.scenes.cath.tone);
        for y in 0..bh {
            let my = (y as i32 + sy).clamp(0, bh as i32 - 1) as usize;
            for x in 0..bw {
                let mx = (x as i32 + sx).clamp(0, bw as i32 - 1) as usize;
                let p = &map[my * bw + mx];
                let depth = p.inv * inv_zoom;
                let z = depth * 1.2 - fly;
                let ang = p.ang + twist;
                let ang_wall = to01(sin_lut[((ang * 24.0) as i32 & 255) as usize]);
                // The wall's colour and the pillars' follow the flight; the three channels see the nave a hair apart (the fringe).
                let pal = self.lut[(((z * 0.05 + ang * 0.5 / 256.0) * 256.0) as i32 & 255) as usize];
                let rib_pal = self.lut[(((z * 0.05 + 0.3) * 256.0) as i32 & 255) as usize];
                let s = to01(sin_lut[(((ang * 16.0 + ray_phase) as i32) & 255) as usize]);
                let s2 = s * s;
                let rays = s2 * s2 * s2 * p.ray * ray_gain;
                let ring = to01(sin_lut[((p.r14 - ring_phase) as i32 & 255) as usize]);
                let ring = (ring * 2.0 - 1.0).max(0.0) * ring_gain * p.shock;
                let mut out = [0f32; 3];
                for ch in 0..3 {
                    let zc = z + (ch as f32 - 1.0) * ca * depth * 1.2;
                    let wall = ang_wall * to01(sin_lut[((zc * 3.0 / TAU * 256.0) as i32 & 255) as usize]);
                    let f = zc * 0.5 - libm::floorf(zc * 0.5);
                    let d = libm::fabsf(f - 0.5) - 0.02;
                    let rib = smooth(0.12, 0.0, d) * p.rib;
                    let core = [1.0, 0.9, 0.8][ch] * p.core * core_gain;
                    let mut v = pal[ch] as f32 / 255.0 * wall * p.fog * wall_gain
                        + rib_pal[ch] as f32 / 255.0 * rib * rib_gain * p.fog
                        + core
                        + ray_col[ch] as f32 / 255.0 * rays
                        + ring_col[ch] as f32 / 255.0 * ring
                        + lift * [0.9, 0.7, 1.0][ch];
                    v *= 1.3;
                    let hot = (v - 0.6).max(0.0);
                    v += hot * hot * 1.2;
                    out[ch] = v * p.vig;
                }
                let i = (y * bw + x) * 4;
                for ch in 0..3 {
                    let q = ((out[ch] * 256.0) as usize).min(1023);
                    self.buf[i + ch] = tone[q];
                }
                self.buf[i + 3] = 255;
            }
        }
        self.scenes.cath.map = map;
        self.scenes.cath.tone = tone;
        let prev = self.scenes.cath.mean;
        self.scenes.cath.mean = self.limit_jump(prev);
    }
}
