//! The audio output pipeline: channel mixing, resampling to the sink rate, start trimming, the loudness gain and limiter, the
//! crossfade of two items, and the stream-time bookkeeping the master clock needs.
//!
//! Timeline model: after an *anchor* (stream start, seek, or a timestamp jump) the stream time of every output
//! frame is `origin + frames_since_origin / rate`. Packet timestamps are only trusted to detect jumps, because a
//! container rounds them to its tick (1 ms in Matroska), which would make a clock built on them jitter.
//!
//! The path of a decoded buffer: a *lane* (mix to the sink's channels, resample, trim) -> the lane's level gain (automatic
//! level) -> the time stretcher (speed other than 1) -> `pending` -> the limiter (automatic level on) -> the sink. Two lanes
//! exist at once only while one item is crossfaded into the next: their audio meets in the fade mixer, which gives the
//! first the falling and the second the rising half of an equal-power curve and sends the sum on as one stream.
use alloc::vec::Vec;
use rvp_core::dynamics::{GainStage, Limiter, equal_power};
use rvp_core::{AudioBuffer, AudioParams, LoudnessMeter, Resampler, TimeStretcher, Timestamp};
use rvp_host::AudioSink;

/// A timestamp discontinuity larger than this re-anchors the timeline.
const JUMP_US: i64 = 10_000;

/// The ceiling of the limiter that guards the automatic level, dBFS (true peak).
pub const LIMITER_CEILING_DB: f32 = -1.0;
/// Most the automatic level will turn a track up or down, dB.
const MAX_BOOST_DB: f32 = 12.0;
const MAX_CUT_DB: f32 = -24.0;
/// How fast the gain follows a known target (a track with a stated or measured loudness, or a changed setting), dB a second:
/// a 12 dB step takes 0.1 s.
const SLEW_KNOWN_DB_S: f32 = 120.0;
/// How fast the gain follows a running estimate: up slowly, down a little quicker, so a loud start is tamed before a quiet
/// one is lifted.
const SLEW_RUN_UP_DB_S: f32 = 1.5;
const SLEW_RUN_DOWN_DB_S: f32 = 4.0;
/// A running estimate is used once this many 400 ms blocks louder than the absolute gate have been measured (about 1.1 s of
/// sound), and its goal moves only when the estimate has changed by more than the dead band.
const RUN_MIN_BLOCKS: u64 = 8;
const RUN_DEADBAND_DB: f32 = 0.5;
/// The limiter lets go of its look-ahead (and accepts a shorter one) when the sink holds less than this, so the end of the
/// audio is never held back waiting for more.
const LIMITER_RELEASE_US: i64 = 20_000;

/// What the automatic level is asked to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelConfig {
    /// On: bring items to `target_lufs` and guard the peaks. Off: the audio is left exactly as it is.
    pub enabled: bool,
    /// The loudness aimed at, LUFS.
    pub target_lufs: f32,
}

impl Default for LevelConfig {
    fn default() -> Self {
        Self { enabled: false, target_lufs: -14.0 }
    }
}

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

/// What a lane made of one decoded buffer.
struct Converted {
    /// The buffer started a new timeline (the first one, or its time did not follow the last).
    jump: bool,
    /// Stream time of the first kept frame and the frames, in the sink's format (`None`: nothing left after trimming).
    kept: Option<(Timestamp, Vec<f32>)>,
}

/// The conversion of one item's decoded audio to the sink format, and that item's loudness gain.
struct Lane {
    resampler: Option<Resampler>,
    in_rate: u32,
    /// Expected stream time of the next input frame (advances with every input frame, kept or dropped).
    expected: Option<Timestamp>,
    discard_until: Timestamp,
    /// The item's loudness when known from its tags or the library (`None`: estimate it while it plays).
    known_lufs: Option<f32>,
    /// The running estimate, made only when it is needed.
    meter: Option<LoudnessMeter>,
    /// Where the running estimate wants the gain, dB, once there is one.
    run_goal: Option<f32>,
    gain: GainStage,
}

impl Lane {
    fn new(rate: u32) -> Self {
        Self {
            resampler: None,
            in_rate: rate,
            expected: None,
            discard_until: 0,
            known_lufs: None,
            meter: None,
            run_goal: None,
            gain: GainStage::default(),
        }
    }

    /// A lane for a new item: nothing is known about its level yet.
    fn start_item(&mut self, known_lufs: Option<f32>) {
        self.known_lufs = known_lufs;
        self.meter = None;
        self.run_goal = None;
    }

