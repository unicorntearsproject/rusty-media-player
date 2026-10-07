//! Scenes in the spirit of the Unicorn Viz effects, drawn on the CPU: the **Bass machine** (a subwoofer, a boombox and a record player,
//! each wired to the live spectrum), **Unicorn Tears** (iridescent teardrops falling through a deep star field) and the **Disco ball** (a
//! mirror-tiled ball that throws sweeping squares of light across the room, with beams and a dance floor).
//!
//! Written from scratch for this player; Unicorn Viz (MIT) is where the ideas come from, its GPU code is not used. They follow the other
//! effects' rules: additive drawing into a small picture, brightness pulses on the beat capped (see the module documentation of
//! [`super`]), and with reduced motion no pulses, no bursts, a slow drift. A scene change never cuts: it fades through the dark.
use super::{Effect, Particle, Viz};
use alloc::vec::Vec;
use libm::{cosf, fabsf, floorf, powf, sinf, sqrtf};

const TAU: f32 = core::f32::consts::TAU;

/// How long one Bass machine scene plays before the next, seconds.
const SCENE_SECS: f32 = 22.0;
/// The fade through the dark between scenes, seconds.
const SCENE_FADE: f32 = 1.4;
/// Mirror tiles of the disco ball, round and from pole to pole.
const TILES_U: usize = 28;
const TILES_V: usize = 14;
/// Sparkles drifting in the disco ball's room.
const ROOM_SPARKS: usize = 150;

/// An expanding pressure ring thrown out by a speaker port on the beat.
#[derive(Debug, Clone, Copy)]
pub(in crate::effects) struct Ring {
    r: f32,
    life: f32,
}

/// A drip of light on the speaker wall: `kind` 0 hangs from the top edge and falls, 1 runs down a side edge, 2 is a splash thrown up from the
/// bottom edge. `x`, `y` in pixels of the picture, `vy` pixels a second down.
#[derive(Debug, Clone, Copy)]
pub(in crate::effects) struct Drip {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    len: f32,
    hue: u8,
    kind: u8,
}

/// Most drips alive at once (so the cost per frame is bounded however loud the music is).
const MAX_DRIPS: usize = 72;
/// For the tests: the cap on drips.
#[cfg(test)]
pub(super) const MAX_DRIPS_FOR_TESTS: usize = MAX_DRIPS;
/// Twinkling sparkles around the wall's edges.
const EDGE_SPARKS: usize = 110;

#[cfg(test)]
impl Drip {
    pub(super) fn kind_for_tests(&self) -> u8 {
        self.kind
    }
}

/// What the three scenes remember between frames.
#[derive(Debug, Default)]
pub(super) struct SceneState {
    /// Which of the Bass machine's three scenes: 0 subwoofer, 1 boombox, 2 record player.
    pub(in crate::effects) scene: u8,
    /// Seconds into the scene.
    t: f32,
    /// Fading out (1 to 0) then in (0 to 1) around a change; 1 is full brightness.
    pub(in crate::effects) fade: f32,
    /// A change is in progress (fading out).
    leaving: bool,
    /// Speaker rings.
    pub(in crate::effects) rings: Vec<Ring>,
    /// Reel and platter angle.
    reel: f32,
    /// Tonearm progress across the record, 0..1.
    arm: f32,
    /// Teardrops (`x` and `y` in 0..1 of the picture, `z` the size, `vy` the fall speed).
    pub(in crate::effects) drops: Vec<Particle>,
    /// Stars: `(x, y, layer)`.
    stars: Vec<(f32, f32, u8)>,
    /// The disco ball's spin, radians.
    ball_spin: f32,
    /// Dance floor tiles lit by the beat: age of the cascade, 0..1.
    cascade: f32,
    /// Hue shift of the ball's room, advancing on the beat.
    pub(in crate::effects) hue_kick: f32,
    /// Which style of room the ball is in (changes with the Bass machine's clock, never twice in a row).
    room: u8,
    room_t: f32,
    /// The disco ball's beat flash, 1 on the hit and decaying; and a count of beats (which facets flash changes with it).
    beat_flash: f32,
    beat_count: u32,
    /// Drips running down and falling off the speaker wall's edges.
    pub(in crate::effects) drips: Vec<Drip>,
    /// Seconds until the next idle drip.
    drip_in: f32,
    /// The Bass cathedral's tables and flight.
    pub(super) cath: super::cathedral::Cath,
    /// Sun Ship 3000's battle.
    pub(super) ship: super::sunship::Ship,
}

fn hash2(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65_535.0
}

fn scale_c(c: [u8; 3], k: f32) -> [u8; 3] {
    [
        (c[0] as f32 * k).min(255.0) as u8,
        (c[1] as f32 * k).min(255.0) as u8,
        (c[2] as f32 * k).min(255.0) as u8,
    ]
}

