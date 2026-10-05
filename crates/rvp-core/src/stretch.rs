//! Pitch-preserving time stretch (WSOLA: waveform similarity overlap-add) for interleaved `f32` audio.
//!
//! Output is `1 / rate` times as long as the input: rate 2.0 plays twice as fast, rate 0.5 half as fast, with the
//! pitch unchanged. Streaming: feed any chunk sizes, take what comes out. The algorithm: cut Hann-windowed
//! segments of `N` frames from the input at an analysis hop of `rate * N/2`, overlap-add them at a fixed synthesis
//! hop of `N/2`, and nudge each cut within a small tolerance to the offset where it best continues the previous
//! segment (normalised cross-correlation on a mono mix), so the joins are phase aligned.
use alloc::vec::Vec;

/// Streaming WSOLA time stretcher.
#[derive(Debug, Clone)]
pub struct TimeStretcher {
    ch: usize,
    rate: f64,
    /// Window length in frames (even).
    n: usize,
    /// Synthesis hop, `n / 2`.
    hs: usize,
    /// Search tolerance on each side, frames.
    tol: usize,
    window: Vec<f32>,
    /// Interleaved input not yet dropped.
    inb: Vec<f32>,
    /// Mono mix of `inb`.
    mono: Vec<f32>,
    /// Absolute frame index of `inb[0]`.
    base: u64,
    /// Ideal analysis position of the next segment (absolute frames).
    next_ideal: f64,
    /// Start of the previously used segment (absolute), if any.
    prev_start: Option<u64>,
    /// Overlap-add accumulator: the second half of the last window, `hs` frames, interleaved.
    acc: Vec<f32>,
}

impl TimeStretcher {
    /// A stretcher for `channels` interleaved channels at `sample_rate` Hz and playback speed `rate` (0.1..=8).
    pub fn new(sample_rate: u32, channels: usize, rate: f64) -> Self {
        let n = ((sample_rate as f64 * 0.040) as usize).max(64) & !1;
        let hs = n / 2;
        let window = (0..n)
            .map(|i| {
                let x = (i as f64 + 0.5) / n as f64;
                let s = libm::sin(core::f64::consts::PI * x);
                (s * s) as f32
            })
            .collect();
        Self {
            ch: channels.max(1),
            rate: rate.clamp(0.1, 8.0),
            n,
            hs,
            tol: n / 4,
            window,
            inb: Vec::new(),
            mono: Vec::new(),
            base: 0,
            next_ideal: 0.0,
            prev_start: None,
            acc: alloc::vec![0.0; hs * channels.max(1)],
        }
    }

    /// The playback speed.
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Forget everything (after a seek).
    pub fn reset(&mut self) {
        self.inb.clear();
        self.mono.clear();
        self.base = 0;
        self.next_ideal = 0.0;
        self.prev_start = None;
        self.acc.iter_mut().for_each(|v| *v = 0.0);
    }

