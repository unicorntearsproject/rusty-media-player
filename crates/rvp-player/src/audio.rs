//! The audio output pipeline: channel mixing, resampling to the sink rate, start trimming, and the
//! stream-time bookkeeping the master clock needs.
//!
//! Timeline model: after an *anchor* (stream start, seek, or a timestamp jump) the stream time of every output
//! frame is `origin + frames_since_origin / rate`. Packet timestamps are only trusted to detect jumps, because a
//! container rounds them to its tick (1 ms in Matroska), which would make a clock built on them jitter.
use alloc::vec::Vec;
use rvp_core::{AudioBuffer, AudioParams, Resampler, TimeStretcher, Timestamp};
use rvp_host::AudioSink;

/// A timestamp discontinuity larger than this re-anchors the timeline.
const JUMP_US: i64 = 10_000;

/// One chunk handed to the sink, for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceEntry {
    /// Stream time of the first frame, microseconds.
    pub pts: Timestamp,
    /// Frames in the chunk (at the sink rate).
    pub frames: usize,
}

/// A stretch of output with one continuous stream timeline: a new one starts at a seek, a timestamp jump, or the
/// first frame of the next item in a gapless chain.
#[derive(Debug, Clone, Copy)]
struct Seg {
    /// Output frame index (counted since the last reset) of the segment's first frame.
    start_frame: u64,
    /// Stream time of that frame.
    origin: Timestamp,
    /// Which playlist item the segment belongs to.
    item: u32,
}

/// Converts decoded audio to the sink format and keeps track of its stream time.
pub struct AudioOut {
    sink: AudioParams,
    resampler: Option<Resampler>,
    in_rate: u32,
    /// Interleaved, sink format, not yet accepted by the sink.
    pending: Vec<f32>,
    pending_off: usize,
    /// Expected stream time of the next input frame (advances with every input frame, kept or dropped).
    expected: Option<Timestamp>,
    /// Timeline segments, oldest first; the last one is being appended to.
    segs: Vec<Seg>,
    /// The next kept frame starts a new segment.
    need_seg: bool,
    /// Item number stamped on new segments.
    cur_item: u32,
    /// Frames pushed to `pending` since the last reset, at the sink rate.
    pushed: u64,
    /// Frames the sink has accepted since the last reset.
    written: u64,
    discard_until: Timestamp,
    trace: Option<Vec<TraceEntry>>,
    /// Playback rate: the input is consumed `rate` times faster than real time (pitch is kept by WSOLA).
    rate: f64,
    stretcher: Option<TimeStretcher>,
    /// Copy what the sink accepts so the visualizer tap can be fed what is being heard.
    tap: bool,
    tap_buf: Vec<f32>,
    /// Output frame index of the first frame in `tap_buf`.
    tap_base: u64,
}

impl AudioOut {
    /// A pipeline for a sink that accepts `sink`.
    pub fn new(sink: AudioParams) -> Self {
        Self {
            sink,
            resampler: None,
            in_rate: sink.sample_rate,
            pending: Vec::new(),
            pending_off: 0,
            expected: None,
            segs: Vec::new(),
            need_seg: true,
            cur_item: 0,
            pushed: 0,
            written: 0,
            discard_until: 0,
            trace: None,
            rate: 1.0,
            stretcher: None,
            tap: false,
            tap_buf: Vec::new(),
            tap_base: 0,
        }
    }

    /// Set the playback rate. Only call this right after [`AudioOut::reset`]: audio already queued keeps the
    /// old rate. Rates other than 1.0 time-stretch the input with WSOLA, so the pitch is unchanged.
    pub fn set_rate(&mut self, rate: f64) {
        self.rate = rate.clamp(0.1, 8.0);
        self.stretcher = (self.rate != 1.0)
            .then(|| TimeStretcher::new(self.sink.sample_rate, self.sink.channels as usize, self.rate));
    }

