//! Audio analysis for visualizers (and, from M10, the visualizer effects): turns PCM into [`VizSummary`]s.
//!
//! Per hop of 512 frames (10.7 ms at 48 kHz, so about 94 summaries a second) the [`Analyzer`] reports the level,
//! 32 log-spaced spectrum bands (a 2048-point Hann FFT), bass/mid/treble, an **onset** flag from the spectral flux of
//! the hop's own 512-point spectrum (adaptive median/MAD threshold, so a drum hit is flagged in the hop it begins
//! in, with a refractory period) and a **tempo** estimate from the autocorrelation of the flux over the last
//! few seconds. Everything is plain `f32` math on a mono mix; no allocation per hop.
//!
//! The ideas (audio-reactive effects driven by FFT bands, adaptive onset thresholds and autocorrelation tempo) are
//! the ones in the Unicorn Viz project; the code here is written from scratch.
#![no_std]
#![forbid(unsafe_code)]
// Signal-processing loops index several buffers at once; iterator rewrites read worse.
#![allow(clippy::needless_range_loop)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod effects;

pub use effects::{EFFECTS, Effect, FrameInput, PALETTES, Palette, Viz};

use alloc::vec::Vec;
use rvp_core::Timestamp;
use rvp_host::{VIZ_BANDS, VizSummary};

/// Frames per analysis hop.
pub const HOP: usize = 512;
/// Window of the spectrum (bands).
const N_SPEC: usize = 2048;
/// Hops of onset flux kept for the tempo estimate (about 6.4 s).
const FLUX_HISTORY: usize = 600;
/// Hops of flux used for the adaptive threshold (about 0.7 s).
const THRESH_WINDOW: usize = 64;
/// Hops after an onset in which no other is reported (about 64 ms).
const REFRACTORY: usize = 6;
/// Tempo search range, beats per minute.
const MIN_BPM: f32 = 60.0;
const MAX_BPM: f32 = 200.0;
/// Lowest and highest band centre edges, Hz.
const F_LOW: f32 = 30.0;
const F_HIGH: f32 = 16_000.0;

/// An in-place radix-2 complex FFT of a fixed size.
struct Fft {
    n: usize,
    rev: Vec<u16>,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Fft {
    fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n <= 1 << 15);
        let bits = n.trailing_zeros();
        let rev = (0..n).map(|i| (i.reverse_bits() >> (usize::BITS - bits)) as u16).collect();
        let (cos, sin) = (0..n / 2)
            .map(|k| {
                let a = -2.0 * core::f64::consts::PI * k as f64 / n as f64;
                (libm::cos(a) as f32, libm::sin(a) as f32)
            })
            .unzip();
        Self { n, rev, cos, sin }
    }

    fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }
}

fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (0.5 - 0.5 * libm::cos(2.0 * core::f64::consts::PI * i as f64 / n as f64)) as f32)
        .collect()
}

/// Streaming analyzer. Feed PCM with [`Analyzer::process`] in any chunk sizes; call [`Analyzer::reset`] after a
/// seek.
pub struct Analyzer {
    sr: u32,
    spec_fft: Fft,
    flux_fft: Fft,
    spec_win: Vec<f32>,
    flux_win: Vec<f32>,
    /// Bin ranges of the bands in the 2048-point spectrum.
    bands: [(usize, usize); VIZ_BANDS],
    /// Mono samples not yet forming a whole hop.
    pending: Vec<f32>,
    /// The last `N_SPEC` mono samples.
    ring: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    prev_mag: Vec<f32>,
    flux: Vec<f32>,
    since_onset: usize,
    hops: u64,
    base_pts: Option<Timestamp>,
    /// Frames consumed since `base_pts`.
    consumed: u64,
    tempo: f32,
    since_tempo: usize,
    scratch: Vec<f32>,
}