impl Viz {
    /// Advance the scene clocks by `dt` and make them ready (called before the three scenes draw).
    fn scenes_step(&mut self, dt: f32, moving: bool, calm: f32) {
        if !moving {
            return;
        }
        let fresh = self.pulse > 0.0 && self.pulse > 0.95 * self.pulse_set;
        let (mid, bass) = (self.mid, self.bass);
        let st = &mut self.scenes;
        st.t += dt;
        st.reel += dt * (0.6 + mid * 4.0) * calm;
        st.arm = (st.arm + dt * 0.004 * calm) % 1.0;
        st.ball_spin += dt * (0.35 + mid * 1.6 + bass * 0.6) * calm;
        st.room_t += dt;
        if st.fade < 1.0 && !st.leaving {
            st.fade = (st.fade + dt / SCENE_FADE).min(1.0);
        }
        if st.leaving {
            st.fade -= dt / SCENE_FADE;
            if st.fade <= 0.0 {
                st.fade = 0.0;
                st.leaving = false;
                st.t = 0.0;
                st.scene = (st.scene + 1) % 3;
                st.rings.clear();
            }
        } else if st.t > SCENE_SECS {
            st.leaving = true;
        }
        if st.room_t > SCENE_SECS * 1.5 {
            st.room_t = 0.0;
            st.room = (st.room + 1) % 3;
        }
        for r in &mut st.rings {
            r.r += dt * 0.55;
            r.life -= dt * 0.9;
        }
        st.rings.retain(|r| r.life > 0.0);
        st.cascade = (st.cascade + dt * 1.6).min(1.0);
        st.beat_flash = (st.beat_flash - dt * 5.0).max(0.0);
        // Speaker wall drips: they fall, and where they land they splash up.
        let (bwf, bhf) = (self.bw as f32, self.bh as f32);
        let g_acc = bhf * 0.9;
        let mut splashes: Vec<Drip> = Vec::new();
        for d in &mut st.drips {
            match d.kind {
                0 => {
                    d.vy += g_acc * dt;
                    d.y += d.vy * dt;
                }
                1 => {
                    d.vy = (d.vy + g_acc * 0.25 * dt).min(bhf * 0.55);
                    d.y += d.vy * dt;
                }
                _ => {
                    d.vy += g_acc * 1.4 * dt;
                    d.y += d.vy * dt;
                    d.x += d.vx * dt;
                }
            }
            if d.kind != 2 && d.y >= bhf - 2.0 {
                d.y = f32::MAX; // landed: gone, and it throws a few drops up
                if d.kind == 0 {
                    splashes.push(*d);
                }
            }
            if d.kind == 2 && d.vy > 0.0 && d.y >= bhf - 2.0 {
                d.y = f32::MAX;
            }
        }
        st.drips.retain(|d| d.y < f32::MAX && d.x > -4.0 && d.x < bwf + 4.0);
        for (n, d) in splashes.iter().enumerate() {
            for k in 0..2 {
                if st.drips.len() < MAX_DRIPS {
                    let side = if k == 0 { -1.0 } else { 1.0 };
                    st.drips.push(Drip {
                        x: d.x,
                        y: bhf - 3.0,
                        vx: side * bhf * (0.10 + 0.18 * hash2(n as u32, k as u32 + 3)),
                        vy: -bhf * (0.28 + 0.3 * hash2(n as u32 + 9, k as u32)),
                        len: 3.0,
                        hue: d.hue,
                        kind: 2,
                    });
                }
            }
        }
        // New drips: a slow trickle always, more with the bass, a burst on the beat (none in calm mode, which also trickles slower).
        st.drip_in -= dt * (0.9 + bass * 5.0) * calm;
        let mut spawn = 0;
        if st.drip_in <= 0.0 {
            st.drip_in = 0.35;
            spawn += 1;
        }
        if fresh && calm >= 1.0 {
            spawn += 6;
        }
        for k in 0..spawn {
            if st.drips.len() >= MAX_DRIPS {
                break;
            }
            let r = hash2((st.t * 977.0) as u32 ^ (k * 131), st.drips.len() as u32 + 11);
            let r2 = hash2((st.t * 313.0) as u32 + k, 71);
            let hue = (r2 * 255.0) as u8;
            // Along the top edge, or running down the left or the right edge.
            let (x, y, kind) = if r < 0.6 {
                (bwf * (0.02 + 0.96 * r2), 1.0, 0u8)
            } else if r < 0.8 {
                (2.0, bhf * 0.5 * r2, 1)
            } else {
                (bwf - 3.0, bhf * 0.5 * r2, 1)
            };
            st.drips.push(Drip {
                x,
                y,
                vx: 0.0,
                vy: bhf * (0.04 + 0.1 * r2),
                len: bhf * (0.04 + 0.08 * r),
                hue,
                kind,
            });
        }
        // The beat starts a ring (once per pulse: the pulse gap already caps them at three a second) and a floor cascade.
        if fresh {
            if st.rings.len() < 5 {
                st.rings.push(Ring { r: 0.05, life: 1.0 });
            }
            st.cascade = 0.0;
            st.hue_kick += 0.07;
            if calm >= 1.0 {
                st.beat_flash = 1.0;
                st.beat_count = st.beat_count.wrapping_add(1);
            }
        }
    }

    /// The scene fade as a brightness factor (a smooth step, so nothing blinks).
    fn scene_brightness(&self) -> f32 {
        let f = self.scenes.fade.clamp(0.0, 1.0);
        f * f * (3.0 - 2.0 * f)
    }

    // ---- Bass machine -----------------------------------------------------------------------------------------------------

    pub(super) fn draw_bass_machine(&mut self, dt: f32, moving: bool, calm: f32) {
        self.scenes_step(dt, moving, calm);
        self.backdrop(0.25 + self.bass * 0.35);
        let k = self.scene_brightness();
        let scene = self.scenes.scene;
        // Draw into the cleared night; the fade scales what the scene adds by darkening afterwards.
        match scene {
            0 => self.scene_subwoofer(),
            1 => self.scene_boombox(),
            _ => self.scene_record(),
        }
        if k < 0.999 {
            let m = (k * 256.0) as i32;
            for p in self.buf.chunks_exact_mut(4) {
                for c in &mut p[..3] {
                    *c = ((*c as i32 * m) >> 8) as u8;
                }
            }
        }
    }