    /// Stream microseconds covered by `frames` output frames at the current rate.
    fn frames_to_us(&self, frames: i64) -> i64 {
        let sr = self.sink.sample_rate as i64;
        if self.rate == 1.0 {
            frames * 1_000_000 / sr
        } else {
            (frames as f64 * 1_000_000.0 * self.rate / sr as f64) as i64
        }
    }

    /// Record every chunk pushed (for tests).
    pub fn enable_trace(&mut self) {
        self.trace = Some(Vec::new());
    }

    /// Chunks recorded so far.
    pub fn trace(&self) -> &[TraceEntry] {
        self.trace.as_deref().unwrap_or(&[])
    }

    /// Forget everything and drop audio before `discard_until` (stream time, microseconds). Audio earlier than
    /// time zero is always dropped.
    pub fn reset(&mut self, discard_until: Timestamp) {
        self.pending.clear();
        self.pending_off = 0;
        self.expected = None;
        self.segs.clear();
        self.need_seg = true;
        self.pushed = 0;
        self.written = 0;
        self.tap_buf.clear();
        self.tap_base = 0;
        self.discard_until = discard_until;
        if let Some(r) = &mut self.resampler {
            r.reset();
        }
        if let Some(t) = &mut self.stretcher {
            t.reset();
        }
    }

    /// Frames waiting to go to the sink.
    pub fn pending_frames(&self) -> usize {
        (self.pending.len() - self.pending_off) / self.sink.channels as usize
    }

    /// Stream time just after the last frame pushed, if any.
    pub fn end_pts(&self) -> Option<Timestamp> {
        self.segs.last().map(|s| s.origin + self.frames_to_us(self.pushed as i64 - s.start_frame as i64))
    }

    /// Set the item number for audio pushed from now on and start a new timeline segment with it (the next
    /// item of a gapless chain). Nothing queued is dropped, so there is no gap and no sink flush.
    pub fn begin_item(&mut self, item: u32) {
        self.cur_item = item;
        self.need_seg = true;
        self.expected = None;
        self.discard_until = 0;
    }

    /// Output frame position (frames since the reset, device latency removed) that is being heard now.
    fn heard_frame(&self, sink: &impl AudioSink) -> i64 {
        let played = self.written as i64 - sink.queued_frames() as i64;
        let lat = sink.output_latency_us() * self.sink.sample_rate as i64 / 1_000_000;
        played - lat
    }

    /// The segment being heard.
    fn heard_seg(&self, sink: &impl AudioSink) -> Option<Seg> {
        let pf = self.heard_frame(sink);
        self.segs.iter().rev().find(|s| s.start_frame as i64 <= pf).or(self.segs.first()).copied()
    }

    /// Turn the copy of the output for the visualizer tap on or off.
    pub fn set_tap(&mut self, on: bool) {
        if self.tap && !on {
            self.tap_buf.clear();
        }
        if !self.tap && on {
            // Start from what is being heard now, not from everything written so far.
            self.tap_buf.clear();
            self.tap_base = self.written;
        }
        self.tap = on;
    }

    /// The output parameters (what the sink was opened with).
    pub fn sink_params(&self) -> AudioParams {
        self.sink
    }

    /// Stream time of output frame `frame` (counted since the last reset).
    fn frame_pts(&self, frame: u64) -> Timestamp {
        let seg = self.segs.iter().rev().find(|s| s.start_frame <= frame).or(self.segs.first());
        match seg {
            Some(s) => s.origin + self.frames_to_us(frame as i64 - s.start_frame as i64),
            None => 0,
        }
    }

    /// The part of the output that has been heard since the last call, with the stream time of its first frame.
    /// Only available while the tap is on.
    pub fn take_heard(&mut self, sink: &impl AudioSink) -> Option<(Timestamp, Vec<f32>)> {
        if !self.tap {
            return None;
        }
        let heard = self.heard_frame(sink);
        if heard <= self.tap_base as i64 {
            return None;
        }
        let ch = self.sink.channels as usize;
        let n =
            ((heard as u64).min(self.tap_base + (self.tap_buf.len() / ch) as u64) - self.tap_base) as usize;
        if n == 0 {
            return None;
        }
        let pts = self.frame_pts(self.tap_base);
        let samples: Vec<f32> = self.tap_buf.drain(..n * ch).collect();
        self.tap_base += n as u64;
        Some((pts, samples))
    }