    /// Feed interleaved input and append the output that became ready to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.ch;
        for f in input.chunks_exact(ch) {
            self.mono.push(f.iter().sum::<f32>() / ch as f32);
        }
        self.inb.extend_from_slice(&input[..input.len() / ch * ch]);
        loop {
            let avail = (self.inb.len() / ch) as u64 + self.base;
            let target = libm::floor(self.next_ideal + 0.5) as i64;
            // The segment may start up to `tol` after the ideal position and needs `n` frames.
            let lo = (target - self.tol as i64).max(self.base as i64) as u64;
            let hi = (target + self.tol as i64).max(lo as i64) as u64;
            if hi + self.n as u64 > avail {
                break;
            }
            let start = match self.prev_start {
                None => (target.max(self.base as i64)) as u64,
                Some(prev) => self.best_start(prev + self.hs as u64, lo, hi),
            };
            self.emit(start, out);
            self.prev_start = Some(start);
            self.next_ideal += self.hs as f64 * self.rate;
            self.compact();
        }
    }

    /// The start in `lo..=hi` whose first `hs` frames best match the natural continuation of the previous
    /// segment (the `hs` frames following it, at `cont`).
    fn best_start(&self, cont: u64, lo: u64, hi: u64) -> u64 {
        let hs = self.hs;
        let at = |p: u64| (p - self.base) as usize;
        let c0 = at(cont);
        if c0 + hs > self.mono.len() {
            return (lo + hi) / 2;
        }
        let reference = &self.mono[c0..c0 + hs];
        // Normalised correlation of the candidate at `p` with the reference, on every `stride`-th sample.
        let score = |p: u64, stride: usize| -> f32 {
            let s = at(p);
            let cand = &self.mono[s..s + hs];
            let (mut dot, mut energy) = (0.0f32, 1e-6f32);
            let mut i = 0;
            while i < hs {
                dot += reference[i] * cand[i];
                energy += cand[i] * cand[i];
                i += stride;
            }
            dot / libm::sqrtf(energy)
        };
        // Coarse pass: every 4th position on every 4th sample; then refine around the winner.
        let mut best = (f32::MIN, (lo + hi) / 2);
        let mut p = lo;
        while p <= hi {
            let sc = score(p, 4);
            if sc > best.0 {
                best = (sc, p);
            }
            p += 4;
        }
        let (from, to) = (best.1.saturating_sub(4).max(lo), (best.1 + 4).min(hi));
        let mut best = (f32::MIN, best.1);
        let mut p = from;
        while p <= to {
            let sc = score(p, 2);
            if sc > best.0 {
                best = (sc, p);
            }
            p += 1;
        }
        best.1
    }

    /// Window the segment at `start`, overlap-add it onto the accumulator, and emit the finished half.
    fn emit(&mut self, start: u64, out: &mut Vec<f32>) {
        let (ch, n, hs) = (self.ch, self.n, self.hs);
        let s = (start - self.base) as usize * ch;
        let seg = &self.inb[s..s + n * ch];
        // First half: add to the accumulator and output.
        for i in 0..hs {
            let w = self.window[i];
            for c in 0..ch {
                out.push(self.acc[i * ch + c] + seg[i * ch + c] * w);
            }
        }
        // Second half becomes the new accumulator.
        for i in 0..hs {
            let w = self.window[hs + i];
            for c in 0..ch {
                self.acc[i * ch + c] = seg[(hs + i) * ch + c] * w;
            }
        }
    }

    /// Drop input that can no longer be used.
    fn compact(&mut self) {
        let keep_from = {
            let ideal = libm::floor(self.next_ideal) as i64 - self.tol as i64;
            let cont = self.prev_start.map_or(i64::MAX, |p| (p + self.hs as u64) as i64);
            ideal.min(cont).max(self.base as i64) as u64
        };
        let drop = (keep_from - self.base) as usize;
        if drop > 4096 {
            self.inb.drain(..drop * self.ch);
            self.mono.drain(..drop);
            self.base += drop as u64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, frames: usize, ch: usize) -> Vec<f32> {
        let mut v = Vec::new();
        for i in 0..frames {
            let s = libm::sin(2.0 * core::f64::consts::PI * freq * i as f64 / rate as f64) as f32 * 0.5;
            for _ in 0..ch {
                v.push(s);
            }
        }
        v
    }

    /// Frequency by counting rising zero crossings in the middle of the signal.
    fn freq_of(x: &[f32], rate: u32, ch: usize) -> f64 {
        let mono: Vec<f32> = x.chunks_exact(ch).map(|f| f[0]).collect();
        let seg = &mono[mono.len() / 4..mono.len() * 3 / 4];
        let crossings = seg.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        crossings as f64 / (seg.len() as f64 / rate as f64)
    }

    #[test]
    fn length_follows_the_rate_and_pitch_stays() {
        for rate in [0.5, 0.75, 1.25, 1.5, 2.0, 3.0, 4.0] {
            let input = sine(440.0, 48_000, 48_000 * 4, 2);
            let mut st = TimeStretcher::new(48_000, 2, rate);
            let mut out = Vec::new();
            // Feed in odd chunk sizes, as a decoder would.
            for chunk in input.chunks(2 * 1000 + 14) {
                st.process(chunk, &mut out);
            }
            let frames = out.len() / 2;
            let want = (48_000.0 * 4.0 / rate) as usize;
            assert!(frames.abs_diff(want) < 6000, "rate {rate}: {frames} frames, wanted about {want}");
            let f = freq_of(&out, 48_000, 2);
            assert!((f - 440.0).abs() < 6.0, "rate {rate}: pitch {f} Hz");
        }
    }

    #[test]
    fn level_is_preserved() {
        let input = sine(220.0, 44_100, 44_100 * 3, 1);
        let mut st = TimeStretcher::new(44_100, 1, 1.7);
        let mut out = Vec::new();
        st.process(&input, &mut out);
        let seg = &out[out.len() / 4..out.len() * 3 / 4];
        let rms = (seg.iter().map(|v| v * v).sum::<f32>() / seg.len() as f32).sqrt();
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.03, "rms {rms}");
    }

    #[test]
    fn reset_starts_over_and_chunking_does_not_matter() {
        let input = sine(330.0, 48_000, 48_000, 2);
        let run = |chunk: usize| {
            let mut st = TimeStretcher::new(48_000, 2, 1.5);
            let mut out = Vec::new();
            for c in input.chunks(chunk) {
                st.process(c, &mut out);
            }
            out
        };
        let a = run(2 * 4800);
        let b = run(2 * 333);
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-4));
        let mut st = TimeStretcher::new(48_000, 2, 1.5);
        let mut o1 = Vec::new();
        st.process(&input, &mut o1);
        st.reset();
        let mut o2 = Vec::new();
        st.process(&input, &mut o2);
        assert_eq!(o1, o2);
    }
}