    /// The speaker wall: two big subs, a column of mids between them and a row of tweeters above, filling the picture; each driver pumps with its
    /// own band. Light drips run down the edges and sparkles twinkle all the way round the frame.
    fn scene_subwoofer(&mut self) {
        let g = self.gain();
        let (bw, bh) = (self.bw as f32, self.bh as f32);
        let tint = self.col(((self.phase * 6.0) as usize) & 255);
        // The cabinet is the whole picture: a bevelled frame in the palette tint with a screw in each corner.
        for e in 0..4 {
            let o = 1.5 + e as f32 * 1.6;
            let edge = scale_c(tint, 0.34 - e as f32 * 0.07);
            self.line_add(o, o, bw - o, o, edge, 1.0, 0);
            self.line_add(o, bh - o, bw - o, bh - o, edge, 1.0, 0);
            self.line_add(o, o, o, bh - o, edge, 1.0, 0);
            self.line_add(bw - o, o, bw - o, bh - o, edge, 1.0, 0);
        }
        let screw = (bh * 0.018).max(2.0);
        for &(sx, sy) in &[
            (bw * 0.025, bh * 0.045),
            (bw * 0.975, bh * 0.045),
            (bw * 0.025, bh * 0.955),
            (bw * 0.975, bh * 0.955),
        ] {
            self.disc_add(sx, sy, screw, [150, 146, 170], 0.7);
        }
        let low = |a: usize, b: usize, s: &Self| (a..b).map(|i| s.bands[i]).sum::<f32>() / (b - a) as f32;
        let (sub_l, sub_r) =
            ((self.bass * 0.6 + low(0, 4, self)).min(1.2), (self.bass * 0.6 + low(2, 6, self)).min(1.2));
        let mids = low(8, 20, self).max(self.mid * 0.8);
        let highs = low(22, 32, self).max(self.treble * 0.8);
        // The two subs, big and low.
        let r_sub = (bw * 0.20).min(bh * 0.36);
        let (cy_sub, x_l, x_r) = (bh * 0.58, bw * 0.235, bw * 0.765);
        self.driver(x_l, cy_sub, r_sub, sub_l, tint, g, 0.09);
        self.driver(x_r, cy_sub, r_sub, sub_r, tint, g, 0.09);
        // Two mids stacked between them.
        let gap = (x_r - x_l) * 0.5 - r_sub - 4.0;
        let r_mid = (bh * 0.115).min(gap * 0.95).max(6.0);
        let mid_tint = self.col(((self.phase * 6.0) as usize + 50) & 255);
        self.driver(bw * 0.5, bh * 0.43, r_mid, mids, mid_tint, g, 0.07);
        self.driver(bw * 0.5, bh * 0.76, r_mid, (mids * 0.9).min(1.2), mid_tint, g, 0.07);
        // A row of tweeters along the top: small domes that flutter with the highs.
        let hi_tint = self.col(((self.phase * 6.0) as usize + 110) & 255);
        let r_tw = (bh * 0.045).max(3.0);
        for i in 0..6 {
            let x = bw * (0.13 + 0.148 * i as f32);
            let k = (highs * (0.7 + 0.3 * sinf(self.phase * 9.0 + i as f32 * 1.7))).clamp(0.0, 1.2);
            self.driver(x, bh * 0.14, r_tw, k, hi_tint, g, 0.12);
        }
        // The ports of the subs throw pressure rings on the beat.
        let rings = self.scenes.rings.clone();
        for &px in &[x_l, x_r] {
            for rg in &rings {
                self.ring_add(px, cy_sub, r_sub * (0.5 + rg.r * 1.6), 1.8, scale_c(tint, 0.5 * rg.life * g));
            }
        }
        // Light dripping down the edges, and sparkles twinkling all the way round.
        let drips = self.scenes.drips.clone();
        for d in &drips {
            let c = self.col((d.hue as usize + (self.phase * 14.0) as usize) & 255);
            let steps = (d.len as i32).max(2);
            for s in 0..steps {
                let f = s as f32 / steps as f32;
                // The trail fades behind the drip; the head is a bright bead. A splash trails along its own path.
                let (tx, ty) = if d.kind == 2 {
                    (d.x - d.vx * 0.04 * f, d.y - d.vy * 0.04 * f)
                } else {
                    (d.x, d.y - f * d.len)
                };
                self.add(tx as i32, ty as i32, c, (1.0 - f) * 0.55 * g);
            }
            self.disc_add(d.x, d.y, 1.8, scale_c(c, 1.0), 0.9 * g);
        }
        let reach = (bh * 0.014).max(2.0);
        for i in 0..EDGE_SPARKS {
            let u = hash2(i as u32, 1);
            let inset = 2.0 + hash2(i as u32, 2) * bh * 0.03;
            // A point on the frame: u runs round the perimeter.
            let per = 2.0 * (bw + bh);
            let d = u * per;
            let (x, y) = if d < bw {
                (d, inset)
            } else if d < bw + bh {
                (bw - inset, d - bw)
            } else if d < 2.0 * bw + bh {
                (bw - (d - bw - bh), bh - inset)
            } else {
                (inset, bh - (d - 2.0 * bw - bh))
            };
            let rate = 1.5 + hash2(i as u32, 3) * 3.5;
            let tw = sinf(self.phase * rate * 4.0 + hash2(i as u32, 4) * 40.0).max(0.0);
            let k = tw * tw * tw * (0.25 + highs * 0.9 + self.level * 0.4).min(1.4) * g;
            if k < 0.05 {
                continue;
            }
            let c = self.col(((hash2(i as u32, 5) * 255.0) as usize + (self.phase * 20.0) as usize) & 255);
            self.add(x as i32, y as i32, [255, 255, 255], k);
            let arm = (reach * (0.5 + tw)) as i32;
            for o in 1..=arm {
                let f = k * (1.0 - o as f32 / (arm + 1) as f32);
                self.add(x as i32 + o, y as i32, c, f);
                self.add(x as i32 - o, y as i32, c, f);
                self.add(x as i32, y as i32 + o, c, f);
                self.add(x as i32, y as i32 - o, c, f);
            }
        }
    }

