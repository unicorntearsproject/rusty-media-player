//! Loudness measurement after ITU-R BS.1770-4 and EBU R 128 (EBU Tech 3341 "EBU mode"): K-weighting, 400 ms blocks every
//! 100 ms, momentary (400 ms) and short-term (3 s) loudness, gated integrated loudness (absolute gate -70 LUFS, relative
//! gate -10 LU) and, optionally, the true peak (4 times oversampled).
//!
//! Streaming and allocation-light: feed interleaved `f32` in any chunk size. The filter coefficients are computed for the
//! sample rate in use from the published analogue prototype (they reproduce the 48 kHz coefficients of the standard),
//! so 44.1 kHz and the rest need no tables. The gated measurement keeps a histogram of block energies (0.02 LU bins that
//! also hold the exact energy sum), so memory does not grow with the length of the programme and the running value can be
//! asked for at any time.
//!
//! Channel order is the film order the rest of the player assumes: L R C LFE Ls Rs (and Lb Rb, or a back centre, after
//! them). The LFE is left out and the surrounds weigh +1.5 dB, as the standard says. One channel counts as dual mono
//! (it is played to both speakers).
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// Blocks quieter than this are ignored altogether (the absolute gate), LUFS.
pub const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// The relative gate sits this far below the loudness of everything that passed the absolute gate, LU.
const RELATIVE_GATE_LU: f64 = 10.0;
/// The constant of the loudness formula.
const OFFSET: f64 = -0.691;
/// Histogram resolution, LU, and number of bins (-70 to +10 LUFS).
const BIN_W: f64 = 0.02;
const BINS: usize = 4000;
/// Sub-blocks (100 ms) in a 400 ms block and in a 3 s window.
const SUBS_MOMENTARY: usize = 4;
const SUBS_SHORT: usize = 30;

/// Loudness in LUFS of a mean-square energy (weighted sum over channels).
#[inline]
fn lufs_of(energy: f64) -> f64 {
    OFFSET + 10.0 * libm::log10(energy)
}

/// Energy that has the loudness `lufs`.
#[inline]
fn energy_of(lufs: f64) -> f64 {
    libm::pow(10.0, (lufs - OFFSET) / 10.0)
}