    /// The item number of the audio being heard.
    pub fn heard_item(&self, sink: &impl AudioSink) -> Option<u32> {
        self.heard_seg(sink).map(|s| s.item)
    }

    /// Forget segments that are completely in the past (call from time to time while playing).
    pub fn prune(&mut self, sink: &impl AudioSink) {
        let pf = self.heard_frame(sink);
        while self.segs.len() > 1 && self.segs[1].start_frame as i64 <= pf {
            self.segs.remove(0);
        }
    }

    /// Append decoded audio.
    pub fn push(&mut self, buf: AudioBuffer) {
        let in_ch = buf.params.channels.max(1) as usize;
        let in_rate = buf.params.sample_rate.max(1);
        let frames = buf.samples.len() / in_ch;
        if frames == 0 {
            return;
        }
        // Format change or timestamp jump: new timeline.
        if in_rate != self.in_rate {
            self.in_rate = in_rate;
            self.resampler = None;
            self.expected = None;
        }
        if self.expected.is_none_or(|e| (buf.pts - e).abs() > JUMP_US) {
            // A jump: a new timeline segment starts at the next kept frame.
            self.need_seg = true;
            self.expected = Some(buf.pts);
        }
        let start = self.expected.unwrap_or(buf.pts);
        self.expected = Some(start + frames as i64 * 1_000_000 / in_rate as i64);

        // Trim what comes before the allowed start.
        let limit = self.discard_until.max(0);
        let mut skip = 0usize;
        if start < limit {
            skip = (((limit - start) as i128 * in_rate as i128 + 500_000) / 1_000_000) as usize;
            if skip >= frames {
                return;
            }
        }
        let kept_start = start + skip as i64 * 1_000_000 / in_rate as i64;
        let samples = &buf.samples[skip * in_ch..];

        let mixed = mix(samples, in_ch, self.sink.channels as usize);
        let before = self.pending.len();
        // First to the sink rate, then (for rates other than 1) time-stretched there.
        let at_sink: alloc::borrow::Cow<'_, [f32]> = if in_rate == self.sink.sample_rate {
            alloc::borrow::Cow::Borrowed(&mixed)
        } else {
            let sink_ch = self.sink.channels as usize;
            let r =
                self.resampler.get_or_insert_with(|| Resampler::new(in_rate, self.sink.sample_rate, sink_ch));
            let mut tmp = Vec::new();
            r.process(&mixed, &mut tmp);
            alloc::borrow::Cow::Owned(tmp)
        };
        match &mut self.stretcher {
            Some(t) => t.process(&at_sink, &mut self.pending),
            None => self.pending.extend_from_slice(&at_sink),
        }
        let added = (self.pending.len() - before) / self.sink.channels as usize;
        if self.need_seg || self.segs.is_empty() {
            self.need_seg = false;
            self.segs.push(Seg { start_frame: self.pushed, origin: kept_start, item: self.cur_item });
        }
        if let Some(t) = &mut self.trace {
            t.push(TraceEntry { pts: kept_start, frames: added });
        }
        self.pushed += added as u64;
    }

    /// Hand as much pending audio to the sink as it accepts. Returns frames written.
    pub fn drain(&mut self, sink: &mut impl AudioSink) -> usize {
        let ch = self.sink.channels as usize;
        let mut total = 0;
        while self.pending_off < self.pending.len() {
            let n = sink.write(&self.pending[self.pending_off..]);
            if n == 0 {
                break;
            }
            if self.tap {
                self.tap_buf.extend_from_slice(&self.pending[self.pending_off..self.pending_off + n * ch]);
            }
            self.pending_off += n * ch;
            self.written += n as u64;
            total += n;
        }
        if self.pending_off == self.pending.len() {
            self.pending.clear();
            self.pending_off = 0;
        } else if self.pending_off > (1 << 16) {
            self.pending.drain(..self.pending_off);
            self.pending_off = 0;
        }
        total
    }

    /// Stream time of the audio being heard right now: written position minus what the sink still holds and
    /// its output latency.
    pub fn heard_pts(&self, sink: &impl AudioSink) -> Option<Timestamp> {
        let seg = self.heard_seg(sink)?;
        Some(seg.origin + self.frames_to_us(self.heard_frame(sink) - seg.start_frame as i64))
    }

    /// Stream time of the first audio frame, once known.
    pub fn origin(&self) -> Option<Timestamp> {
        self.segs.first().map(|s| s.origin)
    }
}