    /// A speaker driver at (`cx`, `cy`): surround, cone (its shading follows `level`), dust cap or dome with a highlight. The cone pumps with
    /// the level, `pump` being how far at full level (a fraction of the radius).
    #[allow(clippy::too_many_arguments)]
    fn driver(&mut self, cx: f32, cy: f32, r: f32, level: f32, tint: [u8; 3], g: f32, pump: f32) {
        let excursion = level * pump * 0.9;
        self.disc_fast(cx, cy, r * 1.08, [54, 50, 70], 1.6);
        let rings = ((r / 5.0) as usize).clamp(3, 9);
        for ring in 0..rings {
            let f = ring as f32 / rings as f32;
            let rr = r * (1.0 - f * 0.78) * (1.0 + excursion * (1.0 - f));
            let shade = 0.10 + 0.20 * f + level * 0.22 * (1.0 - f);
            self.ring_add(cx, cy, rr, 1.4, scale_c(tint, shade * g));
        }
        let cap = r * 0.24 * (1.0 + excursion * 1.6);
        self.disc_add(cx, cy, cap, [190, 186, 210], 0.9);
        let hx = cx + cosf(self.scenes.reel * 0.4) * cap * 0.35;
        let hy = cy - sinf(self.scenes.reel * 0.4) * cap * 0.35;
        self.disc_add(hx, hy, cap * 0.4, [255, 255, 255], 0.5 + level * 0.4);
        // The whole driver glows with its own level.
        self.disc_fast(cx, cy, r * 1.3, tint, 0.08 + level * 0.22);
    }

    fn scene_boombox(&mut self) {
        let g = self.gain();
        let (bw, bh) = (self.bw as f32, self.bh as f32);
        let (cx, cy) = (bw * 0.5, bh * 0.52);
        let (w, h) = (bw * 0.82, bh * 0.62);
        let (x0, y0) = (cx - w * 0.5, cy - h * 0.5);
        let tint = self.col(((self.phase * 5.0) as usize) & 255);
        self.rect_add(x0 as i32, y0 as i32, (x0 + w) as i32, (y0 + h) as i32, [16, 14, 26], 1.0);
        self.line_add(x0, y0, x0 + w, y0, scale_c(tint, 0.4), 1.0, 1);
        // The handle.
        self.line_add(x0 + w * 0.2, y0 - 6.0, x0 + w * 0.8, y0 - 6.0, [70, 66, 90], 1.0, 1);
        // Twin woofers pump on the bass.
        for &wx in &[x0 + w * 0.16, x0 + w * 0.84] {
            let r = h * 0.30 * (1.0 + self.bass * 0.05);
            self.disc_add(wx, cy + h * 0.04, r * 1.06, [48, 44, 66], 0.9);
            for ring in 0..6 {
                let f = ring as f32 / 6.0;
                self.ring_add(
                    wx,
                    cy + h * 0.04,
                    r * (1.0 - f * 0.8),
                    1.4,
                    scale_c(tint, (0.14 + 0.18 * f + self.bass * 0.3 * (1.0 - f)) * g),
                );
            }
            self.disc_add(wx, cy + h * 0.04, r * 0.18, [200, 196, 220], 0.8);
        }
        // The EQ ladder: sixteen columns of segments from the spectrum.
        let (lx0, lx1) = (x0 + w * 0.30, x0 + w * 0.70);
        let (ly1, ly0) = (cy + h * 0.30, cy - h * 0.05);
        let cols = 16usize;
        let segs = 10usize;
        let cw = (lx1 - lx0) / cols as f32;
        for c in 0..cols {
            let a = c * 2;
            let v = ((self.bands[a] + self.bands[a + 1]) * 0.5).clamp(0.0, 1.0);
            let lit = libm::ceilf(powf(v, 0.9) * segs as f32) as usize;
            for sgm in 0..segs {
                let sy = ly1 - (sgm as f32 + 1.0) * ((ly1 - ly0) / segs as f32);
                let on = sgm < lit;
                let col = if sgm >= segs - 2 {
                    [255, 90, 110]
                } else if sgm >= segs - 4 {
                    [255, 214, 90]
                } else {
                    [77, 255, 140]
                };
                let k = if on { 0.9 * g } else { 0.08 };
                self.rect_add(
                    (lx0 + c as f32 * cw + 1.0) as i32,
                    sy as i32,
                    (lx0 + (c + 1) as f32 * cw - 1.0) as i32,
                    (sy + (ly1 - ly0) / segs as f32 - 1.5) as i32,
                    col,
                    k,
                );
            }
        }
        // Two VU needles on bass and mid.
        for (i, &(nx, level)) in [(x0 + w * 0.40, self.bass), (x0 + w * 0.60, self.mid)].iter().enumerate() {
            let (px, py) = (nx, cy - h * 0.12);
            let _ = i;
            self.rect_add(
                (px - 18.0) as i32,
                (py - 22.0) as i32,
                (px + 18.0) as i32,
                py as i32,
                [30, 26, 40],
                1.0,
            );
            let ang = -0.9 + level.clamp(0.0, 1.0) * 1.8;
            self.line_add(px, py, px + sinf(ang) * 20.0, py - cosf(ang) * 20.0, [255, 200, 120], 0.9, 0);
        }
        // The cassette reels turn at a rate set by the mid band.
        for &rx in &[x0 + w * 0.42, x0 + w * 0.58] {
            let ry = cy + h * 0.38;
            self.disc_add(rx, ry, 8.0, [60, 56, 80], 0.8);
            for s in 0..3 {
                let a = self.scenes.reel + s as f32 * TAU / 3.0;
                self.line_add(rx, ry, rx + cosf(a) * 7.0, ry + sinf(a) * 7.0, [210, 206, 230], 0.9, 0);
            }
        }
    }

