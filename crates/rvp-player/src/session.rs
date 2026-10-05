//! A playback session: one opened media item, its demux and decode tasks, and the output side that feeds the
//! host's audio sink and keeps the master clock.
//!
//! Threading model (docs/PLAN.md section 5): everything runs on the caller's thread. `Session::tick` polls the
//! demux and decode tasks (cooperatively, within a time budget) and then does the synchronous output work:
//! feed the audio sink, update the clock. Tasks never touch the host; they communicate through [`Shared`].
use crate::audio::{AudioOut, TraceEntry};
use crate::exec::Executor;
use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;
use rvp_core::task::yield_now;
use rvp_core::{
    AudioBuffer, AudioParams, ClockSource, CodecFactory, Error, MasterClock, Packet, StreamInfo, StreamKind,
    Timestamp,
};
use rvp_demux::{Demuxer, open};
use rvp_host::{AudioSink, Host, Source};

/// Packets buffered between the demuxer and a decoder.
const MAX_PACKETS: usize = 128;
/// Decoded audio kept ahead of the output stage, microseconds.
const AUDIO_AHEAD_US: i64 = 500_000;
/// Audio we want queued (pending + sink) before starting playback, microseconds.
const START_BUFFER_US: i64 = 100_000;
/// Audio-only seeks start this much earlier and discard up to the target, so codecs with overlap state
/// (Opus, Vorbis, AAC, MP3) are warm when the target is reached.
const PREROLL_US: i64 = 100_000;
/// Preferred output format; the host may answer with something else.
const WANT_AUDIO: AudioParams = AudioParams { sample_rate: 48_000, channels: 2 };
/// How often `tick` wants to run while playing.
const TICK_US: i64 = 10_000;
/// Wall-clock budget for the task polling part of a tick.
const BUDGET_US: i64 = 8_000;

/// Playback state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Opening the file.
    Opening,
    /// Open; not playing.
    Paused,
    /// Play requested, waiting for enough decoded data (initial buffering or after a seek).
    Buffering,
    /// Playing.
    Playing,
    /// Everything played out.
    Ended,
    /// Opening failed; see [`Session::error`].
    Failed,
}

#[derive(Default)]
struct Shared {
    opened: bool,
    streams: Vec<StreamInfo>,
    duration_us: Option<Timestamp>,
    error: Option<Error>,
    sel_audio: Option<StreamInfo>,
    audio_in: VecDeque<Packet>,
    audio_dec: VecDeque<AudioBuffer>,
    audio_dec_us: i64,
    demux_done: bool,
    audio_done: bool,
    seek: Option<Timestamp>,
    seeking: bool,
    landed: Option<Timestamp>,
    generation: u32,
    /// Bumped whenever a task moves data; lets `tick` stop polling when nothing changes.
    progress: u64,
}

type Sh = Rc<RefCell<Shared>>;

fn buffer_us(b: &AudioBuffer) -> i64 {
    b.duration_us()
}

async fn demux_task<S: Source + 'static>(source: S, sh: Sh) {
    let mut d = match open(source).await {
        Ok(d) => d,
        Err(e) => {
            let mut s = sh.borrow_mut();
            s.error = Some(e);
            s.demux_done = true;
            return;
        }
    };
    {
        let mut s = sh.borrow_mut();
        s.streams = d.streams().to_vec();
        s.duration_us = d.duration_us();
        s.sel_audio = s.streams.iter().find(|i| i.kind == StreamKind::Audio).cloned();
        s.opened = true;
        s.progress += 1;
    }
    loop {
        let req = sh.borrow_mut().seek.take();
        if let Some(target) = req {
            let res = d.seek(target).await;
            let mut s = sh.borrow_mut();
            s.audio_in.clear();
            s.audio_dec.clear();
            s.audio_dec_us = 0;
            s.demux_done = false;
            s.audio_done = false;
            match res {
                Ok(l) => s.landed = Some(l),
                Err(e) => s.error = Some(e),
            }
            s.seeking = false;
            s.progress += 1;
            continue;
        }
        let blocked = {
            let s = sh.borrow();
            s.demux_done || s.audio_in.len() >= MAX_PACKETS
        };
        if blocked {
            yield_now().await;
            continue;
        }
        let res = d.next_packet().await;
        {
            let mut s = sh.borrow_mut();
            // A seek that arrived while this read was in flight makes the packet stale: drop it.
            if !(s.seek.is_some() || s.seeking) {
                match res {
                    Ok(Some(p)) => {
                        if s.sel_audio.as_ref().is_some_and(|a| a.id == p.stream_id) {
                            s.audio_in.push_back(p);
                        }
                        s.progress += 1;
                    }
                    Ok(None) => s.demux_done = true,
                    Err(e) => {
                        s.error = Some(e);
                        s.demux_done = true;
                    }
                }
            }
        }
        yield_now().await;
    }
}

