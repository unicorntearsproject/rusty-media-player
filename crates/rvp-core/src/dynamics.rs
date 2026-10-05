//! Gain tools for the output stage: dB conversions, the equal-power crossfade curve, a slewed gain stage and a true-peak
//! lookahead limiter.
use crate::loudness::{TRUE_PEAK_HISTORY, TRUE_PEAK_LOOKAHEAD, TruePeak};
use alloc::vec::Vec;

/// Linear gain of `db` decibels.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    libm::powf(10.0, db / 20.0)
}

/// Decibels of a linear gain (`-inf` for zero).
#[inline]
pub fn gain_to_db(g: f32) -> f32 {
    20.0 * libm::log10f(g)
}

/// Equal-power crossfade gains at position `t` (0 at the start of the fade, 1 at its end): `(fading out, fading in)`.
/// Their squares add up to one, so two uncorrelated sounds keep their combined power through the join.
#[inline]
pub fn equal_power(t: f32) -> (f32, f32) {
    let a = t.clamp(0.0, 1.0) * core::f32::consts::FRAC_PI_2;
    (libm::cosf(a), libm::sinf(a))
}

/// A gain that glides to wherever it is told, never faster than a given number of decibels a second, and applies itself
/// to interleaved audio with a straight line from the old gain to the new one across each block (no clicks).
#[derive(Debug, Clone, Copy, Default)]
pub struct GainStage {
    db: f32,
}

impl GainStage {
    /// The gain in decibels now.
    pub fn db(&self) -> f32 {
        self.db
    }

    /// Jump to `db` without a ramp (a new item, a seek).
    pub fn set_now(&mut self, db: f32) {
        self.db = db;
    }

    /// Move toward `goal_db` for a block of `frames` frames at `rate` Hz, at most `max_db_per_s`, and scale `buf` (`ch`
    /// channels interleaved) accordingly. Leaves `buf` alone while the gain is exactly zero dB and the goal is too.
    pub fn apply(&mut self, buf: &mut [f32], ch: usize, rate: u32, goal_db: f32, max_db_per_s: f32) {
        let frames = buf.len() / ch.max(1);
        if frames == 0 {
            return;
        }
        let secs = frames as f32 / rate.max(1) as f32;
        let max_step = max_db_per_s * secs;
        let new_db = self.db + (goal_db - self.db).clamp(-max_step, max_step);
        let (g0, g1) = (db_to_gain(self.db), db_to_gain(new_db));
        self.db = new_db;
        if g0 == 1.0 && g1 == 1.0 {
            return;
        }
        if g0 == g1 {
            for s in buf.iter_mut() {
                *s *= g1;
            }
            return;
        }
        let step = (g1 - g0) / frames as f32;
        for (i, f) in buf.chunks_exact_mut(ch).enumerate() {
            let g = g0 + step * (i as f32 + 1.0);
            for s in f {
                *s *= g;
            }
        }
    }
}

/// How far ahead the limiter starts to bring the gain down, milliseconds: it never has to jump.
const ATTACK_MS: f32 = 2.0;
/// Time the gain takes to recover most of the way, milliseconds.
const RELEASE_MS: f32 = 60.0;
/// Deepest reduction the smooth part asks for; the rest is left to the clip that makes the ceiling absolute.
const MIN_GAIN: f32 = 0.1;

/// A true-peak limiter: a look-ahead gain envelope (the gain starts to fall a couple of milliseconds before a peak, so
/// there is no jump) driven by the 4 times oversampled peak of all channels together, with a slow release, and a final
/// clip that makes the ceiling absolute for the samples.
///
/// It works in place on audio that has not been played yet: [`Limiter::process`] is given the frames to finish and the
/// frames after them, which it only looks at. [`Limiter::lookahead_frames`] says how many of the latter give it its full
/// power; with fewer (the very end of the audio) the attack is shorter and the clip does more.
pub struct Limiter {
    channels: usize,
    ceiling: f32,
    attack: usize,
    release: f32,
    /// The gain at the last frame finished.
    gain: f32,
    /// The last frames finished, as they were before limiting, per channel, oldest first (what the peak filter looks back at).
    history: Vec<Vec<f32>>,
    detector: TruePeak,
    env: Vec<f32>,
    tmp: Vec<f32>,
}

impl Limiter {
    /// A limiter for `channels` interleaved channels at `rate` Hz that keeps the peaks under `ceiling_db` (dBFS, at most 0).
    pub fn new(channels: usize, rate: u32, ceiling_db: f32) -> Self {
        let channels = channels.max(1);
        let attack = ((ATTACK_MS * 0.001 * rate as f32) as usize).max(8);
        let release = 1.0 - libm::expf(-1.0 / (RELEASE_MS * 0.001 * rate as f32));
        Self {
            channels,
            ceiling: db_to_gain(ceiling_db.min(0.0)),
            attack,
            release,
            gain: 1.0,
            history: alloc::vec![alloc::vec![0.0; TRUE_PEAK_HISTORY]; channels],
            detector: TruePeak::new(),
            env: Vec::new(),
            tmp: Vec::new(),
        }
    }