    fn scene_record(&mut self) {
        let g = self.gain();
        let (bw, bh) = (self.bw as f32, self.bh as f32);
        let (cx, cy) = (bw * 0.46, bh * 0.54);
        let r = bh * 0.42;
        let tint = self.col(((self.phase * 5.0) as usize) & 255);
        // The platter and the record: every groove ring reads a different band, so the record is cut by what is playing.
        self.disc_add(cx, cy, r * 1.12, [30, 28, 42], 1.0);
        self.disc_add(cx, cy, r, [10, 9, 14], 1.0);
        let rings = 28usize;
        for i in 0..rings {
            let f = i as f32 / rings as f32;
            let rr = r * (0.34 + 0.64 * f);
            let band = (f * 31.0) as usize;
            let v = self.bands[band.min(31)];
            let col = self.col(((f * 200.0) as usize + (self.phase * 20.0) as usize) & 255);
            self.ring_add(cx, cy, rr, 1.0 + v * 1.4, scale_c(col, (0.12 + 0.75 * v) * g));
        }
        // The label, spinning.
        let lr = r * 0.30;
        self.disc_add(cx, cy, lr, scale_c(tint, 0.8), 0.9);
        for s in 0..4 {
            let a = self.scenes.reel * 0.5 + s as f32 * TAU / 4.0;
            self.line_add(cx, cy, cx + cosf(a) * lr * 0.9, cy + sinf(a) * lr * 0.9, [255, 240, 250], 0.7, 0);
        }
        self.disc_add(cx, cy, 2.5, [0, 0, 0], 1.0);
        // The tonearm: from its pivot at the top right inward as the record plays.
        let (px, py) = (cx + r * 1.28, cy - r * 0.9);
        let target =
            (cx + r * (0.95 - 0.55 * self.scenes.arm) * 0.8, cy - r * 0.35 + r * 0.55 * self.scenes.arm);
        self.disc_add(px, py, 6.0, [90, 86, 110], 0.9);
        self.line_add(px, py, target.0, target.1, [190, 186, 210], 0.95, 1);
        self.disc_add(target.0, target.1, 2.5, [255, 255, 255], 0.8);
    }

    /// A circle outline `thick` pixels wide, in `c` (additive).
    pub(super) fn ring_add(&mut self, cx: f32, cy: f32, r: f32, thick: f32, c: [u8; 3]) {
        if r < 1.0 {
            return;
        }
        let steps = ((r * TAU) as i32).clamp(12, 700);
        for i in 0..steps {
            let a = i as f32 / steps as f32 * TAU;
            let (x, y) = (cx + cosf(a) * r, cy + sinf(a) * r);
            let t = thick as i32;
            for o in 0..=t {
                self.add(x as i32, y as i32 + o, c, 1.0);
            }
        }
    }

    // ---- Unicorn Tears ------------------------------------------------------------------------------------------------------

