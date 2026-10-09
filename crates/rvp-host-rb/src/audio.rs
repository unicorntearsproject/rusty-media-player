//! The audio sink on `audio_open`, `audio_write` and the clock calls.
//!
//! The player's master clock is `heard = written - queued_frames / rate - output_latency`, so the two answers must not overlap:
//! `audio_queued` counts only frames not yet handed to the device and `audio_latency_us` the delay after that (both defined so by
//! the API). When the OS has no such call (`-UNSUPPORTED`) the same numbers are derived from the `audio_clock` snapshot. Without an
//! audio device (no `AUDIO_OUT`, a failed open, a lost device) the sink is a stand-in driven by the clock, so video, seeking and the
//! position keep working, like the desktop host does.
use crate::api;
use bucket_v0_sys::{self as sys, err};
use rvp_core::{AudioParams, Timestamp};
use rvp_host::{AudioIssue, AudioIssueKind, AudioSink, HostError, retry_delay_us};
use std::cell::Cell;

/// A device-less ring drained by the clock.
#[derive(Debug)]
struct Silent {
    last_us: Cell<i64>,
    consumed: Cell<f64>,
}

/// `AudioSink` over a `bucket_v0` audio stream.
pub struct RbAudio {
    stream: Option<i32>,
    params: Option<AudioParams>,
    capacity: Cell<u32>,
    /// Frames written since open or flush (for the derived numbers and the stand-in).
    written: Cell<u64>,
    silent: Option<Silent>,
    paused: Cell<bool>,
    volume: f32,
    /// The native format of the output device as last reported (the stream keeps the granted one).
    native: Option<(u32, u32)>,
    /// A failure the driver has not yet shown to the user.
    failure: Option<i32>,
    /// The failure being shown and retried: the stand-in plays on silently meanwhile, and `maintain` opens the device again with a growing
    /// pause until it works.
    issue: Option<AudioIssue>,
    attempt: u32,
    next_try_us: i64,
    recovered: bool,
}

impl RbAudio {
    /// A closed sink.
    pub fn new() -> Self {
        Self {
            stream: None,
            params: None,
            capacity: Cell::new(0),
            written: Cell::new(0),
            silent: None,
            paused: Cell::new(false),
            volume: 1.0,
            native: None,
            failure: None,
            issue: None,
            attempt: 0,
            next_try_us: 0,
            recovered: false,
        }
    }

    /// True while the sound goes to a device (not the stand-in).
    pub fn has_device(&self) -> bool {
        self.stream.is_some()
    }

    /// The open stream's handle, if any.
    pub fn stream(&self) -> Option<i32> {
        self.stream
    }

    /// The native format of the device reported by `AUDIO_DEVICE_CHANGED`, if it ever was.
    pub fn native_format(&self) -> Option<(u32, u32)> {
        self.native
    }

    /// The device changed. The stream handle survives and the host keeps the granted rate (resampling if it must), so there is
    /// nothing to do but remember it.
    pub fn device_changed(&mut self, stream: i32, rate: u32, channels: u32) {
        if self.stream == Some(stream) {
            self.native = Some((rate, channels));
        }
    }

    /// The stream failed (`AUDIO_ERROR`, or a write that found it closed): play on silently and report it once.
    pub fn fail(&mut self, error: i32) {
        if let Some(h) = self.stream.take() {
            // SAFETY: no pointers.
            unsafe { sys::audio_close(h) };
            self.go_silent(self.written.get() as f64);
        }
        self.failure = Some(error);
        let kind = match error {
            sys::err::BUSY => AudioIssueKind::Busy,
            sys::err::NO_DEVICE | sys::err::CLOSED => AudioIssueKind::Gone,
            _ => AudioIssueKind::Other,
        };
        self.issue =
            Some(AudioIssue { kind, device: String::new(), reason: api::code_name(error).to_string() });
        self.attempt = 0;
        self.next_try_us = api::now_us() + retry_delay_us(0);
    }

    /// The error of a failure not yet reported (once).
    pub fn take_failure(&mut self) -> Option<i32> {
        self.failure.take()
    }

    /// The clock snapshot of the stream (`audio_clock`), for diagnostics and for the derived numbers.
    pub fn clock(&self) -> Option<sys::AudioClockInfo> {
        let h = self.stream?;
        let mut c = sys::AudioClockInfo { struct_size: 32, ..Default::default() };
        // SAFETY: `c` is the 32-byte struct.
        let r = unsafe { sys::audio_clock(h, (&mut c as *mut sys::AudioClockInfo).cast()) };
        (r >= 0).then_some(c)
    }

    fn go_silent(&mut self, consumed: f64) {
        self.silent = Some(Silent { last_us: Cell::new(api::now_us()), consumed: Cell::new(consumed) });
    }

    fn drain_silent(&self) {
        let (Some(s), Some(p)) = (&self.silent, self.params) else { return };
        let now = api::now_us();
        let dt = (now - s.last_us.replace(now)).max(0) as f64;
        if self.paused.get() {
            return;
        }
        let ceiling = self.written.get() as f64;
        s.consumed.set((s.consumed.get() + dt * f64::from(p.sample_rate) / 1e6).min(ceiling));
    }

    fn rate(&self) -> u32 {
        self.params.map_or(48_000, |p| p.sample_rate)
    }
}