async fn audio_task(sh: Sh, codecs: Rc<dyn CodecFactory>) {
    while !sh.borrow().opened {
        yield_now().await;
    }
    let info = sh.borrow().sel_audio.clone();
    let mut dec = match info.as_ref().map(|i| codecs.audio(i)) {
        Some(Ok(d)) => d,
        other => {
            let mut s = sh.borrow_mut();
            if let Some(Err(e)) = other {
                s.error = Some(e);
            }
            s.sel_audio = None;
            s.audio_done = true;
            return;
        }
    };
    let mut epoch = sh.borrow().generation;
    loop {
        let packet = {
            let mut s = sh.borrow_mut();
            if s.generation != epoch {
                epoch = s.generation;
                dec.flush();
            }
            if s.audio_dec_us >= AUDIO_AHEAD_US {
                None
            } else {
                let p = s.audio_in.pop_front();
                if p.is_none() && s.demux_done && !s.seeking {
                    s.audio_done = true;
                }
                p
            }
        };
        if let Some(p) = packet {
            // A packet that fails to decode is dropped; playback continues with the next one.
            if dec.send_packet(&p).is_ok() {
                while let Ok(Some(b)) = dec.receive_buffer() {
                    let mut s = sh.borrow_mut();
                    s.audio_dec_us += buffer_us(&b);
                    s.audio_dec.push_back(b);
                }
            }
            sh.borrow_mut().progress += 1;
        }
        yield_now().await;
    }
}

/// One opened media item and its playback machinery.
pub struct Session {
    exec: Executor,
    sh: Sh,
    clock: MasterClock,
    audio: Option<AudioOut>,
    audio_opened: bool,
    want_play: bool,
    running: bool,
    needs_sink_flush: bool,
    volume: f32,
    muted: bool,
    ended: bool,
    trace: bool,
    seek_target: Option<Timestamp>,
}

impl Session {
    /// Start opening `source`. Nothing happens until [`Session::tick`] is called.
    pub fn new<S: Source + 'static>(source: S, codecs: Rc<dyn CodecFactory>) -> Self {
        let sh: Sh = Rc::default();
        let mut exec = Executor::new();
        exec.spawn(demux_task(source, sh.clone()));
        exec.spawn(audio_task(sh.clone(), codecs));
        Self {
            exec,
            sh,
            clock: MasterClock::new(ClockSource::Monotonic),
            audio: None,
            audio_opened: false,
            want_play: false,
            running: false,
            needs_sink_flush: false,
            volume: 1.0,
            muted: false,
            ended: false,
            trace: false,
            seek_target: None,
        }
    }

    /// Record the audio chunks written (tests).
    pub fn enable_audio_trace(&mut self) {
        self.trace = true;
        if let Some(a) = &mut self.audio {
            a.enable_trace();
        }
    }

    /// Audio chunks recorded so far (see [`Session::enable_audio_trace`]).
    pub fn audio_trace(&self) -> &[TraceEntry] {
        self.audio.as_ref().map_or(&[], |a| a.trace())
    }

    /// Current state.
    pub fn state(&self) -> SessionState {
        let s = self.sh.borrow();
        if s.error.is_some() && !s.opened {
            SessionState::Failed
        } else if !s.opened {
            SessionState::Opening
        } else if self.ended {
            SessionState::Ended
        } else if self.running {
            SessionState::Playing
        } else if self.want_play {
            SessionState::Buffering
        } else {
            SessionState::Paused
        }
    }

    /// The last error, if any.
    pub fn error(&self) -> Option<Error> {
        self.sh.borrow().error.clone()
    }

    /// Streams in the container (empty until open).
    pub fn streams(&self) -> Vec<StreamInfo> {
        self.sh.borrow().streams.clone()
    }

    /// Duration if known.
    pub fn duration_us(&self) -> Option<Timestamp> {
        self.sh.borrow().duration_us
    }

    /// Start or resume playback.
    pub fn play(&mut self) {
        if self.ended {
            self.ended = false;
            self.seek(0);
        }
        self.want_play = true;
    }

    /// Pause. The position freezes.
    pub fn pause(&mut self, now_us: Timestamp) {
        self.want_play = false;
        if self.running {
            self.clock.pause(now_us);
            self.running = false;
        }
    }

    /// Set output volume, 0.0..=1.0.
    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
    }

    /// Mute or unmute.
    pub fn set_muted(&mut self, m: bool) {
        self.muted = m;
    }

    /// Playback position at host time `now_us`.
    pub fn position_us(&self, now_us: Timestamp) -> Timestamp {
        self.clock.now_stream(now_us).max(0)
    }

    /// Seek to `target_us`. Playback restarts (through `Buffering`) once audio is available at the target.
    pub fn seek(&mut self, target_us: Timestamp) {
        let target = target_us.max(0);
        let mut s = self.sh.borrow_mut();
        // Audio-only: start a little early so decoders warm up, then discard up to the target.
        let demux_target = if s.sel_audio.is_some() { (target - PREROLL_US).max(0) } else { target };
        s.generation = s.generation.wrapping_add(1);
        s.audio_in.clear();
        s.audio_dec.clear();
        s.audio_dec_us = 0;
        s.audio_done = false;
        s.demux_done = false;
        s.seek = Some(demux_target);
        s.seeking = true;
        drop(s);
        if let Some(a) = &mut self.audio {
            a.reset(target);
        }
        self.needs_sink_flush = true;
        self.ended = false;
        self.running = false;
        self.clock.seek(target, 0);
        self.clock.pause(0);
        self.seek_target = Some(target);
    }
}