    pub(super) fn draw_unicorn_tears(&mut self, dt: f32, moving: bool, calm: f32) {
        let g = self.gain();
        let (bw, bh) = (self.bw as f32, self.bh as f32);
        if self.scenes.stars.is_empty() {
            for i in 0..220u32 {
                let layer = (hash2(i, 3) * 3.0) as u8;
                self.scenes.stars.push((hash2(i, 1), hash2(i, 2), layer));
            }
        }
        self.fade(if calm < 1.0 { 0.90 } else { 0.84 });
        // Deep parallax stars, twinkling, the near ones pulsing with the bass (smoothly, never a flash).
        let stars = self.scenes.stars.clone();
        for (i, (sx, sy, layer)) in stars.iter().enumerate() {
            let drift = if moving { self.phase * (0.004 + *layer as f32 * 0.006) } else { 0.0 };
            let y = (sy + drift) % 1.0;
            let tw = 0.55 + 0.45 * sinf(self.phase * (1.5 + *layer as f32) + i as f32 * 1.7);
            let k = (0.18 + 0.16 * *layer as f32) * tw * (1.0 + self.bass * 0.5 * (*layer as f32 / 2.0)) * g;
            self.add((sx * bw) as i32, (y * bh) as i32, [200, 190, 255], k);
        }
        if moving {
            // New teardrops: a few a second at rest, more on the beat (the pulse gap caps the beats).
            let rate = (0.9 + self.level * 2.5) * calm;
            let mut spawn = rate * dt;
            if self.pulse > 0.9 * self.pulse_set && self.pulse_set > 0.0 && calm >= 1.0 {
                spawn += 3.0;
            }
            while spawn > 0.0 && self.scenes.drops.len() < 90 {
                let take = if spawn >= 1.0 { 1.0 } else { spawn };
                if self.rand() < take {
                    let x = self.rand();
                    let z = 0.7 + self.rand() * 0.9 + self.bass * 0.4;
                    let vy = 0.10 + self.rand() * 0.16;
                    let hue = (self.rand() * 255.0) as u8;
                    self.scenes.drops.push(Particle {
                        x,
                        y: -0.08,
                        z,
                        vx: 0.0,
                        vy,
                        life: 1.0,
                        hue,
                        burst: false,
                    });
                }
                spawn -= 1.0;
            }
            let fall = 0.55 + self.level * 0.6;
            for d in &mut self.scenes.drops {
                d.y += d.vy * fall * dt * calm;
                d.x += sinf(d.y * 6.0 + d.hue as f32) * 0.012 * dt * calm;
            }
            self.scenes.drops.retain(|d| d.y < 1.12);
        }
        let drops = core::mem::take(&mut self.scenes.drops);
        for d in &drops {
            let r = bh * 0.016 * d.z;
            let (px, py) = (d.x * bw, d.y * bh);
            self.teardrop(px, py, r, d.hue, g);
        }
        self.scenes.drops = drops;
    }

    /// One teardrop: pointed at the top, round at the bottom, filled with colours that shift as it falls, with a specular glint.
    fn teardrop(&mut self, cx: f32, cy: f32, r: f32, hue: u8, g: f32) {
        let tip = cy - r * 2.3;
        let height = r * 3.3;
        let (y0, y1) = (tip as i32, (tip + height) as i32 + 1);
        for y in y0..=y1 {
            let t = ((y as f32 - tip) / height).clamp(0.0, 1.0);
            let hw = if t < 0.72 {
                r * powf(t / 0.72, 1.7)
            } else {
                let u = (t - 0.72) / 0.28;
                r * sqrtf((1.0 - u * u).max(0.0))
            };
            let xs = (cx - hw) as i32;
            let xe = (cx + hw) as i32;
            for x in xs..=xe {
                let across = if hw > 0.5 { (x as f32 - cx) / hw } else { 0.0 };
                let idx =
                    (hue as f32 + y as f32 * 2.2 + across * 30.0 + self.phase * 40.0 + self.treble * 60.0)
                        as usize;
                let c = self.col(idx & 255);
                let shade = (0.55 + 0.45 * (1.0 - across * across).max(0.0)) * (0.6 + 0.4 * t);
                self.add(x, y, c, shade * g);
            }
        }
        // The glint on the upper left of the bulb.
        self.disc_add(cx - r * 0.35, cy - r * 0.15, r * 0.32, [255, 255, 255], 0.75);
    }

    // ---- Disco ball ---------------------------------------------------------------------------------------------------------