    /// The ceiling, linear.
    pub fn ceiling(&self) -> f32 {
        self.ceiling
    }

    /// Frames after the ones being finished that the limiter wants to see for its full effect.
    pub fn lookahead_frames(&self) -> usize {
        // The attack ramp from no reduction to the deepest, the reach of the peak filter, and the sample after the peak.
        self.attack + TRUE_PEAK_LOOKAHEAD + 2
    }

    /// Forget the gain and the history (a seek).
    pub fn reset(&mut self) {
        self.gain = 1.0;
        for h in &mut self.history {
            h.clear();
            h.resize(TRUE_PEAK_HISTORY, 0.0);
        }
    }

    /// The gain at the last frame finished (1.0 is no reduction).
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Limit the first `commit` frames of `buf` in place. The frames after them are only looked at (the next call should
    /// have them first: they are untouched here).
    pub fn process(&mut self, buf: &mut [f32], commit: usize) {
        let ch = self.channels;
        let total = buf.len() / ch;
        let commit = commit.min(total);
        if commit == 0 {
            return;
        }
        // 1. The gain each frame needs on its own: the ceiling over the inter-sample peak around it (frames far under the
        // ceiling are not looked at closely).
        self.env.clear();
        self.env.resize(total, 1.0);
        let quiet = self.ceiling * 0.4;
        let mut next_history: Vec<Vec<f32>> = Vec::with_capacity(ch);
        for c in 0..ch {
            self.tmp.clear();
            self.tmp.extend_from_slice(&self.history[c]);
            self.tmp.extend(buf.chunks_exact(ch).map(|f| f[c]));
            for i in 0..total {
                let at = i + TRUE_PEAK_HISTORY;
                let near = self.tmp[at].abs().max(self.tmp.get(at + 1).map_or(0.0, |v| v.abs()));
                if near <= quiet {
                    continue;
                }
                let peak = self.detector.around(&self.tmp, at);
                if peak > self.ceiling {
                    let need = (self.ceiling / peak).max(MIN_GAIN);
                    if need < self.env[i] {
                        self.env[i] = need;
                    }
                }
            }
            // The last frames to be finished, before they change.
            next_history.push(self.tmp[commit..commit + TRUE_PEAK_HISTORY].to_vec());
        }
        // 2. The attack: going backwards, the gain at a frame may exceed the one after it by no more than the slope that takes
        // it from no reduction to the deepest in `attack` frames.
        let slope = (1.0 - MIN_GAIN) / self.attack as f32;
        for i in (0..total.saturating_sub(1)).rev() {
            let next = self.env[i + 1] + slope;
            if self.env[i] > next {
                self.env[i] = next;
            }
        }
        // 3. Forward: follow the envelope down at once, come back up slowly; apply, and clip what is left.
        let mut g = self.gain;
        for (i, f) in buf.chunks_exact_mut(ch).take(commit).enumerate() {
            let up = g + (1.0 - g) * self.release + 1e-7;
            g = self.env[i].min(up).min(1.0);
            for s in f.iter_mut() {
                *s = (*s * g).clamp(-self.ceiling, self.ceiling);
            }
        }
        self.gain = g;
        self.history = next_history;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_sine(rate: u32, hz: f32, amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = amp * libm::sinf(2.0 * core::f32::consts::PI * hz * i as f32 / rate as f32);
                [s, s]
            })
            .collect()
    }

    /// The largest inter-sample peak of both channels.
    fn true_peak(buf: &[f32]) -> f32 {
        let tp = TruePeak::new();
        let mut peak = 0.0f32;
        for c in 0..2 {
            let mut x: Vec<f32> = alloc::vec![0.0; TRUE_PEAK_HISTORY];
            x.extend(buf.chunks_exact(2).map(|f| f[c]));
            x.extend([0.0; 16]);
            for i in 0..x.len() - 8 {
                peak = peak.max(tp.around(&x, i));
            }
        }
        peak
    }

    #[test]
    fn db_and_equal_power() {
        assert!((db_to_gain(-6.0) - 0.501_187).abs() < 1e-5);
        assert!((gain_to_db(db_to_gain(7.3)) - 7.3).abs() < 1e-4);
        for k in 0..=20 {
            let (o, i) = equal_power(k as f32 / 20.0);
            assert!((o * o + i * i - 1.0).abs() < 1e-6);
        }
        assert_eq!(equal_power(0.0), (1.0, 0.0));
        let (o, i) = equal_power(1.0);
        assert!(o.abs() < 1e-6 && (i - 1.0).abs() < 1e-6);
        // The middle of the fade is 3 dB down for both.
        let (o, i) = equal_power(0.5);
        assert!((gain_to_db(o) + 3.01).abs() < 0.01 && (o - i).abs() < 1e-6);
    }