    /// Mix to the sink's channels, resample to its rate and trim what comes before `discard_until`.
    fn convert(&mut self, sink: AudioParams, buf: &AudioBuffer) -> Converted {
        let in_ch = buf.params.channels.max(1) as usize;
        let in_rate = buf.params.sample_rate.max(1);
        let frames = buf.samples.len() / in_ch;
        if frames == 0 {
            return Converted { jump: false, kept: None };
        }
        // Format change or timestamp jump: new timeline.
        if in_rate != self.in_rate {
            self.in_rate = in_rate;
            self.resampler = None;
            self.expected = None;
        }
        let mut jump = false;
        if self.expected.is_none_or(|e| (buf.pts - e).abs() > JUMP_US) {
            // A jump: a new timeline segment starts at the next kept frame.
            jump = true;
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
                return Converted { jump, kept: None };
            }
        }
        let kept_start = start + skip as i64 * 1_000_000 / in_rate as i64;
        let samples = &buf.samples[skip * in_ch..];

        let mixed = mix(samples, in_ch, sink.channels as usize);
        let at_sink = if in_rate == sink.sample_rate {
            mixed
        } else {
            let sink_ch = sink.channels as usize;
            let r = self.resampler.get_or_insert_with(|| Resampler::new(in_rate, sink.sample_rate, sink_ch));
            let mut tmp = Vec::new();
            r.process(&mixed, &mut tmp);
            tmp
        };
        Converted { jump, kept: Some((kept_start, at_sink)) }
    }

    /// Apply the loudness gain to `frames` (sink format).
    fn level(&mut self, cfg: LevelConfig, sink: AudioParams, frames: &mut [f32]) {
        if !cfg.enabled && self.gain.db() == 0.0 {
            return;
        }
        let ch = sink.channels as usize;
        let (goal, slew) = if !cfg.enabled {
            (0.0, SLEW_KNOWN_DB_S)
        } else if let Some(l) = self.known_lufs {
            ((cfg.target_lufs - l).clamp(MAX_CUT_DB, MAX_BOOST_DB), SLEW_KNOWN_DB_S)
        } else {
            let meter = self.meter.get_or_insert_with(|| LoudnessMeter::new(sink.sample_rate, ch));
            meter.process(frames);
            if meter.gated_blocks() >= RUN_MIN_BLOCKS {
                if let Some(l) = meter.integrated_lufs() {
                    let g = (cfg.target_lufs - l).clamp(MAX_CUT_DB, MAX_BOOST_DB);
                    if self.run_goal.is_none_or(|old| (g - old).abs() > RUN_DEADBAND_DB) {
                        self.run_goal = Some(g);
                    }
                }
            }
            let goal = self.run_goal.unwrap_or(0.0);
            (goal, if goal >= self.gain.db() { SLEW_RUN_UP_DB_S } else { SLEW_RUN_DOWN_DB_S })
        };
        self.gain.apply(frames, ch, sink.sample_rate, goal, slew);
    }
}

/// Audio of one item waiting in the fade mixer, in the sink format, with the stream time of its first frame.
#[derive(Default)]
struct FadeIn {
    data: Vec<f32>,
    /// Stream time of the frame at the start of `data` when it was last empty, and frames taken since (so the time stays exact
    /// however the audio is cut up).
    base: Timestamp,
    taken: u64,
}

impl FadeIn {
    fn push(&mut self, pts: Timestamp, frames: &[f32]) {
        if self.data.is_empty() {
            self.base = pts;
            self.taken = 0;
        }
        self.data.extend_from_slice(frames);
    }

    fn frames(&self, ch: usize) -> u64 {
        (self.data.len() / ch) as u64
    }

    /// Stream time of the first frame in `data`.
    fn pts(&self, rate: u32) -> Timestamp {
        self.base + (self.taken as i128 * 1_000_000 / rate as i128) as i64
    }

    fn take(&mut self, ch: usize, frames: u64) {
        let n = (frames as usize * ch).min(self.data.len());
        self.data.drain(..n);
        self.taken += frames;
    }
}

/// The two items of a crossfade meeting in the mixer.
struct Fade {
    lane_b: Lane,
    /// The first item's audio and the second's, waiting to be mixed.
    a: FadeIn,
    b: FadeIn,
    /// Length of the fade and how much of it has been mixed, frames.
    total: u64,
    done: u64,
    /// The item number the second item is given from the middle of the fade on.
    item_b: u32,
    mid_done: bool,
}

/// How a crossfade stands after [`AudioOut::fade_mix`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeStatus {
    /// More to mix.
    Running,
    /// Done: the second item's audio now follows on as the one stream.
    Finished,
}

/// Converts decoded audio to the sink format and keeps track of its stream time.
pub struct AudioOut {
    sink: AudioParams,
    lane: Lane,
    fade: Option<Fade>,
    level: LevelConfig,
    limiter: Option<Limiter>,
    /// Frames of `pending` (as a sample index) that have been through the limiter (or need not be).
    limited_end: usize,
    /// Interleaved, sink format, not yet accepted by the sink.
    pending: Vec<f32>,
    pending_off: usize,
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
            lane: Lane::new(sink.sample_rate),
            fade: None,
            level: LevelConfig::default(),
            limiter: None,
            limited_end: 0,
            pending: Vec::new(),
            pending_off: 0,
            segs: Vec::new(),
            need_seg: true,
            cur_item: 0,
            pushed: 0,
            written: 0,
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