    pub(super) fn draw_disco_ball(&mut self, dt: f32, moving: bool, calm: f32) {
        self.scenes_step(dt, moving, calm);
        let g = self.gain();
        let (bw, bh) = (self.bw as f32, self.bh as f32);
        let room = self.scenes.room;
        // The room: a dark violet by default, a deep blue or a green-black in the others (never pure black).
        let (br, bgc, bb) = match room {
            0 => (10u8, 7u8, 20u8),
            1 => (6, 12, 24),
            _ => (6, 18, 14),
        };
        for p in self.buf.chunks_exact_mut(4) {
            p[0] = br;
            p[1] = bgc;
            p[2] = bb;
            p[3] = 255;
        }
        let (cx, cy) = (bw * 0.5, bh * 0.34);
        let r = bh * (0.17 + self.bass * 0.006);
        let hue_base = (self.phase * 14.0 + self.scenes.hue_kick * 255.0) as usize;
        // The dance floor: tiles in perspective at the bottom, lit by a cascade from the middle.
        self.dance_floor(hue_base, g);
        // Beams from two spots sweeping with the spin.
        for (i, &sx) in [bw * 0.12, bw * 0.88].iter().enumerate() {
            let a =
                sinf(self.scenes.ball_spin * 0.7 + i as f32 * 2.1) * 0.6 + if i == 0 { 0.35 } else { -0.35 };
            let col = self.col((hue_base + i * 90) & 255);
            self.beam(
                sx,
                bh * 0.98,
                -core::f32::consts::FRAC_PI_2 + a,
                0.07,
                col,
                (0.11 + self.treble * 0.12) * g,
            );
        }
        // Every mirror facet of the ball: its normal (spun with the ball) and what it throws back at the three lights that circle the room.
        // `glints[tile]` is how hard the facet flashes at the viewer; `spots` are the squares of light it throws across the room.
        let (nu, nv) = (TILES_U, TILES_V);
        let spin = self.scenes.ball_spin;
        let bf = self.scenes.beat_flash;
        let beats = self.scenes.beat_count;
        let mut glints = [0f32; TILES_U * TILES_V];
        let lights =
            [(self.phase * 0.31, 0.25f32), (self.phase * 0.23 + 2.1, -0.15), (self.phase * 0.41 + 4.2, 0.45)];
        let hot = 0.5 + self.treble * 0.9;
        for iu in 0..nu {
            let lon = (iu as f32 + 0.5) / nu as f32 * TAU + spin;
            let (sl, cl) = (sinf(lon), cosf(lon));
            for iv in 0..nv {
                let lat = ((iv as f32 + 0.5) / nv as f32 - 0.5) * core::f32::consts::PI;
                let (cla, sla) = (cosf(lat), sinf(lat));
                let (nx, ny, nz) = (cla * sl, sla, cla * cl);
                let tile = iu * nv + iv;
                if nz > -0.1 {
                    // Mirror reflection of the viewer's ray, against each light.
                    let (rx, ry, rz) = (2.0 * nz * nx, 2.0 * nz * ny, 2.0 * nz * nz - 1.0);
                    let mut gl = 0.0;
                    for &(la, lh) in &lights {
                        let (lx, ly, lz) = (cosf(la) * 0.9, lh, sinf(la) * 0.9 + 0.3);
                        let inv = 1.0 / sqrtf(lx * lx + ly * ly + lz * lz);
                        let d = ((rx * lx + ry * ly + rz * lz) * inv).max(0.0);
                        let d2 = d * d;
                        let d4 = d2 * d2;
                        let d8 = d4 * d4;
                        gl += d8 * d8 * d8; // d^24
                    }
                    // Some facets flash on each beat (another set every time).
                    let flash = if hash2(tile as u32, beats) < 0.28 { bf } else { 0.0 };
                    glints[tile] = (gl * 2.4 * hot + flash * 0.9).min(1.6);
                }
                // The spot this facet throws: where its mirror image of the front light lands in the room.
                if nz < -0.2 || hash2(tile as u32, 77) > 0.72 {
                    continue;
                }
                let (dx, dy) = (nx * 2.0, ny * 1.4 - 0.2 * nz);
                let (sx, sy) = (cx + dx * bw * 0.62, cy + dy * bh * 0.78);
                let h = hash2(tile as u32, 5);
                let c = self.col(((h * 255.0) as usize + hue_base) & 255);
                let tw = 0.5 + 0.5 * sinf(self.phase * 3.0 + tile as f32);
                let on_beat = if hash2(tile as u32, beats ^ 0x55) < 0.3 { bf } else { 0.0 };
                let k = ((0.32 + 0.5 * tw * (0.5 + self.treble)) * (0.6 + 0.8 * glints[tile].min(1.0))
                    + on_beat * 0.7)
                    * g;
                let sz = 1.0 + self.level * 2.0 + (bh * 0.004) + on_beat * 1.5;
                // A tilted square (the facet's shape, turned as the ball turns).
                let ang = lon * 0.5;
                let (ca, sa) = (cosf(ang), sinf(ang));
                let n_in = (sz * 2.0) as i32;
                for oy in -n_in..=n_in {
                    for ox in -n_in..=n_in {
                        let (u, v) = (ox as f32 * ca + oy as f32 * sa, -(ox as f32) * sa + oy as f32 * ca);
                        if fabsf(u) <= sz && fabsf(v) <= sz {
                            self.add(sx as i32 + ox, sy as i32 + oy, c, k);
                        }
                    }
                }
                // The bright ones twinkle: a cross of light through the spot.
                if k > 0.55 {
                    let arm = (sz * 2.5 + 2.0 + on_beat * 3.0) as i32;
                    for o in 1..=arm {
                        let f = k * 0.6 * (1.0 - o as f32 / (arm + 1) as f32);
                        self.add(sx as i32 + o, sy as i32, [255, 255, 255], f);
                        self.add(sx as i32 - o, sy as i32, [255, 255, 255], f);
                        self.add(sx as i32, sy as i32 + o, [255, 255, 255], f);
                        self.add(sx as i32, sy as i32 - o, [255, 255, 255], f);
                    }
                }
            }
        }
        // Sparkles drifting in the room, glinting as the ball's light passes.
        for i in 0..ROOM_SPARKS {
            let (hx, hy) = (hash2(i as u32, 21), hash2(i as u32, 22));
            let drift = self.phase * (0.02 + hx * 0.04);
            let (x, y) = (((hx + drift * 0.5) % 1.0) * bw, ((hy + drift * 0.3) % 1.0) * bh * 0.82);
            let tw = sinf(self.phase * (2.0 + hy * 5.0) * 3.0 + hx * 50.0).max(0.0);
            let k = tw * tw * tw * (0.35 + self.treble * 0.8 + bf * 0.5) * g;
            if k < 0.06 {
                continue;
            }
            let c = self.col(((hx * 255.0) as usize + hue_base) & 255);
            self.add(x as i32, y as i32, [255, 255, 255], k);
            let arm = (1.0 + tw * 2.5) as i32;
            for o in 1..=arm {
                let f = k * (1.0 - o as f32 / (arm + 1) as f32);
                self.add(x as i32 + o, y as i32, c, f);
                self.add(x as i32 - o, y as i32, c, f);
                self.add(x as i32, y as i32 + o, c, f);
                self.add(x as i32, y as i32 - o, c, f);
            }
        }
        // The chain and the ball.
        self.line_add(cx, 0.0, cx, cy - r, [90, 86, 110], 0.8, 0);
        for y in (cy - r) as i32..=(cy + r) as i32 {
            for x in (cx - r) as i32..=(cx + r) as i32 {
                let (px, py) = ((x as f32 - cx) / r, (y as f32 - cy) / r);
                let d2 = px * px + py * py;
                if d2 > 1.0 {
                    continue;
                }
                let pz = sqrtf(1.0 - d2);
                // Longitude and latitude on the sphere, spun, quantised into mirror tiles with a thin grout.
                let lon = libm::atan2f(px, pz) + spin;
                let lat = libm::asinf(py);
                let u = lon / TAU * nu as f32;
                let v = (lat / core::f32::consts::PI + 0.5) * nv as f32;
                let (fu, fv) = (u - floorf(u), v - floorf(v));
                let grout = fu < 0.08 || fv < 0.08;
                let iu = (floorf(u) as i32).rem_euclid(nu as i32) as usize;
                let iv = (floorf(v) as i32).clamp(0, nv as i32 - 1) as usize;
                let tile = iu * nv + iv;
                let h = hash2(tile as u32, 9);
                let lit = 0.25 + 0.75 * h * (0.5 + 0.5 * sinf(self.phase * 2.2 + h * 20.0));
                let shade = (0.25 + 0.75 * pz) * lit;
                let c = if grout { [20, 18, 30] } else { self.col(((h * 255.0) as usize + hue_base) & 255) };
                let gl = glints[tile];
                // A mirror: the tile's own light, plus a hard white-hot glint where it catches a light, brighter towards a tile's middle.
                let centre = 1.0 - ((fu - 0.5) * (fu - 0.5) + (fv - 0.5) * (fv - 0.5)) * 2.0;
                let k = if grout { 0.8 } else { (0.35 + shade * (0.7 + self.treble * 0.5)).min(1.2) * g };
                let i = (y.max(0) as usize).min(self.bh - 1) * self.bw + (x.max(0) as usize).min(self.bw - 1);
                let base = [self.buf[i * 4], self.buf[i * 4 + 1], self.buf[i * 4 + 2]];
                for ch in 0..3 {
                    if grout {
                        self.buf[i * 4 + ch] = c[ch];
                    } else {
                        let lit = (c[ch] as f32 * k + 24.0 * pz).max(base[ch] as f32 * 0.2);
                        let white = 255.0 * gl * centre.max(0.2);
                        self.buf[i * 4 + ch] = (lit + white).min(255.0) as u8;
                    }
                }
            }
        }
        // A bloom behind the ball that swells with the bass, and a specular glint.
        self.disc_add(cx, cy, r * 1.9, self.col((hue_base + 40) & 255), 0.10 + self.bass * 0.14 + bf * 0.10);
        self.disc_add(cx - r * 0.35, cy - r * 0.4, r * 0.18, [255, 255, 255], 0.6);
        let _ = fabsf(0.0);
        let _ = Effect::DiscoBall;
    }

