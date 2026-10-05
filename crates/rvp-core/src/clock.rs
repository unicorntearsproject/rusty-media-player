//! The master clock: maps system time to stream time. See `docs/PLAN.md` section 6.
//!
//! The clock holds no time source. Every call passes the current system time (`sys_us`, from the host
//! clock), which keeps it deterministic and unit-testable.
use crate::time::Timestamp;

/// Where the clock gets its truth from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSource {
    /// Free-running from the host clock (no audio, or audio unreliable).
    Monotonic,
    /// Slaved to the audio position reported by the audio pipeline.
    Audio,
}

/// Errors larger than this are treated as a discontinuity and re-anchored at once.
const DISCONTINUITY_US: i64 = 200_000;
/// Fraction of the audio error corrected per update (smoothing).
const SMOOTHING: f64 = 0.1;

/// Maps host time to stream time, with pause, rate and seek.
#[derive(Debug, Clone)]
pub struct MasterClock {
    source: ClockSource,
    paused: bool,
    rate: f64,
    anchor_stream_us: Timestamp,
    anchor_sys_us: Timestamp,
}

impl MasterClock {
    /// A paused clock at stream time 0.
    pub fn new(source: ClockSource) -> Self {
        Self { source, paused: true, rate: 1.0, anchor_stream_us: 0, anchor_sys_us: 0 }
    }

    /// Current source.
    pub fn source(&self) -> ClockSource {
        self.source
    }

    /// Switch source (e.g. audio underrun fallback) without moving the stream position.
    pub fn set_source(&mut self, source: ClockSource, sys_us: Timestamp) {
        self.reanchor(self.now_stream(sys_us), sys_us);
        self.source = source;
    }

    /// True if paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Playback rate (1.0 = normal).
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Stream time at host time `sys_us`.
    pub fn now_stream(&self, sys_us: Timestamp) -> Timestamp {
        if self.paused {
            self.anchor_stream_us
        } else {
            self.anchor_stream_us + ((sys_us - self.anchor_sys_us) as f64 * self.rate) as i64
        }
    }

    /// Host time at which stream time `stream_us` is due. While paused, returns `None`.
    pub fn to_system(&self, stream_us: Timestamp) -> Option<Timestamp> {
        if self.paused || self.rate <= 0.0 {
            return None;
        }
        Some(self.anchor_sys_us + ((stream_us - self.anchor_stream_us) as f64 / self.rate) as i64)
    }

    /// Jump to `stream_us` (seek, track switch, loop). Always a hard re-anchor.
    pub fn seek(&mut self, stream_us: Timestamp, sys_us: Timestamp) {
        self.reanchor(stream_us, sys_us);
    }

    /// Start or resume running.
    pub fn resume(&mut self, sys_us: Timestamp) {
        if self.paused {
            self.anchor_sys_us = sys_us;
            self.paused = false;
        }
    }

    /// Freeze at the current stream time.
    pub fn pause(&mut self, sys_us: Timestamp) {
        if !self.paused {
            self.anchor_stream_us = self.now_stream(sys_us);
            self.anchor_sys_us = sys_us;
            self.paused = true;
        }
    }

    /// Change the playback rate without moving the stream position.
    pub fn set_rate(&mut self, rate: f64, sys_us: Timestamp) {
        let here = self.now_stream(sys_us);
        self.reanchor(here, sys_us);
        self.rate = rate.max(0.0);
    }

    /// Feed the audio position currently being heard (written position minus queued and device latency).
    /// Ignored unless the source is [`ClockSource::Audio`] and the clock is running.
    pub fn update_from_audio(&mut self, heard_stream_us: Timestamp, sys_us: Timestamp) {
        if self.source != ClockSource::Audio || self.paused {
            return;
        }
        let predicted = self.now_stream(sys_us);
        let err = heard_stream_us - predicted;
        if err.abs() > DISCONTINUITY_US {
            self.reanchor(heard_stream_us, sys_us);
        } else {
            self.reanchor(predicted + (err as f64 * SMOOTHING) as i64, sys_us);
        }
    }

    fn reanchor(&mut self, stream_us: Timestamp, sys_us: Timestamp) {
        self.anchor_stream_us = stream_us;
        self.anchor_sys_us = sys_us;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_runs_pauses_and_scales() {
        let mut c = MasterClock::new(ClockSource::Monotonic);
        assert_eq!(c.now_stream(5_000_000), 0); // paused
        c.resume(1_000_000);
        assert_eq!(c.now_stream(3_000_000), 2_000_000);
        c.pause(3_000_000);
        assert_eq!(c.now_stream(9_000_000), 2_000_000);
        c.resume(10_000_000);
        c.set_rate(2.0, 10_000_000);
        assert_eq!(c.now_stream(11_000_000), 4_000_000);
        assert_eq!(c.to_system(6_000_000), Some(12_000_000));
    }

    #[test]
    fn seek_reanchors() {
        let mut c = MasterClock::new(ClockSource::Monotonic);
        c.resume(0);
        c.seek(60_000_000, 1_000_000);
        assert_eq!(c.now_stream(2_000_000), 61_000_000);
    }

    #[test]
    fn audio_master_smooths_jitter_and_tracks_drift() {
        let mut c = MasterClock::new(ClockSource::Audio);
        c.resume(0);
        c.update_from_audio(0, 0);
        // The audio device runs 0.1% fast and reports +-3 ms of jitter; after 10 s of updates every
        // 10 ms the clock must stay within 5 ms of the ideal device position.
        let mut worst = 0i64;
        for i in 1..=1000i64 {
            let sys = i * 10_000;
            let ideal = sys + sys / 1000;
            let jitter = if i % 2 == 0 { 3_000 } else { -3_000 };
            c.update_from_audio(ideal + jitter, sys);
            if i > 100 {
                worst = worst.max((c.now_stream(sys) - ideal).abs());
            }
        }
        assert!(worst < 5_000, "worst error {worst} us");
    }

    #[test]
    fn audio_discontinuity_hard_reanchors() {
        let mut c = MasterClock::new(ClockSource::Audio);
        c.resume(0);
        c.update_from_audio(5_000_000, 1_000_000);
        assert_eq!(c.now_stream(1_000_000), 5_000_000);
    }

    #[test]
    fn audio_updates_ignored_when_paused_or_monotonic() {
        let mut c = MasterClock::new(ClockSource::Audio);
        c.update_from_audio(9_000_000, 0);
        assert_eq!(c.now_stream(0), 0);
        let mut m = MasterClock::new(ClockSource::Monotonic);
        m.resume(0);
        m.update_from_audio(9_000_000, 0);
        assert_eq!(m.now_stream(0), 0);
    }
}