impl Session {
    /// Advance the session: run tasks, feed the audio sink, update the clock. Call this regularly (the host
    /// clock's `request_wake` tells the host when).
    pub fn tick<H: Host>(&mut self, host: &mut H) {
        let t0 = host.clock().now_us();

        // 1. Demux and decode tasks, until quiescent or out of budget.
        for _ in 0..64 {
            let before = self.sh.borrow().progress;
            self.exec.poll_all();
            if self.sh.borrow().progress == before || host.clock().now_us() - t0 > BUDGET_US {
                break;
            }
        }
        let now = host.clock().now_us();

        // 2. Open the audio sink once we know the streams.
        let (opened, has_audio, seeking) = {
            let s = self.sh.borrow();
            (s.opened, s.sel_audio.is_some(), s.seeking)
        };
        if opened && !self.audio_opened {
            self.audio_opened = true;
            if has_audio {
                if let Ok(p) = host.audio().open(WANT_AUDIO) {
                    let mut out = AudioOut::new(p);
                    if self.trace {
                        out.enable_trace();
                    }
                    host.audio().set_paused(true);
                    self.audio = Some(out);
                    self.clock.set_source(ClockSource::Audio, now);
                }
            }
        }
        if !has_audio && self.audio.is_some() {
            self.audio = None; // decoder could not be created
            self.clock.set_source(ClockSource::Monotonic, now);
        }
        if self.needs_sink_flush && self.audio_opened {
            self.needs_sink_flush = false;
            if self.audio.is_some() {
                host.audio().flush();
            }
        }

        // 3. Feed the audio sink.
        if let Some(out) = &mut self.audio {
            if !seeking {
                let want = (WANT_AUDIO.sample_rate / 5) as usize;
                while out.pending_frames() < want {
                    let mut s = self.sh.borrow_mut();
                    let Some(b) = s.audio_dec.pop_front() else { break };
                    s.audio_dec_us -= buffer_us(&b);
                    drop(s);
                    out.push(b);
                }
                out.drain(host.audio());
            }
            host.audio().set_volume(if self.muted { 0.0 } else { self.volume });
        }

        // 4. Start playback once enough is buffered.
        if self.want_play && !self.running && !self.ended && opened && !seeking {
            let audio_done = self.sh.borrow().audio_done;
            let ready = match &self.audio {
                Some(out) => {
                    let frames = out.pending_frames() + host.audio().queued_frames();
                    let queued_us = frames as i64 * 1_000_000 / WANT_AUDIO.sample_rate as i64;
                    queued_us >= START_BUFFER_US
                        || (audio_done && out.origin().is_some())
                        || (audio_done && frames == 0)
                }
                None => true,
            };
            if ready {
                let start = match &self.audio {
                    Some(out) => out.heard_pts(&*host.audio()).or(self.seek_target),
                    None => None,
                };
                if let Some(t) = start.or(Some(self.clock.now_stream(now))) {
                    self.clock.seek(t, now);
                }
                self.clock.resume(now);
                if self.audio.is_some() {
                    host.audio().set_paused(false);
                }
                self.running = true;
            }
        }

        // 5. Keep the clock locked to what is being heard.
        if self.running {
            if let Some(out) = &self.audio {
                if let Some(h) = out.heard_pts(&*host.audio()) {
                    self.clock.update_from_audio(h, now);
                }
            }
        }

        // 6. End of stream.
        if self.running {
            let s = self.sh.borrow();
            let audio_drained = match &self.audio {
                Some(out) => {
                    s.audio_done
                        && s.audio_dec.is_empty()
                        && out.pending_frames() == 0
                        && host.audio().queued_frames() == 0
                }
                None => s.demux_done,
            };
            drop(s);
            if audio_drained {
                self.running = false;
                self.ended = true;
                self.clock.pause(now);
                self.want_play = false;
            }
        }

        host.clock().request_wake(now + TICK_US);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_without_a_file_reports_failure() {
        use alloc::string::String;
        use rvp_host::mock::MemSource;
        struct NoCodecs;
        impl CodecFactory for NoCodecs {
            fn audio(
                &self,
                _: &StreamInfo,
            ) -> rvp_core::Result<alloc::boxed::Box<dyn rvp_core::AudioDecoder>> {
                Err(Error::Unsupported(String::from("none")))
            }
            fn video(
                &self,
                _: &StreamInfo,
            ) -> rvp_core::Result<alloc::boxed::Box<dyn rvp_core::VideoDecoder>> {
                Err(Error::Unsupported(String::from("none")))
            }
        }
        let mut s = Session::new(MemSource::new(alloc::vec![0u8; 64]), Rc::new(NoCodecs));
        assert_eq!(s.state(), SessionState::Opening);
        // No host needed to poll tasks: the open fails on garbage.
        for _ in 0..4 {
            s.exec.poll_all();
        }
        assert_eq!(s.state(), SessionState::Failed);
        assert!(matches!(s.error(), Some(Error::Unsupported(_))));
    }
}