    #[test]
    fn gain_stage_glides_and_stays_out_of_the_way_at_unity() {
        let mut g = GainStage::default();
        let mut quiet = alloc::vec![0.25f32; 2 * 480];
        g.apply(&mut quiet, 2, 48_000, 0.0, 20.0);
        assert!(quiet.iter().all(|&s| s == 0.25), "unity is not touched");
        // 10 ms blocks toward -12 dB at 60 dB a second: 0.6 dB per block, so 20 blocks.
        let mut blocks = 0;
        let mut last_gain = 1.0f32;
        while (g.db() + 12.0).abs() > 1e-4 {
            let mut b = alloc::vec![1.0f32; 2 * 480];
            g.apply(&mut b, 2, 48_000, -12.0, 60.0);
            // Inside a block the gain moves in a straight line, and no step between blocks is bigger than the slew allows.
            let first = b[0];
            assert!((last_gain - first).abs() < 0.08, "{last_gain} {first}");
            last_gain = b[b.len() - 1];
            blocks += 1;
            assert!(blocks <= 21);
        }
        assert_eq!(blocks, 20);
        assert!((last_gain - db_to_gain(-12.0)).abs() < 1e-3);
    }

    #[test]
    fn limiter_leaves_quiet_audio_exactly_alone() {
        let mut l = Limiter::new(2, 48_000, -1.0);
        let orig = stereo_sine(48_000, 440.0, 0.5, 4800);
        let mut buf = orig.clone();
        let n = buf.len() / 2;
        l.process(&mut buf, n - 200);
        assert_eq!(&buf[..2 * (n - 200)], &orig[..2 * (n - 200)]);
        assert_eq!(&buf[2 * (n - 200)..], &orig[2 * (n - 200)..], "the look-ahead is not touched");
        assert_eq!(l.gain(), 1.0);
    }

    #[test]
    fn limiter_never_exceeds_its_ceiling() {
        let ceiling = db_to_gain(-1.0);
        // Sines far over full scale, a burst, fs/4 whose samples hide a higher peak, and noise-like jumps.
        let mut cases: Vec<Vec<f32>> =
            alloc::vec![stereo_sine(48_000, 997.0, 2.5, 9600), stereo_sine(48_000, 12_000.0, 1.6, 9600),];
        let mut burst = stereo_sine(48_000, 220.0, 0.2, 9600);
        for s in &mut burst[4000..4600] {
            *s *= 12.0;
        }
        cases.push(burst);
        let mut steps = Vec::new();
        let mut x = 12345u32;
        for i in 0..9600 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let r = (x >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
            let loud = if (i / 700) % 3 == 0 { 3.0 } else { 0.3 };
            steps.extend([r * loud, -r * loud]);
        }
        cases.push(steps);
        for (n, case) in cases.into_iter().enumerate() {
            // Whole, and in odd-sized pieces with the look-ahead kept back as the output stage does.
            for chunk in [case.len() / 2, 2 * 1000, 2 * 333] {
                let mut l = Limiter::new(2, 48_000, -1.0);
                let la = l.lookahead_frames();
                let mut out = Vec::new();
                let mut pending: Vec<f32> = Vec::new();
                for piece in case.chunks(chunk) {
                    pending.extend_from_slice(piece);
                    let frames = pending.len() / 2;
                    if frames > la {
                        let commit = frames - la;
                        l.process(&mut pending, commit);
                        out.extend(pending.drain(..2 * commit));
                    }
                }
                let rest = pending.len() / 2;
                l.process(&mut pending, rest);
                out.extend(pending);
                assert_eq!(out.len(), case.len());
                let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                assert!(peak <= ceiling + 1e-6, "case {n}, chunk {chunk}: sample peak {peak} over {ceiling}");
                // Between the samples too (the gain is smooth, so the waveform cannot rise far above the samples).
                let tp = true_peak(&out);
                assert!(
                    gain_to_db(tp) < -1.0 + 0.5,
                    "case {n}, chunk {chunk}: true peak {} dB",
                    gain_to_db(tp)
                );
            }
        }
    }

    #[test]
    fn limiter_ramps_in_ahead_of_a_peak_and_releases_slowly() {
        let mut l = Limiter::new(2, 48_000, -1.0);
        let mut buf = alloc::vec![0.3f32; 2 * 9600];
        for f in buf[2 * 4800..2 * 4900].iter_mut() {
            *f = 2.0;
        }
        let n = buf.len() / 2;
        l.process(&mut buf, n);
        // Before the peak the gain comes down smoothly, never by a big step.
        let mut max_step = 0.0f32;
        for w in buf[..2 * 4800].chunks_exact(2).collect::<Vec<_>>().windows(2) {
            max_step = max_step.max((w[1][0] - w[0][0]).abs());
        }
        assert!(max_step < 0.01, "{max_step}");
        assert!(buf[2 * 4790] < 0.3 && buf[2 * 4700] == 0.3, "starts coming down just before the peak");
        // After it the gain is still low a few ms later and has recovered most of the way 100 ms on (a 60 ms release).
        assert!(buf[2 * 5200] < 0.18, "{}", buf[2 * 5200]);
        assert!(buf[2 * 9500] > 0.25 && buf[2 * 9500] < 0.3, "{}", buf[2 * 9500]);
    }
}