    /// Turn the automatic level on or off and set its target. Audio already queued keeps the gain it was given; the gain
    /// moves to its new goal within a tenth of a second or so (no click).
    pub fn set_level(&mut self, cfg: LevelConfig) {
        self.level = cfg;
        if cfg.enabled && self.limiter.is_none() {
            let mut l = Limiter::new(self.sink.channels as usize, self.sink.sample_rate, LIMITER_CEILING_DB);
            l.reset();
            self.limiter = Some(l);
            // What is already queued has not been through the limiter and does not need to.
            self.limited_end = self.pending.len();
        } else if !cfg.enabled {
            self.limiter = None;
            self.limited_end = 0;
        }
    }

    /// The level settings in force.
    pub fn level(&self) -> LevelConfig {
        self.level
    }

    /// The loudness (LUFS) of the item being fed, when its tags or the library say (`None`: estimate it as it plays).
    pub fn set_item_loudness(&mut self, known_lufs: Option<f32>) {
        if self.lane.known_lufs != known_lufs {
            self.lane.known_lufs = known_lufs;
            if known_lufs.is_some() {
                self.lane.meter = None;
                self.lane.run_goal = None;
            }
        }
    }

    /// The gain the item being fed has now, dB (0 when the automatic level is off).
    pub fn current_gain_db(&self) -> f32 {
        self.lane.gain.db()
    }

    /// Forget everything and drop audio before `discard_until` (stream time, microseconds). Audio earlier than
    /// time zero is always dropped.
    pub fn reset(&mut self, discard_until: Timestamp) {
        self.pending.clear();
        self.pending_off = 0;
        self.limited_end = 0;
        self.fade = None;
        self.lane.expected = None;
        self.segs.clear();
        self.need_seg = true;
        self.pushed = 0;
        self.written = 0;
        self.tap_buf.clear();
        self.tap_base = 0;
        self.lane.discard_until = discard_until;
        if let Some(r) = &mut self.lane.resampler {
            r.reset();
        }
        if let Some(t) = &mut self.stretcher {
            t.reset();
        }
        if let Some(l) = &mut self.limiter {
            l.reset();
        }
    }