#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// The two K-weighting stages for `rate` Hz: a high shelf (+4 dB above about 1.7 kHz) and a 38 Hz high-pass.
fn k_weighting(rate: f64) -> [Biquad; 2] {
    let pi = core::f64::consts::PI;
    // Stage 1: the head-related shelf.
    let (f0, gain_db, q) = (1_681.974_450_955_533, 3.999_843_853_973_347, 0.707_175_236_955_419_6);
    let k = libm::tan(pi * f0 / rate);
    let vh = libm::pow(10.0, gain_db / 20.0);
    let vb = libm::pow(vh, 0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b0: (vh + vb * k / q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
        z1: 0.0,
        z2: 0.0,
    };
    // Stage 2: the revised low-frequency B-curve (a second-order high-pass).
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = libm::tan(pi * f0 / rate);
    let a0 = 1.0 + k / q + k * k;
    let hp = Biquad {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
        z1: 0.0,
        z2: 0.0,
    };
    [shelf, hp]
}

/// Channel weights for the film order, 0 for the LFE.
fn channel_weights(channels: usize) -> Vec<f64> {
    const SURROUND: f64 = 1.41;
    match channels {
        1 => alloc::vec![1.0],
        2 => alloc::vec![1.0, 1.0],
        3 => alloc::vec![1.0, 1.0, 1.0],
        4 => alloc::vec![1.0, 1.0, SURROUND, SURROUND],
        5 => alloc::vec![1.0, 1.0, 1.0, SURROUND, SURROUND],
        6 => alloc::vec![1.0, 1.0, 1.0, 0.0, SURROUND, SURROUND],
        7 => alloc::vec![1.0, 1.0, 1.0, 0.0, SURROUND, SURROUND, SURROUND],
        n => {
            let mut w = alloc::vec![1.0, 1.0, 1.0, 0.0, SURROUND, SURROUND, SURROUND, SURROUND];
            w.resize(n, 1.0);
            w
        }
    }
}

/// Number of taps per phase of the oversampling filter, and its reach on each side of the sample.
const TP_TAPS: usize = 12;
const TP_BEFORE: usize = 5;
/// How many input frames after a sample the oversampled value near it depends on.
pub const TRUE_PEAK_LOOKAHEAD: usize = TP_TAPS - TP_BEFORE - 1;
/// Frames before a sample the oversampled value near it depends on.
pub const TRUE_PEAK_HISTORY: usize = TP_BEFORE;

/// The three interpolating phases (a quarter, a half and three quarters of the way to the next sample) of a 4 times
/// oversampling filter: Kaiser-windowed sinc, 12 taps per phase, unity gain at DC.
fn tp_phases() -> [[f32; TP_TAPS]; 3] {
    // Kaiser window, beta 5.
    fn i0(x: f64) -> f64 {
        let (mut sum, mut term) = (1.0, 1.0);
        for k in 1..30 {
            term *= (x / (2.0 * k as f64)) * (x / (2.0 * k as f64));
            sum += term;
        }
        sum
    }
    let beta = 5.0;
    let half = TP_TAPS as f64 / 2.0;
    let mut out = [[0.0f32; TP_TAPS]; 3];
    for (p, phase) in out.iter_mut().enumerate() {
        let frac = (p + 1) as f64 / 4.0;
        let mut taps = [0.0f64; TP_TAPS];
        let mut sum = 0.0;
        for (k, t) in taps.iter_mut().enumerate() {
            // Offset of tap k from the interpolated point, in samples.
            let x = k as f64 - TP_BEFORE as f64 - frac;
            let s = if x.abs() < 1e-9 {
                1.0
            } else {
                libm::sin(core::f64::consts::PI * x) / (core::f64::consts::PI * x)
            };
            let r = x / half;
            let w = if r.abs() >= 1.0 { 0.0 } else { i0(beta * libm::sqrt(1.0 - r * r)) / i0(beta) };
            *t = s * w;
            sum += *t;
        }
        for (o, t) in phase.iter_mut().zip(taps) {
            *o = (t / sum) as f32;
        }
    }
    out
}

/// Estimate of the true (inter-sample) peak of a signal: the sample peak and three interpolated points between every
/// pair of samples, found with a 4 times oversampling filter. Good to about 0.1 dB for programme material.
#[derive(Debug, Clone)]
pub struct TruePeak {
    phases: [[f32; TP_TAPS]; 3],
}

impl Default for TruePeak {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeak {
    /// A detector (the filter is built once).
    pub fn new() -> Self {
        Self { phases: tp_phases() }
    }

    /// The largest absolute value among `x[i]` and the interpolated points between `x[i]` and `x[i + 1]`. The filter looks
    /// [`TRUE_PEAK_HISTORY`] samples before `i` and [`TRUE_PEAK_LOOKAHEAD`] after it; samples that are not in `x` count as
    /// silence.
    pub fn around(&self, x: &[f32], i: usize) -> f32 {
        let mut peak = x.get(i).map_or(0.0, |v| v.abs());
        for phase in &self.phases {
            let mut acc = 0.0f32;
            for (k, c) in phase.iter().enumerate() {
                let idx = i as isize + k as isize - TP_BEFORE as isize;
                if idx >= 0 {
                    if let Some(v) = x.get(idx as usize) {
                        acc += c * v;
                    }
                }
            }
            peak = peak.max(acc.abs());
        }
        peak
    }
}

/// What a finished measurement says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    /// Gated integrated loudness, LUFS.
    pub integrated_lufs: f32,
    /// Largest sample magnitude of any channel, linear (1.0 is full scale).
    pub sample_peak: f32,
    /// Largest inter-sample peak, linear, when the meter was asked to find it.
    pub true_peak: Option<f32>,
    /// Length measured, microseconds.
    pub duration_us: i64,
}

/// A BS.1770 loudness meter for one programme.
pub struct LoudnessMeter {
    rate: u32,
    channels: usize,
    weights: Vec<f64>,
    filters: Vec<[Biquad; 2]>,
    /// Energy (weighted sum over channels of the sum of squares) and frames of the sub-block being filled.
    sub_sum: f64,
    sub_frames: u64,
    /// Frames at which the current sub-block ends (cumulative), so a sub-block is exactly a tenth of a second on average.
    sub_end: u64,
    sub_index: u64,
    total_frames: u64,
    /// The last 30 completed sub-blocks: `(energy sum, frames)`.
    subs: VecDeque<(f64, u64)>,
    hist_count: Vec<u32>,
    hist_sum: Vec<f64>,
    gated_blocks: u64,
    sample_peak: f32,
    true_peak: Option<TpState>,
}

/// The inter-sample peak search: per channel the samples still needed (history plus those not yet judged).
struct TpState {
    detector: TruePeak,
    bufs: Vec<Vec<f32>>,
    judged: Vec<usize>,
    peak: f32,
}

impl TpState {
    fn new(channels: usize) -> Self {
        Self {
            detector: TruePeak::new(),
            bufs: alloc::vec![Vec::new(); channels],
            judged: alloc::vec![0; channels],
            peak: 0.0,
        }
    }
}

impl LoudnessMeter {
    /// A meter for interleaved audio of `channels` channels at `rate` Hz.
    pub fn new(rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        let rate = rate.max(1000);
        Self {
            rate,
            channels,
            weights: channel_weights(channels),
            filters: (0..channels).map(|_| k_weighting(rate as f64)).collect(),
            sub_sum: 0.0,
            sub_frames: 0,
            sub_end: Self::sub_end_at(rate, 1),
            sub_index: 1,
            total_frames: 0,
            subs: VecDeque::with_capacity(SUBS_SHORT + 1),
            hist_count: alloc::vec![0; BINS],
            hist_sum: alloc::vec![0.0; BINS],
            gated_blocks: 0,
            sample_peak: 0.0,
            true_peak: None,
        }
    }

    /// Also find the true peak (about three times the work of the loudness alone).
    pub fn with_true_peak(mut self) -> Self {
        self.true_peak = Some(TpState::new(self.channels));
        self
    }

    fn sub_end_at(rate: u32, k: u64) -> u64 {
        (k * rate as u64 + 5) / 10
    }

    /// Start over (same rate and channels).
    pub fn reset(&mut self) {
        let tp = self.true_peak.is_some();
        *self = Self::new(self.rate, self.channels);
        if tp {
            self.true_peak = Some(TpState::new(self.channels));
        }
    }

    /// Sample rate the meter was made for.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Frames measured so far.
    pub fn frames(&self) -> u64 {
        self.total_frames
    }

    /// Feed interleaved samples.
    pub fn process(&mut self, samples: &[f32]) {
        let ch = self.channels;
        let dual = if ch == 1 { 2.0 } else { 1.0 };
        for frame in samples.chunks_exact(ch) {
            let mut energy = 0.0;
            for (c, &x) in frame.iter().enumerate() {
                let a = x.abs();
                if a > self.sample_peak {
                    self.sample_peak = a;
                }
                let w = self.weights[c];
                if w == 0.0 {
                    continue;
                }
                let f = &mut self.filters[c];
                let shelved = f[0].run(x as f64);
                let y = f[1].run(shelved);
                energy += w * y * y;
            }
            self.sub_sum += energy * dual;
            self.sub_frames += 1;
            self.total_frames += 1;
            if self.total_frames >= self.sub_end {
                self.finish_sub();
            }
        }
        if self.true_peak.is_some() {
            self.scan_true_peak(samples);
        }
    }

    /// Judge every sample that has enough samples after it, keeping only the history and the samples still to judge.
    fn scan_true_peak(&mut self, samples: &[f32]) {
        let ch = self.channels;
        let Some(st) = &mut self.true_peak else { return };
        for c in 0..ch {
            let buf = &mut st.bufs[c];
            buf.extend(samples.chunks_exact(ch).map(|f| f[c]));
            let mut j = st.judged[c];
            while j + TRUE_PEAK_LOOKAHEAD < buf.len() {
                let p = st.detector.around(buf, j);
                if p > st.peak {
                    st.peak = p;
                }
                j += 1;
            }
            let drop = j.saturating_sub(TRUE_PEAK_HISTORY);
            buf.drain(..drop);
            st.judged[c] = j - drop;
        }
    }

    fn finish_sub(&mut self) {
        self.subs.push_back((self.sub_sum, self.sub_frames));
        if self.subs.len() > SUBS_SHORT {
            self.subs.pop_front();
        }
        self.sub_sum = 0.0;
        self.sub_frames = 0;
        self.sub_index += 1;
        self.sub_end = Self::sub_end_at(self.rate, self.sub_index);
        if let Some(e) = self.window_energy(SUBS_MOMENTARY) {
            let l = lufs_of(e);
            if l >= ABSOLUTE_GATE_LUFS {
                let bin = (((l - ABSOLUTE_GATE_LUFS) / BIN_W) as usize).min(BINS - 1);
                self.hist_count[bin] += 1;
                self.hist_sum[bin] += e;
                self.gated_blocks += 1;
            }
        }
    }

    /// Mean energy of the last `n` completed sub-blocks, if there are that many.
    fn window_energy(&self, n: usize) -> Option<f64> {
        if self.subs.len() < n {
            return None;
        }
        let (mut sum, mut frames) = (0.0, 0u64);
        for (s, f) in self.subs.iter().rev().take(n) {
            sum += s;
            frames += f;
        }
        (frames > 0).then(|| sum / frames as f64)
    }

    /// Loudness of the last 400 ms (about, it advances every 100 ms), LUFS; `None` before 400 ms have been measured.
    /// Silence reads as negative infinity.
    pub fn momentary_lufs(&self) -> Option<f32> {
        self.window_energy(SUBS_MOMENTARY).map(|e| lufs_of(e) as f32)
    }

    /// Loudness of the last 3 s (advancing every 100 ms), LUFS; `None` before 3 s have been measured.
    pub fn short_term_lufs(&self) -> Option<f32> {
        self.window_energy(SUBS_SHORT).map(|e| lufs_of(e) as f32)
    }

    /// Blocks that passed the absolute gate so far (a measure of how much there is to go on).
    pub fn gated_blocks(&self) -> u64 {
        self.gated_blocks
    }

    /// Gated integrated loudness of everything fed so far, LUFS. `None` until a block louder than -70 LUFS has been seen.
    pub fn integrated_lufs(&self) -> Option<f32> {
        if self.gated_blocks == 0 {
            return None;
        }
        let (mut count, mut sum) = (0u64, 0.0f64);
        for i in 0..BINS {
            count += self.hist_count[i] as u64;
            sum += self.hist_sum[i];
        }
        let relative = lufs_of(sum / count as f64) - RELATIVE_GATE_LU;
        // Blocks louder than the relative gate (the bin the gate falls in is included only above its middle).
        let first = ((relative - ABSOLUTE_GATE_LUFS) / BIN_W).max(0.0) as usize;
        let (mut count, mut sum) = (0u64, 0.0f64);
        for i in first.min(BINS)..BINS {
            count += self.hist_count[i] as u64;
            sum += self.hist_sum[i];
        }
        if count == 0 {
            return None;
        }
        Some(lufs_of(sum / count as f64) as f32)
    }

    /// Largest sample magnitude so far, linear.
    pub fn sample_peak(&self) -> f32 {
        self.sample_peak
    }

    /// Largest inter-sample peak so far, linear (`None` unless the meter was made with [`LoudnessMeter::with_true_peak`]).
    /// Samples within the last few frames are judged on the next call to [`LoudnessMeter::process`].
    pub fn true_peak(&self) -> Option<f32> {
        self.true_peak.as_ref().map(|t| t.peak.max(self.sample_peak))
    }

    /// The result so far as a [`Measurement`], `None` while there is no audible block yet.
    pub fn measurement(&self) -> Option<Measurement> {
        Some(Measurement {
            integrated_lufs: self.integrated_lufs()?,
            sample_peak: self.sample_peak,
            true_peak: self.true_peak(),
            duration_us: (self.total_frames as i128 * 1_000_000 / self.rate as i128) as i64,
        })
    }
}

/// The loudness two or more programmes have together, from their integrated loudness and lengths: the duration-weighted
/// mean of their energies (what an album measured as one piece would show, without the gating of the join).
pub fn combine_lufs(parts: &[(f32, i64)]) -> Option<f32> {
    let (mut sum, mut total) = (0.0f64, 0i64);
    for &(l, d) in parts {
        if !l.is_finite() || d <= 0 {
            continue;
        }
        sum += energy_of(l as f64) * d as f64;
        total += d;
    }
    (total > 0).then(|| lufs_of(sum / total as f64) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Interleaved stereo sine, the same on both channels, `amp` linear peak.
    fn sine(rate: u32, hz: f64, amp: f64, secs: f64) -> Vec<f32> {
        let n = (rate as f64 * secs) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = (amp * libm::sin(2.0 * core::f64::consts::PI * hz * i as f64 / rate as f64)) as f32;
            v.push(s);
            v.push(s);
        }
        v
    }

    /// Amplitude of a stereo sine that reads `lufs` (both channels, 1 kHz: 0 dBFS reads 0 LUFS).
    fn amp_for(lufs: f64) -> f64 {
        libm::pow(10.0, lufs / 20.0)
    }

    fn measure(rate: u32, ch: usize, parts: &[(Vec<f32>, usize)]) -> LoudnessMeter {
        let mut m = LoudnessMeter::new(rate, ch);
        for (p, repeat) in parts {
            for _ in 0..*repeat {
                m.process(p);
            }
        }
        m
    }

    fn close(a: f32, b: f64, tol: f64) {
        assert!((a as f64 - b).abs() <= tol, "{a} is not within {tol} of {b}");
    }

    #[test]
    fn k_weighting_matches_the_coefficients_of_the_standard_at_48k() {
        let [shelf, hp] = k_weighting(48_000.0);
        let near = |a: f64, b: f64| assert!((a - b).abs() < 1e-9, "{a} vs {b}");
        near(shelf.b0, 1.535_124_859_586_97);
        near(shelf.b1, -2.691_696_189_406_38);
        near(shelf.b2, 1.198_392_810_852_85);
        near(shelf.a1, -1.690_659_293_182_41);
        near(shelf.a2, 0.732_480_774_215_85);
        near(hp.a1, -1.990_047_454_833_98);
        near(hp.a2, 0.990_072_250_366_21);
    }

    // The EBU Tech 3341 minimum-requirement signals that are plain tones (stereo, 1 kHz); tolerance is the +-0.1 LU of the
    // document. Cases 1 and 2: a steady tone reads its own level in M, S and I.
    #[test]
    fn ebu_3341_cases_1_and_2_steady_tones() {
        for lufs in [-23.0, -33.0] {
            let m = measure(48_000, 2, &[(sine(48_000, 1000.0, amp_for(lufs), 1.0), 20)]);
            close(m.integrated_lufs().unwrap(), lufs, 0.1);
            close(m.short_term_lufs().unwrap(), lufs, 0.1);
            close(m.momentary_lufs().unwrap(), lufs, 0.1);
        }
    }

    // Case 3: 10 s at -36, 60 s at -23, 10 s at -36 is -23 (the quiet parts are under the relative gate).
    #[test]
    fn ebu_3341_case_3_relative_gate() {
        let m = measure(
            48_000,
            2,
            &[
                (sine(48_000, 1000.0, amp_for(-36.0), 1.0), 10),
                (sine(48_000, 1000.0, amp_for(-23.0), 1.0), 60),
                (sine(48_000, 1000.0, amp_for(-36.0), 1.0), 10),
            ],
        );
        close(m.integrated_lufs().unwrap(), -23.0, 0.1);
    }

    // Case 4 (absolute gate): audio at -72 LUFS is below the gate and must not drag a -62 LUFS passage down.
    #[test]
    fn ebu_3341_case_4_absolute_gate() {
        let m = measure(
            48_000,
            2,
            &[
                (sine(48_000, 1000.0, amp_for(-72.0), 1.0), 20),
                (sine(48_000, 1000.0, amp_for(-62.0), 1.0), 20),
                (sine(48_000, 1000.0, amp_for(-72.0), 1.0), 20),
            ],
        );
        close(m.integrated_lufs().unwrap(), -62.0, 0.1);
        // And nothing audible at all has no integrated loudness.
        let q = measure(48_000, 2, &[(sine(48_000, 1000.0, amp_for(-75.0), 1.0), 10)]);
        assert_eq!(q.integrated_lufs(), None);
    }

    // Case 5: 20 s at -26, 20.1 s at -20, 20 s at -26 integrate to -23 (all of it is above the relative gate).
    #[test]
    fn ebu_3341_case_5_two_levels() {
        let m = measure(
            48_000,
            2,
            &[
                (sine(48_000, 1000.0, amp_for(-26.0), 1.0), 20),
                (sine(48_000, 1000.0, amp_for(-20.0), 0.1), 201),
                (sine(48_000, 1000.0, amp_for(-26.0), 1.0), 20),
            ],
        );
        close(m.integrated_lufs().unwrap(), -23.0, 0.1);
    }

    #[test]
    fn every_sample_rate_reads_the_same_level() {
        for rate in [8_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000] {
            let m = measure(rate, 2, &[(sine(rate, 997.0, amp_for(-23.0), 1.0), 8)]);
            close(m.integrated_lufs().unwrap(), -23.0, 0.1);
        }
    }

    #[test]
    fn k_weighting_shapes_the_spectrum() {
        // The shelf lifts the highs and the high-pass cuts the lows.
        let at = |hz: f64| {
            let m = measure(48_000, 2, &[(sine(48_000, hz, 0.1, 1.0), 4)]);
            m.integrated_lufs().unwrap() as f64
        };
        let one_k = at(1000.0);
        let hi = at(8000.0) - one_k;
        let lo = at(30.0) - one_k;
        assert!(hi > 3.0 && hi < 4.5, "8 kHz sits about +3.9 dB over 1 kHz, got {hi}");
        assert!(lo < -2.0, "30 Hz is well under 1 kHz, got {lo}");
    }

    #[test]
    fn channel_weights_follow_the_standard() {
        // One channel is dual mono: the same as the same signal on both channels of a stereo pair.
        let mono: Vec<f32> =
            sine(48_000, 1000.0, amp_for(-23.0), 1.0).chunks_exact(2).map(|f| f[0]).collect();
        let m = measure(48_000, 1, &[(mono, 8)]);
        close(m.integrated_lufs().unwrap(), -23.0, 0.1);
        // 5.0: surrounds count 1.5 dB more, so a tone 1.5 dB lower in Ls and Rs reads the same as a stereo pair.
        let amp = amp_for(-24.5);
        let frames = sine(48_000, 1000.0, amp, 1.0);
        let mut five = Vec::new();
        for f in frames.chunks_exact(2) {
            five.extend_from_slice(&[0.0, 0.0, 0.0, f[0], f[1]]);
        }
        let m = measure(48_000, 5, &[(five, 8)]);
        close(m.integrated_lufs().unwrap(), -23.0, 0.1);
        // The LFE does not count.
        let mut six = Vec::new();
        for f in sine(48_000, 1000.0, amp_for(-23.0), 1.0).chunks_exact(2) {
            six.extend_from_slice(&[f[0], f[1], 0.0, 0.5, 0.0, 0.0]);
        }
        let m = measure(48_000, 6, &[(six, 8)]);
        close(m.integrated_lufs().unwrap(), -23.0, 0.1);
    }

    #[test]
    fn chunk_size_does_not_matter() {
        let all = sine(44_100, 440.0, 0.2, 5.0);
        let a = measure(44_100, 2, &[(all.clone(), 1)]);
        let mut b = LoudnessMeter::new(44_100, 2);
        for c in all.chunks(2 * 333) {
            b.process(c);
        }
        assert_eq!(a.integrated_lufs(), b.integrated_lufs());
        assert_eq!(a.frames(), b.frames());
    }

    // The true-peak cases of Tech 3341: the filter must find the peak between the samples of a tone whose samples never
    // reach it (tolerance +0.2 / -0.4 dB in the document).
    #[test]
    fn true_peak_finds_inter_sample_peaks() {
        let rate = 48_000;
        // fs/4 at 45 degrees: every sample is +-0.7071 while the waveform peaks at 1.0.
        let mut x = Vec::new();
        for i in 0..rate {
            let s = libm::sin(core::f64::consts::PI / 2.0 * i as f64 + core::f64::consts::PI / 4.0) as f32;
            x.push(s);
            x.push(s);
        }
        let mut m = LoudnessMeter::new(rate, 2).with_true_peak();
        m.process(&x);
        m.process(&[0.0; 64]); // flush the last samples through the filter
        close(m.sample_peak(), core::f64::consts::FRAC_1_SQRT_2, 0.001);
        let tp_db = 20.0 * libm::log10(m.true_peak().unwrap() as f64);
        assert!((-0.4..=0.2).contains(&tp_db), "true peak {tp_db} dBTP, expected 0");
        // A plain sine at -6 dBFS: true peak -6 dBTP.
        let mut m = LoudnessMeter::new(rate, 2).with_true_peak();
        m.process(&sine(rate, 997.0, amp_for(-6.0), 1.0));
        m.process(&[0.0; 64]);
        let tp_db = 20.0 * libm::log10(m.true_peak().unwrap() as f64);
        assert!((-6.4..=-5.8).contains(&tp_db), "{tp_db}");
        // Without the option there is no true peak.
        assert_eq!(LoudnessMeter::new(rate, 2).true_peak(), None);
    }

    #[test]
    fn true_peak_detector_is_the_same_in_chunks_and_whole() {
        let x: Vec<f32> = sine(48_000, 12_000.0, 0.9, 0.5).chunks_exact(2).map(|f| f[0]).collect();
        let tp = TruePeak::new();
        let whole = (0..x.len() - 8).map(|i| tp.around(&x, i)).fold(0.0f32, f32::max);
        assert!((0.9 * 0.99..0.92).contains(&whole), "{whole}");
    }

    #[test]
    fn combine_is_the_energy_mean() {
        // Two equally long tracks, 6 LU apart: the mean energy is 1.9 LU under the louder one... -(10 log10(0.625)) = 2.04.
        let l = combine_lufs(&[(-14.0, 100), (-20.0, 100)]).unwrap();
        close(l, -14.0 + 10.0 * libm::log10((1.0 + 0.251) / 2.0), 0.01);
        assert_eq!(combine_lufs(&[]), None);
        assert_eq!(combine_lufs(&[(f32::NEG_INFINITY, 10)]), None);
        // Length weights it.
        let l = combine_lufs(&[(-14.0, 300), (-20.0, 100)]).unwrap();
        assert!(l > -15.5 && l < -14.0);
    }

    #[test]
    fn silence_and_short_input_have_no_loudness() {
        let mut m = LoudnessMeter::new(48_000, 2);
        assert_eq!((m.momentary_lufs(), m.short_term_lufs(), m.integrated_lufs()), (None, None, None));
        m.process(&vec![0.0; 2 * 48_000]);
        assert_eq!(m.integrated_lufs(), None);
        assert!(m.momentary_lufs().is_some_and(|l| l == f32::NEG_INFINITY));
        assert_eq!(m.measurement(), None);
        m.reset();
        assert_eq!(m.frames(), 0);
    }
}
