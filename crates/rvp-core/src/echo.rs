//! "Echo out": the throw of a skipped song.
//!
//! When the listener skips a song in the middle, the song is not simply cut off: its last moments are thrown into a wet echo that
//! decays over about two seconds while the next song comes in underneath. A feedback delay fed with silence repeats the last stretch of
//! what was heard, each repeat quieter and a little duller; that whole tail is computed once when the skip happens (a few hundred
//! thousand multiplications, no state that lives on), and then mixed into the first two seconds of the next song's audio as it is made.
//! Nothing here runs while a song plays normally, and the incoming song never passes through a delay, so it gains no latency.
//!
//! Pure functions and plain buffers; `no_std + alloc`.
use alloc::vec::Vec;

/// The repeat time: an eighth note at 100 beats a minute.
pub const DELAY_MS: u32 = 300;
/// How much of each repeat is fed back.
pub const FEEDBACK: f32 = 0.68;
/// The level of the first repeat against the song it echoes.
pub const WET: f32 = 0.95;
/// How long the echo lasts; its level also falls in a straight line to zero over this time.
pub const TAIL_MS: u32 = 2000;
/// The song is carried on for this long, fading out, after the point where it was cut (the part that was queued but not yet heard), so
/// the cut itself never clicks.
pub const DRY_FADE_MS: u32 = 15;
/// The next song rises over this long (a raised cosine).
pub const FADE_IN_MS: u32 = 500;
/// Each repeat starts and ends with a ramp this long, so the repeat points are smooth.
const EDGE_MS: u32 = 5;
/// Each repeat is a little duller than the one before (a one-pole low-pass, this much of the new sample).
const DAMP: f32 = 0.5;
/// Values below this are flushed to zero (denormals slow the CPU down and mean nothing).
const FLUSH: f32 = 1.0e-20;

/// The tail of a skipped song, and the way the next song comes in over it.
#[derive(Debug, Clone, PartialEq)]
pub struct Throw {
    tail: Vec<f32>,
    channels: usize,
    /// Frames of the tail already played (written ahead of the next song or mixed into it).
    pos: usize,
    /// Frames of the next song already mixed (it rises over `fade_in`).
    song_pos: usize,
    fade_in: usize,
}

fn frames(ms: u32, rate: u32) -> usize {
    (rate as u64 * ms as u64 / 1000) as usize
}

/// A soft ceiling: untouched up to 0.9, then bent smoothly towards 1.0 so the sum of the song and its echo never clips.
fn ceil(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.9 { x } else { libm::copysignf(0.9 + 0.1 * libm::tanhf((a - 0.9) / 0.1), x) }
}

impl Throw {
    /// Build the tail. `heard` is what was audible just before the skip, interleaved, ending at the point of the cut; `after` is what had been
    /// queued beyond that point (it may be empty). Returns `None` when there is less than half a repeat of sound to echo.
    pub fn build(heard: &[f32], after: &[f32], channels: usize, rate: u32) -> Option<Throw> {
        let ch = channels.max(1);
        let d = frames(DELAY_MS, rate);
        let n = frames(TAIL_MS, rate);
        let heard_frames = heard.len() / ch;
        if d == 0 || heard_frames < d / 2 {
            return None;
        }
        // The chunk that repeats: the last `d` frames heard (fewer if that is all there is, padded in front with silence).
        let take = heard_frames.min(d);
        let mut chunk = alloc::vec![0.0f32; d * ch];
        chunk[(d - take) * ch..].copy_from_slice(&heard[(heard_frames - take) * ch..heard_frames * ch]);
        // Smooth edges so a repeat never starts or stops on a step.
        let edge = frames(EDGE_MS, rate).clamp(1, d / 2);
        for i in 0..edge {
            let g = i as f32 / edge as f32;
            for c in 0..ch {
                chunk[i * ch + c] *= g;
                chunk[(d - 1 - i) * ch + c] *= g;
            }
        }
        let mut tail = alloc::vec![0.0f32; n * ch];
        // The queued remainder of the song, fading out: the cut does not click.
        let fade = frames(DRY_FADE_MS, rate).max(1);
        for i in 0..(after.len() / ch).min(fade).min(n) {
            let g = 1.0 - i as f32 / fade as f32;
            for c in 0..ch {
                tail[i * ch + c] += after[i * ch + c] * g;
            }
        }
        // The repeats: each one fainter, duller and under the straight-line fade of the whole tail.
        let mut k = 0usize;
        let mut gain = WET;
        while k * d < n {
            let start = k * d;
            for i in 0..d.min(n - start) {
                let env = 1.0 - (start + i) as f32 / n as f32;
                for c in 0..ch {
                    tail[(start + i) * ch + c] += chunk[i * ch + c] * gain * env;
                }
            }
            // The next repeat comes from this one through a low-pass.
            for c in 0..ch {
                let mut y = 0.0f32;
                for i in 0..d {
                    let x = chunk[i * ch + c];
                    y += DAMP * (x - y);
                    chunk[i * ch + c] = y;
                }
            }
            gain *= FEEDBACK;
            k += 1;
        }
        for v in &mut tail {
            if v.abs() < FLUSH {
                *v = 0.0;
            }
        }
        Some(Throw { tail, channels: ch, pos: 0, song_pos: 0, fade_in: frames(FADE_IN_MS, rate) })
    }