/// Mix interleaved `inch` channels to `outch` (1 or 2). Standard film channel order is assumed for more than
/// two channels: FL FR FC LFE BL BR SL SR (LFE is dropped).
pub fn mix(samples: &[f32], inch: usize, outch: usize) -> Vec<f32> {
    if inch == outch {
        return samples.to_vec();
    }
    let frames = samples.len() / inch;
    let mut out = Vec::with_capacity(frames * outch);
    // (left weights, right weights) over input channels.
    let weights: Vec<(f32, f32)> = match inch {
        1 => alloc::vec![(1.0, 1.0)],
        2 => alloc::vec![(1.0, 0.0), (0.0, 1.0)],
        3 => alloc::vec![(1.0, 0.0), (0.0, 1.0), (0.707, 0.707)],
        4 => alloc::vec![(1.0, 0.0), (0.0, 1.0), (0.707, 0.0), (0.0, 0.707)],
        5 => alloc::vec![(1.0, 0.0), (0.0, 1.0), (0.707, 0.707), (0.707, 0.0), (0.0, 0.707)],
        _ => {
            let mut w =
                alloc::vec![(1.0, 0.0), (0.0, 1.0), (0.707, 0.707), (0.0, 0.0), (0.707, 0.0), (0.0, 0.707)];
            w.extend([(0.707, 0.0), (0.0, 0.707)]);
            w.resize(inch, (0.0, 0.0));
            w
        }
    };
    let norm_l: f32 = weights.iter().map(|w| w.0).sum::<f32>().max(1.0);
    let norm_r: f32 = weights.iter().map(|w| w.1).sum::<f32>().max(1.0);
    for f in samples.chunks_exact(inch) {
        let (mut l, mut r) = (0.0, 0.0);
        for (s, w) in f.iter().zip(&weights) {
            l += s * w.0;
            r += s * w.1;
        }
        if inch == 1 {
            l = f[0];
            r = f[0];
        } else if inch > 2 {
            l /= norm_l;
            r /= norm_r;
        }
        if outch == 1 {
            out.push((l + r) * 0.5);
        } else {
            out.push(l);
            out.push(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Sink {
        queued: usize,
        written: usize,
        cap: usize,
    }

    impl AudioSink for Sink {
        fn open(&mut self, w: AudioParams) -> Result<AudioParams, rvp_host::HostError> {
            Ok(w)
        }
        fn queued_frames(&self) -> usize {
            self.queued
        }
        fn output_latency_us(&self) -> Timestamp {
            0
        }
        fn write(&mut self, s: &[f32]) -> usize {
            let n = (s.len() / 2).min(self.cap.saturating_sub(self.queued));
            self.queued += n;
            self.written += n;
            n
        }
        fn flush(&mut self) {}
        fn set_paused(&mut self, _: bool) {}
        fn set_volume(&mut self, _: f32) {}
    }

    fn buf(pts: Timestamp, frames: usize, rate: u32) -> AudioBuffer {
        AudioBuffer {
            params: AudioParams { sample_rate: rate, channels: 2 },
            samples: alloc::vec![0.5; frames * 2],
            pts,
        }
    }

    #[test]
    fn trims_negative_start_exactly_and_ignores_rounded_timestamps() {
        let mut a = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        a.reset(0);
        // AAC priming: 1024 frames at -21333 us are dropped completely, the next buffer is kept.
        a.push(buf(-21_333, 1024, 48_000));
        assert_eq!(a.pending_frames(), 0);
        a.push(buf(1, 1024, 48_000)); // container rounding: 1 us late is still continuous
        assert_eq!(a.pending_frames(), 1024);
        assert!(a.origin().unwrap().abs() <= 1);
        a.push(buf(21_334, 1024, 48_000));
        assert_eq!(a.end_pts().unwrap(), 2 * 1024 * 1_000_000 / 48_000);
    }

    #[test]
    fn partial_trim_for_seek_target() {
        let mut a = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        a.reset(1_000_000);
        a.push(buf(990_000, 960, 48_000)); // 20 ms buffer, first 10 ms precede the target
        assert_eq!(a.pending_frames(), 480);
        assert_eq!(a.origin(), Some(1_000_000));
    }

    #[test]
    fn jump_starts_a_new_segment_without_disturbing_queued_audio() {
        let mut a = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        a.reset(0);
        a.push(buf(0, 480, 48_000));
        a.push(buf(5_000_000, 480, 48_000));
        assert_eq!(a.origin(), Some(0));
        assert_eq!(a.end_pts(), Some(5_010_000));
        let mut sink = Sink { cap: 1_000_000, ..Default::default() };
        let n = a.drain(&mut sink);
        assert_eq!(n, 960);
        sink.queued = n - 240; // 5 ms into the first segment
        assert_eq!(a.heard_pts(&sink), Some(5_000));
        sink.queued = n - 480 - 240; // 5 ms into the second
        assert_eq!(a.heard_pts(&sink), Some(5_005_000));
    }

    #[test]
    fn gapless_items_continue_without_a_gap_and_report_which_is_heard() {
        let mut a = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        a.reset(0);
        a.push(buf(0, 480, 48_000));
        a.begin_item(1);
        a.push(buf(0, 480, 48_000)); // the next item starts at its own time zero
        let mut sink = Sink { cap: 1_000_000, ..Default::default() };
        let n = a.drain(&mut sink);
        assert_eq!(n, 960, "nothing was dropped or inserted");
        sink.queued = n - 479;
        assert_eq!(a.heard_item(&sink), Some(0));
        sink.queued = n - 480;
        assert_eq!(a.heard_item(&sink), Some(1));
        assert_eq!(a.heard_pts(&sink), Some(0));
        a.prune(&sink);
        assert_eq!(a.origin(), Some(0));
    }

    #[test]
    fn resamples_to_sink_rate_and_heard_position_follows_the_sink() {
        let mut a = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        a.reset(0);
        for i in 0..10 {
            a.push(buf(i * 20_000, 882, 44_100)); // 20 ms at 44.1 kHz
        }
        assert!((a.pending_frames() as i64 - 9600).abs() < 40, "{}", a.pending_frames());
        let mut sink = Sink { cap: 1_000_000, ..Default::default() };
        let n = a.drain(&mut sink);
        assert_eq!(n, sink.written);
        // Sink has played 4800 frames (100 ms) of what it accepted.
        sink.queued = n - 4800;
        assert_eq!(a.heard_pts(&sink), Some(100_000));
    }

    #[test]
    fn mixing() {
        assert_eq!(mix(&[0.5, -0.5], 1, 2), [0.5, 0.5, -0.5, -0.5]);
        assert_eq!(mix(&[1.0, 0.0, 0.5, 0.5], 2, 1), [0.5, 0.5]);
        let m = mix(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 6, 2); // centre only
        assert!((m[0] - 0.707 / 2.414).abs() < 1e-3 && m[0] == m[1]);
    }
}