impl Analyzer {
    /// An analyzer for audio at `sample_rate` Hz.
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8_000);
        let nyq = sr as f32 * 0.5;
        let f_high = F_HIGH.min(nyq * 0.92);
        let mut bands = [(0usize, 0usize); VIZ_BANDS];
        for (i, b) in bands.iter_mut().enumerate() {
            let lo = F_LOW * libm::powf(f_high / F_LOW, i as f32 / VIZ_BANDS as f32);
            let hi = F_LOW * libm::powf(f_high / F_LOW, (i + 1) as f32 / VIZ_BANDS as f32);
            let bin = |f: f32| (f * N_SPEC as f32 / sr as f32) as usize;
            let (l, h) = (bin(lo).max(1), bin(hi).max(1));
            *b = (l, h.max(l).min(N_SPEC / 2 - 1));
        }
        Self {
            sr,
            spec_fft: Fft::new(N_SPEC),
            flux_fft: Fft::new(HOP),
            spec_win: hann(N_SPEC),
            flux_win: hann(HOP),
            bands,
            pending: Vec::new(),
            ring: alloc::vec![0.0; N_SPEC],
            re: alloc::vec![0.0; N_SPEC],
            im: alloc::vec![0.0; N_SPEC],
            prev_mag: alloc::vec![0.0; HOP / 2 + 1],
            flux: Vec::new(),
            since_onset: REFRACTORY,
            hops: 0,
            base_pts: None,
            consumed: 0,
            tempo: 0.0,
            since_tempo: 0,
            scratch: Vec::new(),
        }
    }

    /// The sample rate this analyzer was made for.
    pub fn sample_rate(&self) -> u32 {
        self.sr
    }

    /// Forget all history (a seek, a new item).
    pub fn reset(&mut self) {
        self.pending.clear();
        self.ring.iter_mut().for_each(|v| *v = 0.0);
        self.prev_mag.iter_mut().for_each(|v| *v = 0.0);
        self.flux.clear();
        self.since_onset = REFRACTORY;
        self.hops = 0;
        self.base_pts = None;
        self.consumed = 0;
        self.tempo = 0.0;
        self.since_tempo = 0;
    }

    /// Analyse interleaved `channels`-channel audio whose first frame is at stream time `pts_us`; append a summary
    /// to `out` for every completed hop.
    pub fn process(
        &mut self,
        samples: &[f32],
        channels: usize,
        pts_us: Timestamp,
        out: &mut Vec<VizSummary>,
    ) {
        let ch = channels.max(1);
        if self.base_pts.is_none() {
            self.base_pts = Some(pts_us);
            self.consumed = 0;
        }
        // Per-channel peak of what arrives is tracked inside the hop; keep the mono mix and the peak together.
        for f in samples.chunks_exact(ch) {
            let mono = f.iter().sum::<f32>() / ch as f32;
            self.pending.push(mono);
            // Peak uses the loudest channel.
            let p = f.iter().fold(0.0f32, |a, v| a.max(v.abs()));
            self.scratch.push(p);
        }
        while self.pending.len() >= HOP {
            let hop: Vec<f32> = self.pending.drain(..HOP).collect();
            let peaks: Vec<f32> = self.scratch.drain(..HOP).collect();
            let summary = self.hop(&hop, &peaks);
            out.push(summary);
        }
    }

    fn hop(&mut self, hop: &[f32], peaks: &[f32]) -> VizSummary {
        let pts =
            self.base_pts.unwrap_or(0) + (self.consumed as i128 * 1_000_000 / self.sr as i128) as Timestamp;
        self.consumed += HOP as u64;
        self.hops += 1;
        let level = libm::sqrtf(hop.iter().map(|v| v * v).sum::<f32>() / HOP as f32).min(1.0);
        let peak = peaks.iter().fold(0.0f32, |a, &v| a.max(v)).min(1.0);

        // Spectrum bands from the last 2048 samples.
        self.ring.rotate_left(HOP);
        let tail = N_SPEC - HOP;
        self.ring[tail..].copy_from_slice(hop);
        for i in 0..N_SPEC {
            self.re[i] = self.ring[i] * self.spec_win[i];
            self.im[i] = 0.0;
        }
        self.spec_fft.forward(&mut self.re, &mut self.im);
        let mut bands = [0.0f32; VIZ_BANDS];
        let norm = 4.0 / N_SPEC as f32; // a full-scale sine reads 1.0
        for (b, &(lo, hi)) in bands.iter_mut().zip(&self.bands) {
            let mut m = 0.0f32;
            for k in lo..=hi {
                m = m.max(libm::sqrtf(self.re[k] * self.re[k] + self.im[k] * self.im[k]) * norm);
            }
            let db = 20.0 * libm::log10f(m.max(1e-7));
            *b = ((db + 70.0) / 70.0).clamp(0.0, 1.0);
        }
        let mean = |s: &[f32]| s.iter().sum::<f32>() / s.len() as f32;
        let (bass, mid, treble) = (mean(&bands[..8]), mean(&bands[8..24]), mean(&bands[24..]));

        // Onset: spectral flux of 512-sample windows. Two per hop, one aligned with the hop and one centred on its
        // start, so a click gets a window that sees it in the middle wherever it falls; the larger flux counts.
        let n = N_SPEC;
        let centred: Vec<f32> = self.ring[n - HOP - HOP / 2..n - HOP / 2].to_vec();
        let f_centred = self.flux_of(&centred);
        let f_aligned = self.flux_of(hop);
        let flux = f_centred.max(f_aligned);
        let (onset, strength) = self.detect(flux);
        self.flux.push(flux);
        if self.flux.len() > FLUX_HISTORY {
            self.flux.remove(0);
        }
        self.since_tempo += 1;
        if self.since_tempo >= 24 && self.flux.len() >= 300 {
            self.since_tempo = 0;
            if let Some(t) = self.estimate_tempo() {
                self.tempo = t;
            }
        }
        VizSummary {
            pts_us: pts,
            level,
            peak,
            bands,
            bass,
            mid,
            treble,
            onset,
            onset_strength: strength,
            tempo_bpm: self.tempo,
        }
    }

    /// Spectral flux of a 512-sample window against the previous window (positive log-magnitude differences).
    fn flux_of(&mut self, win: &[f32]) -> f32 {
        let (re, im) = (&mut self.re[..HOP], &mut self.im[..HOP]);
        for i in 0..HOP {
            re[i] = win[i] * self.flux_win[i];
            im[i] = 0.0;
        }
        self.flux_fft.forward(re, im);
        let scale = 4.0 / HOP as f32;
        let mut flux = 0.0f32;
        for k in 0..=HOP / 2 {
            let m = libm::sqrtf(re[k] * re[k] + im[k] * im[k]) * scale;
            let lm = libm::logf(1.0 + 200.0 * m);
            let d = lm - self.prev_mag[k];
            if d > 0.0 {
                flux += d;
            }
            self.prev_mag[k] = lm;
        }
        flux / (HOP / 2) as f32
    }

    /// Adaptive threshold: median plus a few median absolute deviations of the recent flux.
    fn detect(&mut self, flux: f32) -> (bool, f32) {
        self.since_onset += 1;
        let n = self.flux.len().min(THRESH_WINDOW);
        let prev = self.flux.last().copied().unwrap_or(0.0);
        if n < 8 {
            return (false, 0.0);
        }
        let mut w: Vec<f32> = self.flux[self.flux.len() - n..].to_vec();
        w.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let med = w[n / 2];
        let mut dev: Vec<f32> = w.iter().map(|v| (v - med).abs()).collect();
        dev.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let mad = dev[n / 2];
        let thr = (med + 4.0 * 1.4826 * mad).max(0.03);
        if flux > thr && flux >= prev && self.since_onset >= REFRACTORY {
            self.since_onset = 0;
            (true, flux / thr)
        } else {
            (false, 0.0)
        }
    }

    /// Tempo from the autocorrelation of the flux (60 to 200 bpm), with a mild preference for 120.
    fn estimate_tempo(&self) -> Option<f32> {
        let hop_s = HOP as f32 / self.sr as f32;
        let (lmin, lmax) = ((60.0 / MAX_BPM / hop_s) as usize, (60.0 / MIN_BPM / hop_s) as usize + 1);
        let n = self.flux.len();
        if n < lmax * 2 {
            return None;
        }
        // Onset envelope: flux minus a local mean, half-wave rectified.
        let mut e: Vec<f32> = Vec::with_capacity(n);
        for i in 0..n {
            let (a, b) = (i.saturating_sub(16), (i + 17).min(n));
            let m = self.flux[a..b].iter().sum::<f32>() / (b - a) as f32;
            e.push((self.flux[i] - m).max(0.0));
        }
        // A little smoothing so an onset that lands between two hops counts for both.
        let mut es = e.clone();
        for i in 1..n - 1 {
            es[i] = 0.25 * e[i - 1] + 0.5 * e[i] + 0.25 * e[i + 1];
        }
        let e = es;
        let energy: f32 = e.iter().map(|v| v * v).sum();
        if energy < 1e-6 {
            return None;
        }
        let mut r = alloc::vec![0.0f32; lmax + 2];
        for (l, slot) in r.iter_mut().enumerate().skip(lmin.saturating_sub(1)) {
            let mut s = 0.0;
            for i in 0..n - l {
                s += e[i] * e[i + l];
            }
            *slot = s / (n - l) as f32;
        }
        let mut best = (0.0f32, 0usize);
        for l in lmin..=lmax {
            let bpm = 60.0 / (l as f32 * hop_s);
            let prior = libm::expf(-0.5 * libm::powf(libm::log2f(bpm / 120.0) / 1.2, 2.0));
            let score = r[l] * prior;
            if score > best.0 {
                best = (score, l);
            }
        }
        let mut l = best.1;
        if l == 0 || best.0 <= 0.0 {
            return None;
        }
        // A train of beats correlates at every multiple of the beat: if half this lag is nearly as strong, the
        // beat is the shorter one (150 bpm must not be reported as 75).
        let half = l / 2;
        if half >= lmin.max(2) {
            let (hl, hv) = (half - 1..=half + 1)
                .map(|k| (k, r[k]))
                .fold((0, 0.0f32), |a, b| if b.1 > a.1 { b } else { a });
            if hv >= 0.5 * r[l] {
                l = hl;
            }
        }
        // Parabolic interpolation of the lag.
        let (a, b, c) = (r[l - 1], r[l], r[l + 1]);
        let denom = a - 2.0 * b + c;
        let frac = if denom.abs() > 1e-12 { 0.5 * (a - c) / denom } else { 0.0 };
        let lag = l as f32 + frac.clamp(-1.0, 1.0);
        Some(60.0 / (lag * hop_s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn run(a: &mut Analyzer, mono: &[f32], chunk: usize) -> Vec<VizSummary> {
        let mut out = Vec::new();
        for c in mono.chunks(chunk) {
            a.process(c, 1, 0, &mut out);
        }
        out
    }

    fn sine(freq: f32, sr: u32, secs: f32, amp: f32) -> Vec<f32> {
        (0..(sr as f32 * secs) as usize)
            .map(|i| amp * libm::sinf(2.0 * core::f32::consts::PI * freq * i as f32 / sr as f32))
            .collect()
    }

    /// The bands whose bin range contains `f` Hz (at low frequencies neighbouring bands share bins).
    fn bands_of(f: f32, sr: u32) -> Vec<usize> {
        let bin = (f * N_SPEC as f32 / sr as f32 + 0.5) as usize;
        let a = Analyzer::new(sr);
        (0..VIZ_BANDS).filter(|&i| bin + 1 >= a.bands[i].0 && bin <= a.bands[i].1 + 1).collect()
    }

    #[test]
    fn a_sine_lights_its_own_band() {
        for f in [110.0, 1000.0, 5000.0] {
            let mut a = Analyzer::new(48_000);
            let s = run(&mut a, &sine(f, 48_000, 1.0, 0.8), 1000);
            let last = s.last().unwrap();
            let want = bands_of(f, 48_000);
            let loudest =
                (0..VIZ_BANDS).max_by(|&x, &y| last.bands[x].partial_cmp(&last.bands[y]).unwrap()).unwrap();
            assert!(want.contains(&loudest), "{f} Hz: band {loudest} is loudest, expected one of {want:?}");
            assert!(last.bands[loudest] > 0.9, "{f} Hz reads {}", last.bands[loudest]);
            // Bands well away from the tone are far quieter.
            for (i, v) in last.bands.iter().enumerate() {
                if want.iter().all(|w| i.abs_diff(*w) >= 3) {
                    assert!(*v < 0.6, "{f} Hz leaks {v} into band {i}");
                }
            }
            let avg = s[s.len() - 30..].iter().map(|v| v.level).sum::<f32>() / 30.0;
            assert!((avg - 0.8 / 2f32.sqrt()).abs() < 0.02, "level {avg}");
            assert!((last.peak - 0.8).abs() < 0.01);
        }
    }

    #[test]
    fn silence_is_silent() {
        let mut a = Analyzer::new(44_100);
        let s = run(&mut a, &vec![0.0; 44_100 * 2], 4410);
        assert!(s.iter().all(|x| x.level == 0.0 && !x.onset && x.tempo_bpm == 0.0));
        assert!(s.iter().all(|x| x.bands.iter().all(|&b| b == 0.0)));
    }

    /// Short clicks every 500 ms (120 bpm) over a quiet noise floor.
    fn clicks(bpm: f32, sr: u32, secs: f32) -> (Vec<f32>, Vec<usize>) {
        let n = (sr as f32 * secs) as usize;
        let mut x = vec![0.0f32; n];
        let mut seed = 12345u32;
        for v in &mut x {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *v = ((seed >> 16) as f32 / 32768.0 - 1.0) * 0.002;
        }
        let period = 60.0 / bpm * sr as f32;
        let mut at = Vec::new();
        let mut t = period;
        while (t as usize) + 300 < n {
            let i = t as usize;
            for j in 0..200 {
                x[i + j] += 0.8 * libm::expf(-(j as f32) / 40.0) * if j % 2 == 0 { 1.0 } else { -0.6 };
            }
            at.push(i);
            t += period;
        }
        (x, at)
    }

    #[test]
    fn clicks_are_onsets_at_the_right_hop_and_the_tempo_is_found() {
        for bpm in [90.0f32, 120.0, 150.0] {
            let sr = 48_000;
            let (x, at) = clicks(bpm, sr, 12.0);
            let mut a = Analyzer::new(sr);
            let s = run(&mut a, &x, 777);
            let onsets: Vec<i64> = s.iter().filter(|v| v.onset).map(|v| v.pts_us).collect();
            let want: Vec<i64> = at.iter().map(|&i| (i as i64) * 1_000_000 / sr as i64).collect();
            let hop_us = (HOP as i64) * 1_000_000 / sr as i64;
            // Every click is flagged within one hop, and nothing else is.
            assert_eq!(
                onsets.len(),
                want.len(),
                "{bpm} bpm: {} onsets for {} clicks",
                onsets.len(),
                want.len()
            );
            for (o, w) in onsets.iter().zip(&want) {
                assert!((o - w).abs() <= hop_us, "{bpm} bpm: onset at {o} us, click at {w} us");
            }
            let tempo = s.last().unwrap().tempo_bpm;
            assert!((tempo - bpm).abs() < 2.0, "{bpm} bpm estimated as {tempo}");
        }
    }

    #[test]
    fn chunking_does_not_change_the_result_and_reset_starts_over() {
        let (x, _) = clicks(120.0, 48_000, 4.0);
        let a1 = run(&mut Analyzer::new(48_000), &x, 4800);
        let a2 = run(&mut Analyzer::new(48_000), &x, 333);
        assert_eq!(a1.len(), a2.len());
        assert!(a1.iter().zip(&a2).all(|(p, q)| p == q));
        let mut an = Analyzer::new(48_000);
        let first = run(&mut an, &x, 1024);
        an.reset();
        let second = run(&mut an, &x, 1024);
        assert_eq!(first, second);
    }

    #[test]
    fn stereo_and_timestamps() {
        let mut a = Analyzer::new(48_000);
        let mono = sine(440.0, 48_000, 0.5, 0.5);
        let stereo: Vec<f32> = mono.iter().flat_map(|&v| [v, v]).collect();
        let mut out = Vec::new();
        a.process(&stereo, 2, 2_000_000, &mut out);
        assert_eq!(out.len(), 24_000 / HOP);
        assert_eq!(out[0].pts_us, 2_000_000);
        assert_eq!(out[1].pts_us, 2_000_000 + (HOP as i64 * 1_000_000 / 48_000));
    }
}