impl Default for RbAudio {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSink for RbAudio {
    fn open(&mut self, want: AudioParams) -> Result<AudioParams, HostError> {
        if want.sample_rate == 0 || want.channels == 0 {
            return Err(HostError("no audio format".into()));
        }
        let want = AudioParams { sample_rate: want.sample_rate, channels: want.channels.min(2) };
        // The same format again: keep the stream and start it fresh (no close and reopen between items).
        if let (Some(h), Some(p)) = (self.stream, self.params) {
            if p == want {
                // SAFETY: no pointers.
                unsafe {
                    sys::audio_flush(h);
                    sys::audio_pause(h, 0);
                }
                self.written.set(0);
                self.paused.set(false);
                return Ok(p);
            }
        }
        if let Some(h) = self.stream.take() {
            // SAFETY: no pointers.
            unsafe { sys::audio_close(h) };
        }
        self.silent = None;
        self.written.set(0);
        self.paused.set(false);
        self.failure = None;
        let mut info = sys::AudioOpenInfo { struct_size: 16, ..Default::default() };
        // SAFETY: `info` is the 16-byte struct.
        let h = unsafe {
            sys::audio_open(
                want.sample_rate as i32,
                i32::from(want.channels),
                (&mut info as *mut sys::AudioOpenInfo).cast(),
            )
        };
        if h > 0 && info.rate > 0 && (1..=2).contains(&info.channels) {
            let got = AudioParams { sample_rate: info.rate, channels: info.channels as u16 };
            self.stream = Some(h);
            self.params = Some(got);
            self.capacity.set(info.capacity_frames.max(got.sample_rate));
            api::trace(&format!(
                "audio.open want={}x{} got={}x{} stream={h} capacity={}",
                want.sample_rate, want.channels, got.sample_rate, got.channels, info.capacity_frames
            ));
            // SAFETY: no pointers.
            unsafe { sys::audio_volume(h, self.volume) };
            return Ok(got);
        }
        api::warn(&format!(
            "no audio output ({}); playing silently",
            api::code_name(if h > 0 { err::INVALID } else { h })
        ));
        if h > 0 {
            // SAFETY: no pointers.
            unsafe { sys::audio_close(h) };
        }
        self.params = Some(want);
        self.capacity.set(want.sample_rate);
        self.go_silent(0.0);
        Ok(want)
    }

    fn issue(&self) -> Option<AudioIssue> {
        self.issue.clone()
    }

    fn maintain(&mut self) {
        if self.issue.is_none() || api::now_us() < self.next_try_us {
            return;
        }
        let Some(want) = self.params else { return };
        let _ = self.open(want);
        if self.stream.is_some() {
            self.issue = None;
            self.recovered = true;
        } else {
            self.attempt += 1;
            self.next_try_us = api::now_us() + retry_delay_us(self.attempt);
        }
    }

    fn take_recovered(&mut self) -> Option<String> {
        std::mem::take(&mut self.recovered).then(String::new)
    }

    fn queued_frames(&self) -> usize {
        if self.silent.is_some() {
            self.drain_silent();
            let consumed = self.silent.as_ref().map_or(0.0, |s| s.consumed.get());
            return (self.written.get() as f64 - consumed).max(0.0) as usize;
        }
        let Some(h) = self.stream else { return 0 };
        // SAFETY: no pointers.
        let q = unsafe { sys::audio_queued(h) };
        if q >= 0 {
            return q as usize;
        }
        // No `audio_queued`: what was written, minus what the clock says was heard, minus what is in the device's own delay.
        let Some(c) = self.clock() else { return 0 };
        let rate = f64::from(self.rate());
        let elapsed = if self.paused.get() { 0 } else { (api::now_us() - c.host_time_us).max(0) };
        let heard = c.frames_played as f64 + elapsed as f64 * rate / 1e6;
        let in_device = c.latency_us as f64 * rate / 1e6;
        (self.written.get() as f64 - heard - in_device).max(0.0) as usize
    }

    fn output_latency_us(&self) -> Timestamp {
        let Some(h) = self.stream else { return 0 };
        // SAFETY: no pointers.
        let l = unsafe { sys::audio_latency_us(h) };
        if l >= 0 { l } else { self.clock().map_or(0, |c| c.latency_us) }
    }

    fn write(&mut self, interleaved: &[f32]) -> usize {
        let ch = self.params.map_or(2, |p| usize::from(p.channels)).max(1);
        let frames = interleaved.len() / ch;
        if frames == 0 {
            return 0;
        }
        if let Some(h) = self.stream {
            // SAFETY: the buffer holds `frames` frames of `ch` samples.
            let n = unsafe { sys::audio_write(h, interleaved.as_ptr(), api::len32(frames)) };
            if n >= 0 {
                self.written.set(self.written.get() + n as u64);
                return n as usize;
            }
            if matches!(n, err::CLOSED | err::NO_DEVICE | err::NOT_FOUND) {
                self.fail(n);
            } else {
                return 0;
            }
        }
        // The stand-in: a ring of one second, drained by the clock.
        let queued = self.queued_frames();
        let room = (self.capacity.get() as usize).saturating_sub(queued);
        let n = frames.min(room);
        self.written.set(self.written.get() + n as u64);
        n
    }

    fn flush(&mut self) {
        self.written.set(0);
        if let Some(h) = self.stream {
            // SAFETY: no pointers.
            unsafe { sys::audio_flush(h) };
        }
        if let Some(s) = &self.silent {
            s.consumed.set(0.0);
            s.last_us.set(api::now_us());
        }
    }

    fn set_paused(&mut self, paused: bool) {
        if self.silent.is_some() {
            self.drain_silent();
        }
        if self.paused.get() != paused {
            api::trace(&format!("audio.paused {paused}"));
        }
        self.paused.set(paused);
        if let Some(h) = self.stream {
            // SAFETY: no pointers.
            unsafe { sys::audio_pause(h, paused as i32) };
        }
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(h) = self.stream {
            // SAFETY: no pointers.
            unsafe { sys::audio_volume(h, self.volume) };
        }
    }
}
