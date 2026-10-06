//! The visualizer effects: spectrum bars, oscilloscope, tunnel, beat-reactive particles and plasma, drawn on the CPU into a
//! small RGBA picture that the UI scales up to the window. Driven by [`VizSummary`]s (bands, level, onset) and the latest
//! waveform, so they look the same in a browser, on the desktop and in Rusty Bucket.
//!
//! The look follows the Unicorn Viz demoscene spirit (neon bars with peak caps and a reflection, glowing scope trails, a
//! twisting tunnel, a starfield that bursts on the beat, sine plasma), written from scratch and kept cheap: effects draw at
//! half or quarter resolution, tables replace per-pixel trigonometry, and nothing allocates per frame once the buffers exist.
//!
//! **Motion safety.** Brightness changes on the beat are capped at 12 percent and at three a second (the general flash
//! threshold), nothing ever flashes the whole picture, and with `reduce_motion` the effects run calm: no beat pulses or bursts,
//! slow drift, heavy smoothing.
use alloc::vec::Vec;
use libm::{atan2f, cosf, powf, sinf, sqrtf};
use rvp_core::Timestamp;
use rvp_host::{VIZ_BANDS, VizSummary};
use theme::tokens as t;

/// Longest the picture may go between two brightness pulses on the beat, in microseconds (three a second).
const PULSE_GAP_US: Timestamp = 340_000;
/// Most the picture brightens on the beat (a fraction).
const PULSE_GAIN: f32 = 0.12;
/// Pixels the effect picture may have (the UI scales it to the window).
const MAX_PIXELS: usize = 560_000;

/// An effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Effect {
    /// Frequency bars with peak caps and a reflection.
    #[default]
    Spectrum,
    /// The waveform as a glowing line with trails.
    Scope,
    /// A twisting tunnel.
    Tunnel,
    /// A starfield that bursts on the beat.
    Particles,
    /// Sine plasma.
    Plasma,
}

/// Every effect, in the order the preset switcher steps through them.
pub const EFFECTS: [Effect; 5] =
    [Effect::Spectrum, Effect::Scope, Effect::Tunnel, Effect::Particles, Effect::Plasma];

impl Effect {
    /// Name for the switcher.
    pub fn name(self) -> &'static str {
        match self {
            Effect::Spectrum => "Spectrum",
            Effect::Scope => "Scope",
            Effect::Tunnel => "Tunnel",
            Effect::Particles => "Starfield",
            Effect::Plasma => "Plasma",
        }
    }

    /// The effect after (`+1`) or before (`-1`) this one, wrapping.
    pub fn step(self, d: i32) -> Effect {
        let i = EFFECTS.iter().position(|e| *e == self).unwrap_or(0) as i32;
        EFFECTS[(i + d).rem_euclid(EFFECTS.len() as i32) as usize]
    }

    /// How much smaller than the window the effect is drawn (a bigger number is cheaper and softer).
    fn divisor(self) -> usize {
        match self {
            Effect::Spectrum | Effect::Scope | Effect::Particles => 2,
            Effect::Tunnel | Effect::Plasma => 4,
        }
    }
}

/// A colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Palette {
    /// Magenta, violet and cyan: the Unicorn Tears gradient.
    #[default]
    Tears,
    /// Ink to magenta to amber.
    Sunset,
    /// Violet, cyan and lime.
    Aurora,
    /// The whole spectrum, red round to violet and back (bars run across it).
    Rainbow,
}

/// Every palette.
pub const PALETTES: [Palette; 4] = [Palette::Tears, Palette::Sunset, Palette::Aurora, Palette::Rainbow];

impl Palette {
    /// Name for the switcher.
    pub fn name(self) -> &'static str {
        match self {
            Palette::Tears => "Tears",
            Palette::Sunset => "Sunset",
            Palette::Aurora => "Aurora",
            Palette::Rainbow => "Rainbow",
        }
    }

    /// The palette after this one.
    pub fn next(self) -> Palette {
        let i = PALETTES.iter().position(|p| *p == self).unwrap_or(0);
        PALETTES[(i + 1) % PALETTES.len()]
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k]
}

fn rgb(c: theme::Rgba) -> [f32; 3] {
    [c.r as f32, c.g as f32, c.b as f32]
}

