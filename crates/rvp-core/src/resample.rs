//! A streaming polyphase windowed-sinc resampler for interleaved `f32` audio.
//!
//! 32-tap Blackman-windowed sinc, 256 phases with linear interpolation between phases, unity gain at DC.
//! Good for playback (device rate differs from stream rate), not for mastering: stop-band rejection is
//! roughly -70 dB. Streaming: feed any chunk sizes; output is delayed by nothing (the kernel is centred),
//! and [`Resampler::flush`] emits the tail.
use alloc::vec::Vec;

const HALF: usize = 16; // taps per side
const PHASES: usize = 256;

/// Streaming resampler.
#[derive(Debug, Clone)]
pub struct Resampler {
    channels: usize,
    step: f64,
    /// Position of the next output frame, in frames from the start of `hist`.
    pos: f64,
    /// Interleaved input history (starts with `HALF` frames of silence).
    hist: Vec<f32>,
    /// `(PHASES + 1) * 2 * HALF` kernel coefficients, normalised per phase.
    table: Vec<f32>,
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        let p = core::f64::consts::PI * x;
        libm::sin(p) / p
    }
}

fn blackman(t: f64) -> f64 {
    // t in [-1, 1]
    let x = (t + 1.0) * 0.5; // 0..1
    if !(0.0..=1.0).contains(&x) {
        return 0.0;
    }
    0.42 - 0.5 * libm::cos(2.0 * core::f64::consts::PI * x)
        + 0.08 * libm::cos(4.0 * core::f64::consts::PI * x)
}

impl Resampler {
    /// A resampler from `in_rate` to `out_rate` Hz for `channels` interleaved channels.
    pub fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        let step = in_rate as f64 / out_rate.max(1) as f64;
        let cutoff = if step > 1.0 { 1.0 / step } else { 1.0 }; // low-pass when downsampling
        let mut table = Vec::with_capacity((PHASES + 1) * 2 * HALF);
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            let start = table.len();
            let mut sum = 0.0;
            for k in 0..2 * HALF {
                let t = k as f64 - (HALF as f64 - 1.0) - frac; // sample offset from the output position
                let v = cutoff * sinc(cutoff * t) * blackman(t / HALF as f64);
                sum += v;
                table.push(v as f32);
            }
            for v in &mut table[start..] {
                *v = (*v as f64 / sum) as f32;
            }
        }
        let mut r = Self { channels: channels.max(1), step, pos: HALF as f64, hist: Vec::new(), table };
        r.reset();
        r
    }

    /// Forget all history (after a seek).
    pub fn reset(&mut self) {
        self.hist.clear();
        self.hist.resize(HALF * self.channels, 0.0);
        self.pos = HALF as f64;
    }

    /// Resample `input` (interleaved) and append the result to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        self.hist.extend_from_slice(input);
        let frames = self.hist.len() / ch;
        loop {
            let i0 = self.pos as usize; // floor, pos >= 0
            if i0 + HALF >= frames {
                break;
            }
            let frac = self.pos - i0 as f64;
            let ph = frac * PHASES as f64;
            let p = ph as usize;
            let w = (ph - p as f64) as f32;
            let a = &self.table[p * 2 * HALF..(p + 1) * 2 * HALF];
            let b = &self.table[(p + 1) * 2 * HALF..(p + 2) * 2 * HALF];
            let first = i0 + 1 - HALF;
            for c in 0..ch {
                let mut acc = 0.0f32;
                for k in 0..2 * HALF {
                    let coef = a[k] + (b[k] - a[k]) * w;
                    acc += coef * self.hist[(first + k) * ch + c];
                }
                out.push(acc);
            }
            self.pos += self.step;
        }
        // Drop history we no longer need.
        let keep_from = (self.pos as usize).saturating_sub(HALF - 1);
        if keep_from > 0 {
            self.hist.drain(..keep_from * ch);
            self.pos -= keep_from as f64;
        }
    }

    /// Emit the frames still pending by feeding silence; call at end of stream.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        let zeros = alloc::vec![0.0f32; HALF * self.channels];
        self.process(&zeros, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| libm::sin(2.0 * core::f64::consts::PI * freq * i as f64 / rate as f64) as f32 * 0.5)
            .collect()
    }

    #[test]
    fn upsample_preserves_a_sine_with_high_snr() {
        let mut r = Resampler::new(44_100, 48_000, 1);
        let mut out = Vec::new();
        let input = sine(44_100, 1000.0, 44_100);
        // Feed in odd-sized chunks to exercise streaming.
        for c in input.chunks(777) {
            r.process(c, &mut out);
        }
        r.flush(&mut out);
        assert!((out.len() as i64 - 48_000).abs() <= 2, "len {}", out.len());
        let want = sine(48_000, 1000.0, 48_000);
        let (mut e, mut s) = (0.0f64, 0.0f64);
        for i in 1000..46_000 {
            let d = (out[i] - want[i]) as f64;
            e += d * d;
            s += (want[i] as f64).powi(2);
        }
        let snr = 10.0 * libm::log10(s / e);
        assert!(snr > 60.0, "SNR {snr} dB");
    }

    #[test]
    fn downsample_attenuates_above_new_nyquist_and_keeps_stereo_interleaving() {
        let mut r = Resampler::new(48_000, 24_000, 2);
        let tone = sine(48_000, 18_000.0, 4800); // above the new 12 kHz Nyquist
        let mut stereo = Vec::new();
        for s in &tone {
            stereo.push(*s);
            stereo.push(0.0); // right channel silent
        }
        let mut out = Vec::new();
        r.process(&stereo, &mut out);
        assert!(out.iter().skip(200).step_by(2).all(|v| v.abs() < 0.02), "aliasing leaked through");
        assert!(out.iter().skip(1).step_by(2).all(|v| *v == 0.0), "channels mixed");
    }

    #[test]
    fn flush_emits_the_tail() {
        let mut r = Resampler::new(48_000, 44_100, 1);
        let mut out = Vec::new();
        r.process(&alloc::vec![0.25; 4800], &mut out);
        let before = out.len();
        r.flush(&mut out);
        assert!(out.len() > before);
        assert!((out.len() as i64 - 4410).abs() <= 2, "len {}", out.len());
    }
}