    /// Only the end of the song, faded out over [`DRY_FADE_MS`], and the next song rising over a few milliseconds: what a skip does when the echo
    /// is switched off (or for Back), so the cut never clicks. `after` is what had been queued beyond the cut.
    pub fn declick(after: &[f32], channels: usize, rate: u32) -> Option<Throw> {
        let ch = channels.max(1);
        let fade = frames(DRY_FADE_MS, rate).max(1);
        let n = (after.len() / ch).min(fade);
        if n == 0 {
            return None;
        }
        let mut tail = alloc::vec![0.0f32; fade * ch];
        for i in 0..n {
            let g = 1.0 - i as f32 / fade as f32;
            for c in 0..ch {
                tail[i * ch + c] = after[i * ch + c] * g;
            }
        }
        Some(Throw { tail, channels: ch, pos: 0, song_pos: 0, fade_in: frames(DRY_FADE_MS, rate) })
    }

    /// The next `n` frames of the tail by themselves (interleaved), to play before the next song is ready; fewer when the tail is over. The song
    /// then joins at the place the tail has reached.
    pub fn lead(&mut self, n: usize) -> Vec<f32> {
        let total = self.len_frames();
        let take = n.min(total.saturating_sub(self.pos));
        let ch = self.channels;
        let mut out: Vec<f32> = self.tail[self.pos * ch..(self.pos + take) * ch].to_vec();
        for v in &mut out {
            *v = ceil(*v);
        }
        self.pos += take;
        out
    }

    /// True once any of the tail has been played on its own.
    pub fn started(&self) -> bool {
        self.pos > 0
    }

    /// Take back `n` frames of what [`Throw::lead`] gave, when the sink accepted fewer.
    pub fn unlead(&mut self, n: usize) {
        self.pos = self.pos.saturating_sub(n);
    }

    /// Frames of the tail (its length in time at the rate it was built for).
    pub fn len_frames(&self) -> usize {
        self.tail.len() / self.channels
    }

    /// True once the tail is over and the next song has risen fully.
    pub fn done(&self) -> bool {
        self.pos >= self.len_frames() && self.song_pos >= self.fade_in
    }