/// 256 colours around the scheme (it is cyclic, so a phase can wrap).
fn build_lut(p: Palette) -> [[u8; 3]; 256] {
    let stops: &[[f32; 3]] = match p {
        Palette::Tears => &[rgb(t::MAGENTA_500), rgb(t::VIOLET_500), rgb(t::CYAN_500), rgb(t::VIOLET_500)],
        Palette::Sunset => &[rgb(t::INK_600), rgb(t::MAGENTA_500), [255.0, 194.0, 77.0], rgb(t::MAGENTA_500)],
        Palette::Aurora => &[rgb(t::VIOLET_500), rgb(t::CYAN_500), rgb(t::LIME_500), rgb(t::CYAN_500)],
        // Saturated but not harsh: red, orange, yellow, green, cyan, blue, violet, magenta, and round again.
        Palette::Rainbow => &[
            [255.0, 59.0, 74.0],
            [255.0, 142.0, 36.0],
            [255.0, 214.0, 51.0],
            [86.0, 224.0, 92.0],
            [34.0, 211.0, 224.0],
            [66.0, 120.0, 255.0],
            [152.0, 96.0, 255.0],
            [236.0, 64.0, 200.0],
        ],
    };
    let mut lut = [[0u8; 3]; 256];
    for (i, c) in lut.iter_mut().enumerate() {
        let x = i as f32 / 256.0 * stops.len() as f32;
        let k = x as usize;
        let col = lerp3(stops[k % stops.len()], stops[(k + 1) % stops.len()], x - k as f32);
        *c = [col[0] as u8, col[1] as u8, col[2] as u8];
    }
    lut
}

/// What a frame needs besides the effect's own state.
#[derive(Debug, Clone, Copy)]
pub struct FrameInput<'a> {
    /// Host time of this frame, microseconds.
    pub now_us: Timestamp,
    /// Audio is playing: only then does anything move (a paused picture stays exactly as it is).
    pub playing: bool,
    /// Calm mode (reduced motion).
    pub reduce_motion: bool,
    /// The latest mono samples heard, newest last (for the scope); may be empty.
    pub scope: &'a [f32],
}

struct Particle {
    x: f32,
    y: f32,
    z: f32,
    vx: f32,
    vy: f32,
    life: f32,
    hue: u8,
    burst: bool,
}

/// The state of the visualizer: smoothed audio features, the effect's own memory and the picture.
pub struct Viz {
    /// The effect being drawn.
    pub effect: Effect,
    /// The colour scheme.
    pub palette: Palette,
    lut: [[u8; 3]; 256],
    lut_for: Palette,
    bands: [f32; VIZ_BANDS],
    peaks: [f32; VIZ_BANDS],
    level: f32,
    bass: f32,
    mid: f32,
    treble: f32,
    pulse: f32,
    last_pulse_us: Timestamp,
    last_us: Option<Timestamp>,
    phase: f32,
    spin: f32,
    buf: Vec<u8>,
    /// The night backdrop (the picture fades back towards it, so trails melt into the sky instead of into black).
    bg: Vec<u8>,
    bw: usize,
    bh: usize,
    built_for: Option<(Effect, usize, usize)>,
    parts: Vec<Particle>,
    rng: u32,
    /// Polar map of the tunnel: angle (0..256) and inverse radius (fixed point) per pixel.
    polar: Vec<(u8, u16)>,
    sin_lut: [i16; 256],
    frames: u64,
}

impl Default for Viz {
    fn default() -> Self {
        Self::new()
    }
}

impl Viz {
    /// A visualizer with the first effect and the Tears palette.
    pub fn new() -> Self {
        let mut sin_lut = [0i16; 256];
        for (i, s) in sin_lut.iter_mut().enumerate() {
            *s = (sinf(i as f32 / 256.0 * core::f32::consts::TAU) * 1024.0) as i16;
        }
        Self {
            effect: Effect::Spectrum,
            palette: Palette::Tears,
            lut: build_lut(Palette::Tears),
            lut_for: Palette::Tears,
            bands: [0.0; VIZ_BANDS],
            peaks: [0.0; VIZ_BANDS],
            level: 0.0,
            bass: 0.0,
            mid: 0.0,
            treble: 0.0,
            pulse: 0.0,
            last_pulse_us: -PULSE_GAP_US,
            last_us: None,
            phase: 0.0,
            spin: 0.0,
            buf: Vec::new(),
            bg: Vec::new(),
            bw: 0,
            bh: 0,
            built_for: None,
            parts: Vec::new(),
            rng: 0x1234_5678,
            polar: Vec::new(),
            sin_lut,
            frames: 0,
        }
    }