    /// A translucent beam from (`x`, `y`) at `angle` (radians, 0 pointing right), `spread` wide, drawn additively with a fall-off along it.
    fn beam(&mut self, x: f32, y: f32, angle: f32, spread: f32, c: [u8; 3], k: f32) {
        let len = self.bh as f32 * 1.4;
        for step in 0..(len as i32 / 2) {
            let d = step as f32 * 2.0;
            let fall = 1.0 - d / len;
            let w = d * spread;
            let (ax, ay) = (x + cosf(angle) * d, y + sinf(angle) * d);
            let (nx, ny) = (-sinf(angle), cosf(angle));
            let mut o = -w;
            while o <= w {
                let edge = 1.0 - fabsf(o) / w.max(0.5);
                self.add((ax + nx * o) as i32, (ay + ny * o) as i32, c, k * fall * edge);
                o += 1.0;
            }
        }
    }

    /// The dance floor in perspective at the bottom of the picture, its tiles lighting in a cascade outward from the middle.
    fn dance_floor(&mut self, hue_base: usize, g: f32) {
        let (bw, bh) = (self.bw as f32, self.bh as f32);
        let horizon = bh * 0.62;
        let rows = 7;
        let cascade = self.scenes.cascade;
        for row in 0..rows {
            let f0 = row as f32 / rows as f32;
            let f1 = (row + 1) as f32 / rows as f32;
            // Perspective: rows get taller towards the viewer.
            let y0 = horizon + (bh - horizon) * f0 * f0;
            let y1 = horizon + (bh - horizon) * f1 * f1;
            let cols = 4 + row * 2;
            let spread = 0.35 + f1 * 0.9;
            for col in 0..cols {
                let u0 = col as f32 / cols as f32 - 0.5;
                let u1 = (col + 1) as f32 / cols as f32 - 0.5;
                let (x0, x1) = (bw * 0.5 + u0 * bw * spread * 2.0, bw * 0.5 + u1 * bw * spread * 2.0);
                // The cascade reaches a tile when its distance from the centre is within what has travelled.
                let dist = ((u0 + u1) * 0.5).abs() * 2.0 + (1.0 - f0) * 0.5;
                let reach = (cascade * 2.2 - dist).clamp(0.0, 1.0);
                let checker = (row + col) % 2 == 0;
                let base = if checker { 0.08 } else { 0.03 };
                let c = self.col((hue_base + row * 24 + col * 9) & 255);
                let band = self.bands[(col * 31 / cols.max(1)).min(31)];
                let k = (base + reach * 0.45 + band * 0.25) * g;
                self.rect_add(x0 as i32 + 1, y0 as i32, x1 as i32 - 1, y1 as i32 - 1, c, k);
            }
        }
    }
}