    /// Mix the tail into `song`, the next frames of the incoming song (interleaved): the song rises over [`FADE_IN_MS`], the tail is added, and
    /// the sum is kept below full scale.
    pub fn apply(&mut self, song: &mut [f32]) {
        let ch = self.channels;
        let total = self.len_frames();
        for (j, frame) in song.chunks_exact_mut(ch).enumerate() {
            let (p, sp) = (self.pos + j, self.song_pos + j);
            if p >= total && sp >= self.fade_in {
                break;
            }
            let rise = if sp < self.fade_in {
                0.5 - 0.5 * libm::cosf(core::f32::consts::PI * sp as f32 / self.fade_in as f32)
            } else {
                1.0
            };
            for (c, s) in frame.iter_mut().enumerate() {
                let echo = if p < total { self.tail[p * ch + c] } else { 0.0 };
                *s = ceil(*s * rise + echo);
            }
        }
        let n = song.len() / ch;
        self.pos += n;
        self.song_pos += n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn tone(ms: u32, ch: usize) -> Vec<f32> {
        let n = frames(ms, RATE);
        let mut v = Vec::new();
        for i in 0..n {
            let s = 0.8 * libm::sinf(2.0 * core::f32::consts::PI * 440.0 * i as f32 / RATE as f32);
            for _ in 0..ch {
                v.push(s);
            }
        }
        v
    }

    #[test]
    fn the_tail_decays_to_silence_with_each_repeat_fainter() {
        let t = Throw::build(&tone(500, 2), &[], 2, RATE).unwrap();
        assert_eq!(t.len_frames(), frames(TAIL_MS, RATE));
        let d = frames(DELAY_MS, RATE);
        let peak =
            |from: usize, to: usize| t.tail[from * 2..to * 2].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        // The first repeat is loud (a wet echo), and each one after it is quieter.
        let peaks: Vec<f32> = (0..6).map(|k| peak(k * d, (k + 1) * d)).collect();
        assert!(peaks[0] > 0.5, "{peaks:?}");
        assert!(peaks.windows(2).all(|w| w[1] < w[0]), "{peaks:?}");
        // The end is exactly silent: nothing is left to ring on into the next song.
        let end = &t.tail[(t.len_frames() - 64) * 2..];
        assert!(end.iter().all(|v| v.abs() < 0.01), "{end:?}");
        assert!(t.tail[t.tail.len() - 2..].iter().all(|v| v.abs() < 1e-5));
    }

    #[test]
    fn nothing_is_denormal_and_nothing_is_not_a_number() {
        let t = Throw::build(&tone(400, 2), &tone(30, 2), 2, RATE).unwrap();
        for v in &t.tail {
            assert!(v.is_finite());
            assert!(*v == 0.0 || v.abs() >= f32::MIN_POSITIVE, "denormal {v}");
        }
    }

    #[test]
    fn the_song_and_its_echo_never_clip_and_the_song_rises_smoothly() {
        let mut t = Throw::build(&tone(500, 2), &[], 2, RATE).unwrap();
        // A loud incoming song: the sum stays inside full scale all the way.
        let mut peak = 0.0f32;
        let mut first = 0.0f32;
        let mut last_step = 0.0f32;
        let mut prev = 0.0f32;
        for block in 0..(3 * RATE as usize / 480) {
            let mut song = alloc::vec![0.95f32; 480 * 2];
            t.apply(&mut song);
            if block == 0 {
                first = song[0].abs();
            }
            for f in song.chunks_exact(2) {
                peak = peak.max(f[0].abs());
                last_step = last_step.max((f[0] - prev).abs());
                prev = f[0];
            }
        }
        assert!(peak <= 1.0, "peak {peak}");
        assert!(first < 0.01, "the song starts from silence ({first})");
        assert!(last_step < 0.08, "no steps: {last_step}");
        assert!(t.done());
        // Once done the song passes through untouched.
        let mut song = alloc::vec![0.5f32; 8];
        t.apply(&mut song);
        assert_eq!(song, alloc::vec![0.5f32; 8]);
    }

    #[test]
    fn the_cut_is_carried_on_for_a_few_milliseconds_and_faded() {
        let after = alloc::vec![0.5f32; frames(40, RATE) * 2];
        let t = Throw::build(&tone(400, 2), &after, 2, RATE).unwrap();
        // The queued remainder is faded to nothing over the dry fade (the echo of the chunk adds on top; check against a build without it).
        let bare = Throw::build(&tone(400, 2), &[], 2, RATE).unwrap();
        let dry = |i: usize| t.tail[i * 2] - bare.tail[i * 2];
        assert!(dry(0) > 0.45);
        assert!(dry(frames(DRY_FADE_MS, RATE) - 1) < 0.01);
        assert!(dry(frames(DRY_FADE_MS, RATE) + 10).abs() < 1e-6);
    }

    #[test]
    fn too_little_sound_to_echo_makes_no_tail_and_mono_works() {
        assert!(Throw::build(&tone(50, 2), &[], 2, RATE).is_none());
        assert!(Throw::build(&[], &[], 2, RATE).is_none());
        let t = Throw::build(&tone(400, 1), &[], 1, RATE).unwrap();
        assert_eq!(t.len_frames(), frames(TAIL_MS, RATE));
    }
}