    /// Frames waiting to go to the sink (the last few milliseconds may be held back by the limiter until more arrive).
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
        self.lane.expected = None;
        self.lane.discard_until = 0;
        self.lane.start_item(None);
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
    /// Only available while the tap is on. It is what the sink was given: after the level and the fade.
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
        let c = self.lane.convert(self.sink, &buf);
        if c.jump {
            self.need_seg = true;
        }
        let Some((kept_start, mut frames)) = c.kept else { return };
        self.lane.level(self.level, self.sink, &mut frames);
        self.emit(kept_start, frames);
    }

    /// Time stretch (if the rate asks for it) and queue `frames` (sink format, whose first frame is stream time `pts`),
    /// starting a timeline segment if one is due.
    fn emit(&mut self, pts: Timestamp, frames: Vec<f32>) {
        if frames.is_empty() {
            return;
        }
        let before = self.pending.len();
        match &mut self.stretcher {
            Some(t) => t.process(&frames, &mut self.pending),
            None => self.pending.extend_from_slice(&frames),
        }
        let added = (self.pending.len() - before) / self.sink.channels as usize;
        if self.need_seg || self.segs.is_empty() {
            self.need_seg = false;
            self.segs.push(Seg { start_frame: self.pushed, origin: pts, item: self.cur_item });
        }
        if let Some(t) = &mut self.trace {
            t.push(TraceEntry { pts, frames: added });
        }
        self.pushed += added as u64;
    }

    // ---- crossfade --------------------------------------------------------------------------------------------------------

    /// Start crossfading into the next item: from now on [`AudioOut::push_fade_a`] takes what is left of the item playing and
    /// [`AudioOut::push_fade_b`] the start of the next, and [`AudioOut::fade_mix`] sends their equal-power sum on. The fade is
    /// `frames` long (frames of source time at the sink rate). The timeline and the item number change in the middle of it,
    /// where the two are equally loud. `known_lufs` is the loudness of the next item when its tags or the library say.
    pub fn begin_fade(&mut self, frames: u64, item_b: u32, known_lufs: Option<f32>) {
        let mut lane_b = Lane::new(self.sink.sample_rate);
        lane_b.start_item(known_lufs);
        // The second item starts at the gain it will keep when that is known, so the fade does not also move the gain.
        if self.level.enabled {
            if let Some(l) = known_lufs {
                lane_b.gain.set_now((self.level.target_lufs - l).clamp(MAX_CUT_DB, MAX_BOOST_DB));
            }
        }
        self.fade = Some(Fade {
            lane_b,
            a: FadeIn::default(),
            b: FadeIn::default(),
            total: frames.max(1),
            done: 0,
            item_b,
            mid_done: frames < 2,
        });
        if frames < 2 {
            self.cur_item = item_b;
            self.need_seg = true;
        }
    }

    /// True while a crossfade is being mixed.
    pub fn fading(&self) -> bool {
        self.fade.is_some()
    }

    /// Frames of the first and of the second item waiting in the fade mixer.
    pub fn fade_frames(&self) -> (usize, usize) {
        let ch = self.sink.channels as usize;
        self.fade.as_ref().map_or((0, 0), |f| (f.a.frames(ch) as usize, f.b.frames(ch) as usize))
    }

    /// Frames of the fade still to be mixed.
    pub fn fade_remaining(&self) -> u64 {
        self.fade.as_ref().map_or(0, |f| f.total - f.done)
    }

    /// Give the fade mixer decoded audio of the item that is ending.
    pub fn push_fade_a(&mut self, buf: AudioBuffer) {
        let c = self.lane.convert(self.sink, &buf);
        let Some((pts, mut frames)) = c.kept else { return };
        self.lane.level(self.level, self.sink, &mut frames);
        if let Some(f) = &mut self.fade {
            f.a.push(pts, &frames);
        }
    }

    /// Give the fade mixer decoded audio of the item that is starting.
    pub fn push_fade_b(&mut self, buf: AudioBuffer) {
        let sink = self.sink;
        let level = self.level;
        let Some(f) = &mut self.fade else { return };
        let c = f.lane_b.convert(sink, &buf);
        let Some((pts, mut frames)) = c.kept else { return };
        f.lane_b.level(level, sink, &mut frames);
        f.b.push(pts, &frames);
    }

    /// Mix as much of the fade as both items' audio allows and queue it. `a_over` says no more of the first item will
    /// come (it ended early: the rest of the fade is the second item alone), `b_over` the same for the second.
    pub fn fade_mix(&mut self, a_over: bool, b_over: bool) -> FadeStatus {
        let ch = self.sink.channels as usize;
        let rate = self.sink.sample_rate;
        loop {
            let Some(f) = &mut self.fade else { return FadeStatus::Finished };
            let remaining = f.total - f.done;
            if remaining == 0 {
                break;
            }
            let (fa, fb) = (f.a.frames(ch), f.b.frames(ch));
            if a_over && fa == 0 {
                break; // the first item is gone: the rest of the fade is the second item alone
            }
            let avail = if b_over && fb == 0 { fa } else { fa.min(fb) };
            let mut n = avail.min(remaining);
            if !f.mid_done {
                // Stop at the middle: the timeline changes there.
                n = n.min((f.total / 2).saturating_sub(f.done).max(1));
            }
            if n == 0 {
                return FadeStatus::Running;
            }
            let mut out: Vec<f32> = Vec::with_capacity(n as usize * ch);
            for i in 0..n as usize {
                // The gains at the middle of the frame.
                let t = (f.done as f32 + i as f32 + 0.5) / f.total as f32;
                let (ga, gb) = equal_power(t);
                for c in 0..ch {
                    let xa = f.a.data.get(i * ch + c).copied().unwrap_or(0.0);
                    let xb = f.b.data.get(i * ch + c).copied().unwrap_or(0.0);
                    out.push(xa * ga + xb * gb);
                }
            }
            let pts = if f.mid_done { f.b.pts(rate) } else { f.a.pts(rate) };
            f.a.take(ch, n);
            f.b.take(ch, n);
            f.done += n;
            let switch = !f.mid_done && f.done >= f.total / 2;
            if switch {
                f.mid_done = true;
            }
            let item_b = f.item_b;
            self.emit(pts, out);
            if switch {
                // From here on the audio, the clock and the now-playing information belong to the second item.
                self.cur_item = item_b;
                self.need_seg = true;
            }
        }
        self.finish_fade()
    }

    /// The fade is over (or the first item has nothing left): the second item goes on alone.
    fn finish_fade(&mut self) -> FadeStatus {
        let Some(f) = self.fade.take() else { return FadeStatus::Finished };
        if !f.mid_done {
            self.cur_item = f.item_b;
            self.need_seg = true;
        }
        let b_pts = f.b.pts(self.sink.sample_rate);
        let mut b = f.b.data;
        let ch = self.sink.channels as usize;
        if f.done < f.total && !b.is_empty() {
            // The first item ended early, with the second still coming in: a short ramp up to full, not a step.
            let (_, g0) = equal_power(f.done as f32 / f.total as f32);
            let ramp = (b.len() / ch).min(self.sink.sample_rate as usize / 100).max(1);
            for (i, frame) in b.chunks_exact_mut(ch).take(ramp).enumerate() {
                let g = g0 + (1.0 - g0) * (i as f32 + 1.0) / ramp as f32;
                for s in frame {
                    *s *= g;
                }
            }
        }
        self.lane = f.lane_b;
        self.emit(b_pts, b);
        FadeStatus::Finished
    }

    /// Abandon a crossfade in progress (a seek): whatever was mixed stays queued.
    pub fn abort_fade(&mut self) {
        self.fade = None;
    }

    /// Hand as much pending audio to the sink as it accepts. Returns frames written.
    pub fn drain(&mut self, sink: &mut impl AudioSink) -> usize {
        let ch = self.sink.channels as usize;
        // The limiter finishes what it can see enough of: all but its look-ahead, unless the sink is nearly out of audio.
        let ready_end = match &mut self.limiter {
            Some(lim) => {
                let queued_us = sink.queued_frames() as i64 * 1_000_000 / self.sink.sample_rate as i64;
                let hold = if queued_us >= LIMITER_RELEASE_US { lim.lookahead_frames() } else { 0 };
                let unlimited = (self.pending.len() - self.limited_end) / ch;
                let commit = unlimited.saturating_sub(hold);
                if commit > 0 {
                    lim.process(&mut self.pending[self.limited_end..], commit);
                    self.limited_end += commit * ch;
                }
                self.limited_end
            }
            None => self.pending.len(),
        };
        let mut total = 0;
        while self.pending_off < ready_end {
            let n = sink.write(&self.pending[self.pending_off..ready_end]);
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
            self.limited_end = 0;
        } else if self.pending_off > (1 << 16) {
            self.pending.drain(..self.pending_off);
            self.limited_end -= self.pending_off.min(self.limited_end);
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

/// Mix interleaved `inch` channels to `outch` (1 or 2). The standard film channel order is assumed for more than
/// two channels: FL FR FC LFE BL BR SL SR (LFE is dropped). The stereo mix is the usual ITU-style one (centre and
/// surrounds at -3 dB, a back centre at -6 dB) scaled so its largest row sums to one, which
/// is what ffmpeg's `aresample=rematrix_maxval=1.0` does: loud multichannel passages cannot clip.
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
        6 => alloc::vec![(1.0, 0.0), (0.0, 1.0), (0.707, 0.707), (0.0, 0.0), (0.707, 0.0), (0.0, 0.707)],
        // 6.1 (FL FR FC LFE BC SL SR): the back centre goes to both sides.
        7 => alloc::vec![
            (1.0, 0.0),
            (0.0, 1.0),
            (0.707, 0.707),
            (0.0, 0.0),
            (0.5, 0.5),
            (0.707, 0.0),
            (0.0, 0.707)
        ],
        // 7.1 (FL FR FC LFE BL BR SL SR): back and side surrounds, each at -3 dB.
        _ => {
            let mut w = alloc::vec![
                (1.0, 0.0),
                (0.0, 1.0),
                (0.707, 0.707),
                (0.0, 0.0),
                (0.707, 0.0),
                (0.0, 0.707),
                (0.707, 0.0),
                (0.0, 0.707)
            ];
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
        /// Everything accepted, interleaved.
        rec: Vec<f32>,
        /// A sink that always reports this many frames queued (a device that plays as fast as it is fed).
        fixed_queued: Option<usize>,
    }

    impl AudioSink for Sink {
        fn open(&mut self, w: AudioParams) -> Result<AudioParams, rvp_host::HostError> {
            Ok(w)
        }
        fn queued_frames(&self) -> usize {
            self.fixed_queued.unwrap_or(self.queued)
        }
        fn output_latency_us(&self) -> Timestamp {
            0
        }
        fn write(&mut self, s: &[f32]) -> usize {
            let n = (s.len() / 2).min(self.cap.saturating_sub(self.queued));
            self.queued += n;
            self.written += n;
            self.rec.extend_from_slice(&s[..n * 2]);
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

    // ---- crossfade -------------------------------------------------------------------------------------------------------

    fn tone(hz: f64, amp: f64, frames: usize, from: usize) -> Vec<f32> {
        (from..from + frames)
            .flat_map(|i| {
                let s = (amp * libm::sin(2.0 * core::f64::consts::PI * hz * i as f64 / 48_000.0)) as f32;
                [s, s]
            })
            .collect()
    }

    fn chunks(samples: &[f32], start_frame: usize, size: usize) -> Vec<AudioBuffer> {
        samples
            .chunks(size * 2)
            .enumerate()
            .map(|(i, c)| AudioBuffer {
                params: AudioParams { sample_rate: 48_000, channels: 2 },
                samples: c.to_vec(),
                pts: ((start_frame + i * size) as i64) * 1_000_000 / 48_000,
            })
            .collect()
    }

    /// Item A (440 Hz, 3 s) plays to 2 s, then fades for `fade` frames into item B (880 Hz, 3 s). Returns what the sink got.
    fn run_fade(fade: usize, a_len: usize) -> (AudioOut, Sink, Vec<f32>, Vec<f32>) {
        let mut out = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        out.reset(0);
        let mut sink = Sink { cap: 10_000_000, ..Default::default() };
        let a = tone(440.0, 0.5, a_len, 0);
        let b = tone(880.0, 0.5, 144_000, 0);
        let start = a_len - fade;
        for buf in chunks(&a[..start * 2], 0, 1000) {
            out.push(buf);
        }
        out.drain(&mut sink);
        out.begin_fade(fade as u64, 1, None);
        let mut a_tail = chunks(&a[start * 2..], start, 1000).into_iter();
        let mut b_head = chunks(&b, 0, 1000).into_iter();
        let mut b_rest = Vec::new();
        let mut a_done = false;
        while out.fading() {
            let mut moved = false;
            if out.fade_frames().0 < 2000 {
                if let Some(x) = a_tail.next() {
                    out.push_fade_a(x);
                    moved = true;
                } else {
                    a_done = true;
                }
            }
            if out.fade_frames().1 < 2000 {
                if let Some(x) = b_head.next() {
                    out.push_fade_b(x);
                    moved = true;
                }
            }
            out.fade_mix(a_done && out.fade_frames().0 == 0, false);
            out.drain(&mut sink);
            assert!(moved || !out.fading() || a_done, "the fade must make progress");
        }
        // The rest of the second item goes on as an ordinary stream.
        for x in b_head {
            b_rest.push(x.clone());
            out.push(x);
        }
        out.drain(&mut sink);
        (out, sink, a, b)
    }

    #[test]
    fn crossfade_is_the_equal_power_sum_and_the_rest_is_untouched() {
        let (fade, a_len) = (48_000, 144_000);
        let (_out, sink, a, b) = run_fade(fade, a_len);
        let start = a_len - fade;
        assert_eq!(sink.rec.len() / 2, a_len + 144_000 - fade, "the length is both items less the overlap");
        // Before the fade: item A exactly. After it: item B exactly.
        assert_eq!(&sink.rec[..start * 2], &a[..start * 2]);
        assert_eq!(&sink.rec[a_len * 2..], &b[fade * 2..]);
        // In it: A falling and B rising on a quarter circle, sample by sample.
        for k in 0..fade {
            let t = (k as f32 + 0.5) / fade as f32;
            let (ga, gb) = rvp_core::dynamics::equal_power(t);
            for c in 0..2 {
                let want = a[(start + k) * 2 + c] * ga + b[k * 2 + c] * gb;
                let got = sink.rec[(start + k) * 2 + c];
                assert!((got - want).abs() < 1e-5, "frame {k}: {got} vs {want}");
            }
        }
        // Two unrelated tones keep their combined power through the join: the level in 100 ms windows stays put.
        let rms = |from: usize| {
            let w = &sink.rec[from * 2..(from + 4800) * 2];
            libm::sqrt(w.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / w.len() as f64)
        };
        let before = rms(start - 9600);
        for w in 0..10 {
            let level = rms(start + w * 4800);
            let db: f64 = 20.0 * libm::log10(level / before);
            assert!(db.abs() < 0.3, "window {w}: {db} dB off the steady level");
        }
    }

    #[test]
    fn crossfade_switches_clock_and_item_in_the_middle() {
        let (fade, a_len) = (48_000, 144_000);
        let (out, mut sink, _a, _b) = run_fade(fade, a_len);
        let total = sink.written;
        let mid = a_len - fade + fade / 2;
        // Just before the middle the first item is heard, at its own time; from the middle on the second, at its own time
        // (half a second in, as the fade runs half-way).
        sink.queued = total - (mid - 1);
        assert_eq!(out.heard_item(&sink), Some(0));
        assert_eq!(out.heard_pts(&sink), Some((mid as i64 - 1) * 1_000_000 / 48_000));
        sink.queued = total - mid;
        assert_eq!(out.heard_item(&sink), Some(1));
        // (The lane adds up the length of each buffer as it goes, so a microsecond or two creeps in per buffer.)
        let near = |got: Option<Timestamp>, want: i64| {
            assert!(got.is_some_and(|g| (g - want).abs() < 20), "{got:?} {want}")
        };
        near(out.heard_pts(&sink), 500_000);
        sink.queued = total - (mid + 4800);
        near(out.heard_pts(&sink), 600_000);
        // And the end of the fade is a second into the second item.
        sink.queued = total - a_len;
        near(out.heard_pts(&sink), 1_000_000);
        // The sink got no gap and nothing twice: the timeline is the one of the second item to its end.
        sink.queued = 0;
        near(out.heard_pts(&sink), 3_000_000);
    }

    #[test]
    fn crossfade_survives_the_first_item_ending_early() {
        // The first item has only 0.4 s of the 1 s fade: the rest is the second item alone, brought up to full in a few ms.
        let fade = 48_000;
        let mut out = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        out.reset(0);
        let mut sink = Sink { cap: 10_000_000, ..Default::default() };
        let b = tone(880.0, 0.5, 144_000, 0);
        out.begin_fade(fade as u64, 1, None);
        for x in chunks(&tone(440.0, 0.5, 19_200, 0), 0, 1000) {
            out.push_fade_a(x);
        }
        for x in chunks(&b[..2 * 60_000], 0, 1000) {
            out.push_fade_b(x);
        }
        assert_eq!(out.fade_mix(true, false), FadeStatus::Finished);
        out.drain(&mut sink);
        assert!(!out.fading());
        assert_eq!(
            sink.rec.len() / 2,
            60_000,
            "all of the second item's audio came out, none was lost or doubled"
        );
        // No step anywhere: neighbouring samples of these tones never differ by much.
        let worst = sink.rec.windows(2).step_by(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.2, "{worst}");
        // The tail is the second item at full gain again.
        let n = sink.rec.len();
        assert_eq!(&sink.rec[n - 2000..], &b[2 * 60_000 - 2000..2 * 60_000]);
    }

    #[test]
    fn a_fade_that_is_too_short_to_split_does_not_get_stuck() {
        let mut out = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        out.reset(0);
        out.begin_fade(1, 5, None);
        for x in chunks(&tone(440.0, 0.5, 100, 0), 0, 100) {
            out.push_fade_a(x);
        }
        for x in chunks(&tone(880.0, 0.5, 100, 0), 0, 100) {
            out.push_fade_b(x);
        }
        assert_eq!(out.fade_mix(false, false), FadeStatus::Finished);
        assert!(!out.fading());
        let mut sink = Sink { cap: 1_000_000, ..Default::default() };
        let n = out.drain(&mut sink);
        assert_eq!(n, 100, "one frame of mixture, the second item's other 99");
        assert_eq!(out.heard_item(&sink), Some(5));
    }

    // ---- automatic level ----------------------------------------------------------------------------------------------------

    fn level_run(cfg: LevelConfig, known: Option<f32>, input: &[f32], queued: usize) -> (AudioOut, Sink) {
        let mut out = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        out.reset(0);
        out.set_level(cfg);
        out.set_item_loudness(known);
        let mut sink = Sink { cap: 100_000_000, fixed_queued: Some(queued), ..Default::default() };
        for buf in chunks(input, 0, 1024) {
            out.push(buf);
            out.drain(&mut sink);
        }
        (out, sink)
    }

    #[test]
    fn level_off_leaves_the_audio_exactly_alone() {
        let input = tone(440.0, 0.9, 48_000, 0);
        let (out, sink) = level_run(LevelConfig::default(), Some(-30.0), &input, 0);
        assert_eq!(sink.rec, input);
        assert_eq!(out.current_gain_db(), 0.0);
        // Also a known item that is already at the target and has no peak over the ceiling: nothing to do.
        let input = tone(440.0, 0.8, 48_000, 0);
        let (_o, sink) = level_run(LevelConfig { enabled: true, target_lufs: -14.0 }, Some(-14.0), &input, 0);
        let bad = sink.rec.iter().zip(&input).position(|(a, b)| a != b);
        assert!(sink.rec == input, "{bad:?} {} {}", sink.rec.len(), input.len());
    }

    #[test]
    fn known_loudness_sets_the_gain_within_a_tenth_of_a_second() {
        let input = tone(440.0, 0.05, 48_000, 0);
        let cfg = LevelConfig { enabled: true, target_lufs: -14.0 };
        // The track is at -24 LUFS: +10 dB.
        let (out, sink) = level_run(cfg, Some(-24.0), &input, 0);
        assert!((out.current_gain_db() - 10.0).abs() < 1e-3);
        let n = sink.rec.len();
        let peak = sink.rec[n - 9600..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((peak - 0.05 * 3.162_277).abs() < 0.002, "{peak}");
        // Up to full gain in about 80 ms (10 dB at 120 dB a second), smoothly: no frame is more than a hair away from the last.
        let early = sink.rec[..2 * 48_00].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(early < 0.05 * 3.2);
        // A loud track is turned down.
        let (out, _) = level_run(cfg, Some(-8.0), &input, 0);
        assert!((out.current_gain_db() + 6.0).abs() < 1e-3);
        // And the boost and the cut are limited.
        let (out, _) = level_run(cfg, Some(-60.0), &input, 0);
        assert_eq!(out.current_gain_db(), 12.0);
        let (out, _) = level_run(cfg, Some(10.0), &input, 0);
        assert_eq!(out.current_gain_db(), -24.0);
    }

    #[test]
    fn a_running_estimate_converges_without_pumping() {
        // A steady programme at -23 LUFS (stereo 997 Hz at -23 dBFS) with no tags: the gain finds +9 dB for -14 LUFS.
        let amp = libm::pow(10.0, -23.0 / 20.0);
        let input = tone(997.0, amp, 48_000 * 24, 0);
        let cfg = LevelConfig { enabled: true, target_lufs: -14.0 };
        let mut out = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        out.reset(0);
        out.set_level(cfg);
        let mut sink = Sink { cap: 100_000_000, ..Default::default() };
        let mut history: Vec<f32> = Vec::new();
        let mut last = 0;
        for buf in chunks(&input, 0, 480) {
            out.push(buf);
            out.drain(&mut sink);
            if sink.rec.len() / 2 - last >= 4800 {
                last = sink.rec.len() / 2;
                history.push(out.current_gain_db());
            }
        }
        // Nothing at first (the estimate needs a second or so of sound), then a slow rise, then steady at the right value.
        assert_eq!(history[0], 0.0);
        let fin = *history.last().unwrap();
        assert!((fin - 9.0).abs() < 0.6, "settled at {fin} dB");
        let max_step = history.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(max_step <= 0.16, "no more than 1.5 dB a second: {max_step} dB in 100 ms");
        // It never overshoots the goal by much and never swings back and forth.
        let peak = history.iter().copied().fold(f32::MIN, f32::max);
        assert!(peak < 9.6, "{peak}");
        let reversals = history
            .windows(3)
            .filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < 0.0 && (w[2] - w[0]).abs() > 0.05)
            .count();
        assert_eq!(reversals, 0, "{history:?}");
    }

    #[test]
    fn a_loud_start_is_brought_down_faster_than_a_quiet_one_comes_up() {
        let amp = libm::pow(10.0, -6.0 / 20.0);
        let input = tone(997.0, amp, 48_000 * 12, 0);
        let cfg = LevelConfig { enabled: true, target_lufs: -14.0 };
        let (out, _) = level_run(cfg, None, &input, 0);
        // -6 LUFS to -14 is -8 dB, reached at 4 dB a second.
        assert!((out.current_gain_db() + 8.0).abs() < 0.6, "{}", out.current_gain_db());
    }

    #[test]
    fn the_limiter_keeps_peaks_under_the_ceiling_and_holds_back_only_its_look_ahead() {
        let cfg = LevelConfig { enabled: true, target_lufs: -10.0 };
        // A -22 LUFS track brought up 12 dB: peaks of 0.9 would become 3.6.
        let input = tone(440.0, 0.9, 48_000, 0);
        let (_out, sink) = level_run(cfg, Some(-22.0), &input, 0);
        let ceiling = rvp_core::dynamics::db_to_gain(LIMITER_CEILING_DB);
        let peak = sink.rec.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak <= ceiling + 1e-6, "{peak}");
        assert!(peak > 0.85, "and it is still loud: {peak}");
        // With the sink well stocked the last milliseconds wait for more audio; with it nearly empty they are let go.
        let (out, sink) = level_run(cfg, Some(-22.0), &input, 20_000);
        assert!(out.pending_frames() > 0 && out.pending_frames() < 400, "{}", out.pending_frames());
        assert!(sink.rec.len() / 2 < 48_000);
        let (out, sink) = level_run(cfg, Some(-22.0), &input, 0);
        assert_eq!((out.pending_frames(), sink.rec.len() / 2), (0, 48_000));
    }

    #[test]
    fn two_items_with_different_levels_reach_the_same_loudness_through_a_fade() {
        // A at -24 LUFS and B at -34, both brought to -14: the fade starts B already at its own +20 -> clamped +12 gain.
        let cfg = LevelConfig { enabled: true, target_lufs: -14.0 };
        let mut out = AudioOut::new(AudioParams { sample_rate: 48_000, channels: 2 });
        out.reset(0);
        out.set_level(cfg);
        out.set_item_loudness(Some(-24.0));
        out.begin_fade(4800, 1, Some(-30.0));
        out.push_fade_a(chunks(&tone(440.0, 0.05, 4800, 0), 0, 4800).remove(0));
        out.push_fade_b(chunks(&tone(880.0, 0.0158, 4800, 0), 0, 4800).remove(0));
        out.fade_mix(false, false);
        assert!(!out.fading());
        // B's lane carried on, at its gain from the first frame of the fade.
        assert!((out.current_gain_db() - 12.0).abs() < 1e-3, "{}", out.current_gain_db());
    }
}