    /// Frames drawn so far (for tests).
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The picture: RGBA, `width * height * 4` bytes, and its size.
    pub fn picture(&self) -> (&[u8], usize, usize) {
        (&self.buf, self.bw, self.bh)
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Forget the audio history (a new item, a seek): the picture settles to quiet.
    pub fn reset_audio(&mut self) {
        self.bands = [0.0; VIZ_BANDS];
        self.peaks = [0.0; VIZ_BANDS];
        self.level = 0.0;
        self.bass = 0.0;
        self.mid = 0.0;
        self.treble = 0.0;
        self.pulse = 0.0;
    }

    /// Fold one summary of the audio being heard into the smoothed features.
    pub fn feed(&mut self, s: &VizSummary, reduce_motion: bool) {
        // One summary is 10.7 ms of audio at 48 kHz; attack is quick, decay a little over a tenth of a second.
        let (up, down) = if reduce_motion { (0.035, 0.012) } else { (0.55, 0.10) };
        let follow = |cur: &mut f32, new: f32| {
            let k = if new > *cur { up } else { down };
            *cur += (new - *cur) * k;
        };
        for i in 0..VIZ_BANDS {
            follow(&mut self.bands[i], s.bands[i]);
            // Peak caps hold and then fall.
            self.peaks[i] = if self.bands[i] >= self.peaks[i] {
                self.bands[i]
            } else {
                (self.peaks[i] - 0.0035).max(self.bands[i])
            };
        }
        follow(&mut self.level, s.level.min(1.0) * 2.0);
        follow(&mut self.bass, s.bass);
        follow(&mut self.mid, s.mid);
        follow(&mut self.treble, s.treble);
        if s.onset && !reduce_motion && s.pts_us - self.last_pulse_us >= PULSE_GAP_US {
            self.pulse = (0.4 + s.onset_strength.min(1.5) * 0.4).min(1.0);
            self.last_pulse_us = s.pts_us;
            if self.effect == Effect::Particles {
                self.burst(18 + (s.onset_strength.min(2.0) * 10.0) as usize);
            }
        }
    }

    fn burst(&mut self, n: usize) {
        for _ in 0..n {
            if self.parts.len() >= 520 {
                break;
            }
            let a = self.rand() * core::f32::consts::TAU;
            let sp = 0.25 + self.rand() * 0.8;
            let hue = (self.rand() * 255.0) as u8;
            self.parts.push(Particle {
                x: 0.0,
                y: 0.0,
                z: 1.0,
                vx: cosf(a) * sp,
                vy: sinf(a) * sp,
                life: 1.0,
                hue,
                burst: true,
            });
        }
    }

    fn col(&self, i: usize) -> [u8; 3] {
        self.lut[i & 255]
    }

    /// Draw a frame for a window of `w` x `h` pixels.
    pub fn render(&mut self, w: usize, h: usize, input: &FrameInput<'_>) {
        let div = self.effect.divisor();
        // Keep the picture within a pixel budget even on a huge window.
        let mut d = div;
        while (w / d).max(1) * (h / d).max(1) > MAX_PIXELS {
            d += 1;
        }
        let (bw, bh) = ((w / d).max(8), (h / d).max(8));
        if self.built_for != Some((self.effect, bw, bh)) {
            self.bw = bw;
            self.bh = bh;
            self.buf.clear();
            self.buf.resize(bw * bh * 4, 0);
            for p in self.buf.chunks_exact_mut(4) {
                p.copy_from_slice(&[7, 6, 13, 255]);
            }
            self.built_for = Some((self.effect, bw, bh));
            self.parts.clear();
            if matches!(self.effect, Effect::Tunnel | Effect::Plasma) {
                self.build_polar();
            }
            if matches!(self.effect, Effect::Scope | Effect::Particles) {
                self.backdrop(0.18);
                self.bg = self.buf.clone();
            }
        }
        if self.lut_for != self.palette {
            self.lut = build_lut(self.palette);
            self.lut_for = self.palette;
        }
        let dt = match self.last_us {
            Some(l) => ((input.now_us - l).clamp(0, 100_000)) as f32 / 1e6,
            None => 0.0,
        };
        self.last_us = Some(input.now_us);
        let calm = if input.reduce_motion { 0.25 } else { 1.0 };
        let moving = input.playing && dt > 0.0;
        if moving {
            self.phase += dt * (0.25 + self.level * 1.4 + self.bass * 0.8) * calm;
            self.spin += dt * (0.05 + self.mid * 0.5) * calm;
            self.pulse = (self.pulse - dt * 3.2).max(0.0);
        }
        // A paused picture is left exactly as it is.
        if !input.playing && self.frames > 0 {
            return;
        }
        match self.effect {
            Effect::Spectrum => self.draw_spectrum(),
            Effect::Scope => self.draw_scope(input.scope, calm),
            Effect::Tunnel => self.draw_tunnel(),
            Effect::Particles => self.draw_particles(dt, moving, input.reduce_motion),
            Effect::Plasma => self.draw_plasma(),
        }
        self.frames += 1;
    }

    fn gain(&self) -> f32 {
        1.0 + PULSE_GAIN * self.pulse
    }

    // ---- drawing primitives (additive, saturating) -------------------------------------------------------------------

    fn add(&mut self, x: i32, y: i32, c: [u8; 3], k: f32) {
        if x < 0 || y < 0 || x as usize >= self.bw || y as usize >= self.bh {
            return;
        }
        let i = (y as usize * self.bw + x as usize) * 4;
        for ch in 0..3 {
            self.buf[i + ch] = (self.buf[i + ch] as f32 + c[ch] as f32 * k).min(255.0) as u8;
        }
    }

    /// Fade the picture towards the backdrop: `k` of what was there stays.
    fn fade(&mut self, k: f32) {
        let m = (k * 256.0) as i32;
        if self.bg.len() != self.buf.len() {
            for p in self.buf.chunks_exact_mut(4) {
                for c in &mut p[..3] {
                    *c = ((*c as i32 * m) >> 8) as u8;
                }
            }
            return;
        }
        for (p, b) in self.buf.chunks_exact_mut(4).zip(self.bg.chunks_exact(4)) {
            for i in 0..3 {
                p[i] = (b[i] as i32 + (((p[i] as i32 - b[i] as i32) * m) >> 8)) as u8;
            }
        }
    }

    fn rect_add(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: [u8; 3], k: f32) {
        for y in y0.max(0)..y1.min(self.bh as i32) {
            for x in x0.max(0)..x1.min(self.bw as i32) {
                self.add(x, y, c, k);
            }
        }
    }

    fn disc_add(&mut self, cx: f32, cy: f32, r: f32, c: [u8; 3], k: f32) {
        let (x0, x1) = ((cx - r) as i32, (cx + r) as i32 + 1);
        let (y0, y1) = ((cy - r) as i32, (cy + r) as i32 + 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d =
                    sqrtf((x as f32 - cx) * (x as f32 - cx) + (y as f32 - cy) * (y as f32 - cy)) / r.max(0.5);
                if d < 1.0 {
                    let f = 1.0 - d;
                    self.add(x, y, c, k * f * f);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn line_add(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 3], k: f32, thick: i32) {
        let steps = (libm::fabsf(x1 - x0).max(libm::fabsf(y1 - y0)) as i32).max(1);
        for s in 0..=steps {
            let f = s as f32 / steps as f32;
            let (x, y) = (x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
            for oy in -thick..=thick {
                for ox in 0..=0 {
                    let fall = 1.0 / (1.0 + libm::fabsf(oy as f32));
                    self.add(x as i32 + ox, y as i32 + oy, c, k * fall);
                }
            }
        }
    }

    // ---- the effects ----------------------------------------------------------------------------------------------

    fn backdrop(&mut self, glow: f32) {
        // Night gradient with a faint violet bloom behind the middle that follows the bass.
        let (bw, bh) = (self.bw as f32, self.bh as f32);
        for y in 0..self.bh {
            let fy = y as f32 / bh;
            for x in 0..self.bw {
                let dx = (x as f32 / bw - 0.5) * 1.6;
                let dy = fy - 0.62;
                let bloom = (1.0 - (dx * dx + dy * dy) * 2.6).max(0.0);
                let k = bloom * bloom * glow;
                let i = (y * self.bw + x) * 4;
                self.buf[i] = (9.0 + 26.0 * k) as u8;
                self.buf[i + 1] = (7.0 + 8.0 * k) as u8;
                self.buf[i + 2] = (16.0 + 46.0 * k) as u8;
                self.buf[i + 3] = 255;
            }
        }
    }

    fn draw_spectrum(&mut self) {
        let g = self.gain();
        self.backdrop(0.35 + self.bass * 0.5);
        let (bw, bh) = (self.bw as i32, self.bh as i32);
        let margin = (bw / 14).max(6);
        let n = (((bw - 2 * margin) / 7).clamp(20, 64)) as usize;
        let slot = (bw - 2 * margin) as f32 / n as f32;
        let base = (bh as f32 * 0.74) as i32;
        let max_h = bh as f32 * 0.62;
        for i in 0..n {
            // Interpolate the 32 bands over the bars.
            let pos = i as f32 * (VIZ_BANDS - 1) as f32 / (n - 1) as f32;
            let (a, frac) = (pos as usize, pos - pos as usize as f32);
            let b = (a + 1).min(VIZ_BANDS - 1);
            let v = (self.bands[a] * (1.0 - frac) + self.bands[b] * frac).clamp(0.0, 1.0);
            let pk = (self.peaks[a] * (1.0 - frac) + self.peaks[b] * frac).clamp(0.0, 1.0);
            let hgt = powf(v, 1.25) * max_h;
            let x0 = margin + (i as f32 * slot) as i32;
            let x1 = margin + ((i as f32 + 1.0) * slot) as i32 - 1;
            // The bars use half the colour cycle (a calm gradient); the rainbow gives them the whole of it, red to violet.
            let col = if self.palette == Palette::Rainbow {
                self.col(i * 232 / n)
            } else {
                self.col(i * 256 / n / 2 + 8)
            };
            let top = base - hgt as i32;
            // Halo, then the body brighter towards the top, then the reflection.
            self.rect_add(x0 - 1, top - 2, x1 + 1, base, col, 0.10 * g);
            for y in top..base {
                let f = 0.45 + 0.55 * (base - y) as f32 / hgt.max(1.0);
                self.rect_add(x0, y, x1, y + 1, col, f * 0.9 * g);
            }
            let cap = base - (powf(pk, 1.25) * max_h) as i32;
            if pk > 0.03 {
                self.rect_add(x0, cap - 2, x1, cap, [255, 239, 251], 0.85);
            }
            for r in 0..((hgt * 0.35) as i32) {
                let f = 0.30 * (1.0 - r as f32 / (hgt * 0.35).max(1.0));
                self.rect_add(x0, base + 2 + r, x1, base + 3 + r, col, f * g);
            }
        }
        // The floor line (only while there is something standing on it).
        if self.peaks.iter().any(|&p| p > 0.03) {
            self.rect_add(margin - 2, base, bw - margin + 2, base + 1, [189, 131, 255], 0.5);
        }
    }

    fn draw_scope(&mut self, scope: &[f32], calm: f32) {
        let g = self.gain();
        self.fade(if calm < 1.0 { 0.9 } else { 0.8 });
        let (bw, bh) = (self.bw, self.bh);
        let mid = bh as f32 * 0.5;
        let amp = bh as f32 * 0.34 * (0.7 + self.level * 0.6).min(1.4);
        self.rect_add(0, mid as i32, bw as i32, mid as i32 + 1, [52, 35, 87], 0.9);
        if scope.len() < 32 {
            return;
        }
        // Take a stretch that starts at a rising zero crossing, so a steady tone stands still.
        let span = (scope.len() / 2).min(1024);
        let mut start = scope.len() - span;
        for i in (0..scope.len() - span).rev().take(scope.len() / 2) {
            if scope[i] <= 0.0 && scope[i + 1] > 0.0 {
                start = i;
                break;
            }
        }
        let n = bw;
        let mut prev: Option<(f32, f32)> = None;
        for x in 0..n {
            let pos = start as f32 + x as f32 * (span as f32 - 1.0) / n as f32;
            let i = (pos as usize).min(scope.len() - 2);
            let f = pos - i as f32;
            let s = scope[i] * (1.0 - f) + scope[i + 1] * f;
            let y = mid - s.clamp(-1.0, 1.0) * amp;
            let c = self.col(x * 256 / n + 20);
            if let Some((px, py)) = prev {
                // A wide dim halo, the coloured line, and a thin hot core.
                // (The trails add up, so each pass is faint.)
                self.line_add(px, py, x as f32, y, c, 0.07 * g, 4);
                self.line_add(px, py, x as f32, y, c, 0.30 * g, 2);
                self.line_add(px, py, x as f32, y, [255, 239, 251], 0.10, 0);
            }
            prev = Some((x as f32, y));
        }
    }

    fn build_polar(&mut self) {
        let (bw, bh) = (self.bw, self.bh);
        self.polar.clear();
        let (cx, cy) = (bw as f32 * 0.5, bh as f32 * 0.5);
        let scale = bh as f32 * 0.5;
        for y in 0..bh {
            for x in 0..bw {
                let (dx, dy) = ((x as f32 + 0.5 - cx) / scale, (y as f32 + 0.5 - cy) / scale);
                let r = sqrtf(dx * dx + dy * dy).max(0.001);
                let a = atan2f(dy, dx) / core::f32::consts::TAU + 0.5;
                let inv = (1.0 / r * 256.0).clamp(0.0, 65_535.0);
                self.polar.push(((a * 255.99) as u8, inv as u16));
            }
        }
    }

    fn draw_tunnel(&mut self) {
        let g = self.gain();
        let depth = (self.phase * 2600.0) as u32;
        let twist = (self.spin * 256.0 * 3.0) as u32;
        let hue = (self.phase * 18.0) as u32;
        let bright = (0.55 + 0.45 * (self.level * 0.5 + self.bass * 0.7).min(1.0)) * g;
        let (bw, bh) = (self.bw, self.bh);
        for y in 0..bh {
            for x in 0..bw {
                let (ang, inv) = self.polar[y * bw + x];
                let u = (ang as u32).wrapping_mul(2).wrapping_add(twist);
                let v = (inv as u32).wrapping_add(depth);
                // Rings and spokes: a checker of the two, with a soft edge from the sine table.
                let ring = self.sin_lut[((v >> 1) & 255) as usize] as i32;
                let spoke = self.sin_lut[((u.wrapping_mul(3)) & 255) as usize] as i32;
                let pat = ((ring * spoke) >> 11) + 512; // 0..1024 roughly
                let idx = ((v >> 5).wrapping_add(hue)) as usize + (pat as usize >> 3);
                let c = self.lut[idx & 255];
                // Fog: dark in the middle (far away), full at the rim.
                let fog = (1.0 - 1.0 / (1.0 + inv as f32 / 3600.0)).clamp(0.0, 1.0);
                let k = ((pat as f32 / 1024.0).clamp(0.15, 1.0) * (1.0 - fog * 0.85) * bright).min(1.0);
                let i = (y * bw + x) * 4;
                self.buf[i] = (c[0] as f32 * k) as u8;
                self.buf[i + 1] = (c[1] as f32 * k) as u8;
                self.buf[i + 2] = (c[2] as f32 * k) as u8;
                self.buf[i + 3] = 255;
            }
        }
    }

    fn draw_particles(&mut self, dt: f32, moving: bool, reduce_motion: bool) {
        let g = self.gain();
        self.fade(if reduce_motion { 0.92 } else { 0.88 });
        let (bw, bh) = (self.bw as f32, self.bh as f32);
        let (cx, cy) = (bw * 0.5, bh * 0.5);
        let scale = bh * 0.55;
        // A calm field of stars at all times; more of them and faster when the music is loud.
        while self.parts.iter().filter(|p| !p.burst).count() < 230 {
            let a = self.rand() * core::f32::consts::TAU;
            let r = 0.05 + self.rand() * 1.4;
            let z = 0.2 + self.rand() * 1.8;
            let hue = (self.rand() * 255.0) as u8;
            self.parts.push(Particle {
                x: cosf(a) * r,
                y: sinf(a) * r,
                z,
                vx: 0.0,
                vy: 0.0,
                life: 1.0,
                hue,
                burst: false,
            });
        }
        let speed = (0.12 + self.level * 0.55 + self.bass * 0.5) * if reduce_motion { 0.25 } else { 1.0 };
        let mut parts = core::mem::take(&mut self.parts);
        for p in &mut parts {
            if moving {
                if p.burst {
                    p.x += p.vx * dt;
                    p.y += p.vy * dt;
                    p.vx *= 1.0 - dt * 0.8;
                    p.vy *= 1.0 - dt * 0.8;
                    p.life -= dt * 0.7;
                } else {
                    p.z -= dt * speed;
                    if p.z < 0.05 {
                        p.z += 2.0;
                        p.life = 1.0;
                    }
                }
            }
            let (sx, sy) = if p.burst { (p.x, p.y) } else { (p.x / p.z, p.y / p.z) };
            let (px, py) = (cx + sx * scale, cy + sy * scale * 0.9);
            let near = if p.burst { p.life.max(0.0) } else { (1.0 - p.z * 0.5).clamp(0.0, 1.0) };
            let c = self.col(p.hue as usize);
            let r = if p.burst { 2.0 + p.life.max(0.0) * 2.4 } else { 1.0 + near * 2.4 };
            self.disc_add(px, py, r, c, (0.5 + near * 0.9) * g);
        }
        parts.retain(|p| !(p.burst && p.life <= 0.0));
        self.parts = parts;
        // A soft heart in the middle that swells with the bass (smoothed, never a flash).
        let c = self.col(((self.phase * 20.0) as usize) & 255);
        self.disc_add(cx, cy, 8.0 + self.bass * 30.0, c, 0.3 + self.bass * 0.4);
    }

    fn draw_plasma(&mut self) {
        let g = self.gain();
        let (bw, bh) = (self.bw, self.bh);
        let t = (self.phase * 70.0) as u32;
        let f1 = 3 + (self.bass * 5.0) as u32;
        let f2 = 4 + (self.mid * 6.0) as u32;
        let f3 = 2 + (self.treble * 7.0) as u32;
        let bright = (0.6 + 0.4 * self.level.min(1.0)) * g;
        for y in 0..bh {
            let yy = (y * 256 / bh) as u32;
            for x in 0..bw {
                let xx = (x * 256 / bw) as u32;
                let a = self.sin_lut[((xx * f1 + t) & 255) as usize] as i32;
                let b = self.sin_lut[((yy * f2 + t * 2) & 255) as usize] as i32;
                let c = self.sin_lut[(((xx + yy) * f3 + t * 3) & 255) as usize] as i32;
                let d = self.sin_lut[(((xx.wrapping_sub(yy)) / 2 + t) & 255) as usize] as i32;
                let v = a + b + c + d; // -4096..4096
                let idx = ((v + 4096) >> 5) as usize + (self.phase * 12.0) as usize;
                let col = self.lut[idx & 255];
                let k = (0.45 + (v + 4096) as f32 / 16_384.0) * bright;
                let i = (y * bw + x) * 4;
                self.buf[i] = (col[0] as f32 * k).min(255.0) as u8;
                self.buf[i + 1] = (col[1] as f32 * k).min(255.0) as u8;
                self.buf[i + 2] = (col[2] as f32 * k).min(255.0) as u8;
                self.buf[i + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn summary(pts: i64, bands: f32, onset: bool) -> VizSummary {
        VizSummary {
            pts_us: pts,
            level: bands * 0.5,
            peak: bands,
            bands: [bands; VIZ_BANDS],
            bass: bands,
            mid: bands,
            treble: bands,
            onset,
            onset_strength: 1.0,
            tempo_bpm: 120.0,
        }
    }

    fn frame(v: &mut Viz, now: i64, playing: bool, calm: bool, scope: &[f32]) {
        v.render(320, 180, &FrameInput { now_us: now, playing, reduce_motion: calm, scope });
    }

    fn hash(v: &Viz) -> u64 {
        v.picture()
            .0
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
    }

    #[test]
    fn every_effect_moves_with_the_music_and_rests_when_paused() {
        let scope: Vec<f32> = (0..2048).map(|i| sinf(i as f32 * 0.12) * 0.6).collect();
        for effect in EFFECTS {
            let mut v = Viz::new();
            v.effect = effect;
            let mut now = 0;
            let mut hashes = Vec::new();
            for f in 0..40 {
                // Louder and quieter passages with a beat every ten frames.
                let loud = 0.55 + 0.4 * sinf(f as f32 * 0.9);
                for k in 0..2 {
                    v.feed(&summary(now + k * 10_000, loud, f % 10 == 0 && k == 0), false);
                }
                frame(&mut v, now, true, false, &scope);
                hashes.push(hash(&v));
                now += 33_000;
            }
            let distinct = {
                let mut h = hashes.clone();
                h.sort();
                h.dedup();
                h.len()
            };
            assert!(distinct > 30, "{effect:?}: only {distinct} different pictures in 40 frames");
            // Paused: the same picture however long it is asked for.
            let still = hash(&v);
            for _ in 0..5 {
                now += 33_000;
                frame(&mut v, now, false, false, &scope);
                assert_eq!(hash(&v), still, "{effect:?} moved while paused");
            }
            // Picture size follows the window and the budget.
            let (_, w, h) = v.picture();
            assert!(w * h > 1000 && w * h <= MAX_PIXELS);
        }
    }

    #[test]
    fn pictures_are_lit_by_loud_audio_and_dark_when_silent() {
        let bright = |v: &Viz| -> u64 {
            v.picture().0.chunks_exact(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum()
        };
        for effect in [Effect::Spectrum, Effect::Tunnel, Effect::Plasma] {
            let (mut quiet, mut loud) = (Viz::new(), Viz::new());
            quiet.effect = effect;
            loud.effect = effect;
            let mut now = 0;
            for _ in 0..30 {
                quiet.feed(&summary(now, 0.0, false), false);
                loud.feed(&summary(now, 0.95, false), false);
                frame(&mut quiet, now, true, false, &[]);
                frame(&mut loud, now, true, false, &[]);
                now += 33_000;
            }
            assert!(
                bright(&loud) > bright(&quiet),
                "{effect:?}: loud {} quiet {}",
                bright(&loud),
                bright(&quiet)
            );
        }
    }

    #[test]
    fn beat_pulses_are_capped_at_three_a_second_and_twelve_percent() {
        let mut v = Viz::new();
        let mut pulses = 0;
        let mut max_gain = 1.0f32;
        // An onset in every summary (a pathological, 94 a second): pulses must still be at most three a second.
        let mut last = 0.0;
        for i in 0..940 {
            v.feed(&summary(i * 10_700, 0.9, true), false);
            if v.pulse > last + 0.1 {
                pulses += 1;
            }
            last = v.pulse;
            max_gain = max_gain.max(v.gain());
            v.pulse = (v.pulse - 0.034).max(0.0); // what render would decay by in 10.7 ms of motion
        }
        assert!(pulses <= 31, "{pulses} pulses in 10 s");
        assert!(max_gain <= 1.0 + PULSE_GAIN + 1e-4, "{max_gain}");
    }

    #[test]
    fn calm_mode_has_no_pulses_or_bursts() {
        let mut v = Viz::new();
        v.effect = Effect::Particles;
        for i in 0..200 {
            v.feed(&summary(i * 10_700, 0.9, true), true);
        }
        assert_eq!(v.pulse, 0.0);
        frame(&mut v, 0, true, true, &[]);
        assert!(v.parts.iter().all(|p| !p.burst));
    }

    #[test]
    fn effects_and_palettes_step_around() {
        assert_eq!(Effect::Spectrum.step(-1), Effect::Plasma);
        assert_eq!(Effect::Plasma.step(1), Effect::Spectrum);
        assert_eq!(EFFECTS.len(), 5);
        assert_eq!(Palette::Tears.next().next().next().next(), Palette::Tears);
        assert_eq!(Palette::Aurora.next(), Palette::Rainbow);
        // The rainbow runs through every hue and its ends meet (it is cyclic), so bars across it show red to violet.
        let rb = build_lut(Palette::Rainbow);
        assert!(rb[0][0] > 200 && rb[0][1] < 100, "starts red: {:?}", rb[0]);
        let hues = [0usize, 64, 100, 136, 170, 210];
        assert!(hues.windows(2).all(|w| rb[w[0]] != rb[w[1]]));
        let dist =
            |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).map(|(x, y)| (*x as i32 - y as i32).abs()).sum::<i32>();
        assert!(dist(rb[255], rb[0]) < 40, "the last colour leads back into the first");
        let lut = build_lut(Palette::Tears);
        assert_eq!(lut[0], [255, 43, 214]);
        assert!(lut.iter().all(|c| c.iter().any(|&v| v > 0)));
        let _ = vec![0u8; 1];
    }
}
