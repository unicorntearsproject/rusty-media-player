//! Run a [`Session`] to completion in virtual time: the engine behind `rvp-headless play`.
use crate::{FileSource, HeadlessHost, VirtualClock};
use rvp_core::{
    AudioDecoder, CodecFactory, Error, Packet, Result, StreamInfo, Timestamp, VideoDecoder, VideoFrame,
};
use rvp_player::{
    Session, SessionEvent, SessionState, SubtitleTrack, TraceEntry, VideoStats, VideoTraceEntry,
};
use std::path::Path;
use std::rc::Rc;

/// Codecs linked into this build (AAC/MP3/FLAC/Vorbis/Opus audio, H.264 and AV1 video).
#[derive(Default)]
pub struct DefaultCodecs {
    /// Test hook: make the Nth video packet take this much virtual time to decode (`(n, microseconds)`).
    pub stall: Option<(usize, Timestamp)>,
    /// The virtual clock the stall advances.
    pub clock: Option<Rc<VirtualClock>>,
    /// Test hook: every video packet takes this much virtual time to decode (a slow machine); needs `clock`.
    pub video_cost_us: Timestamp,
    /// Test hook: each of the first ten packets a new audio decoder gets takes this much virtual time (a slow start); needs `clock`. Shared, so a test can set it
    /// right before a skip.
    pub audio_start_us: Rc<std::cell::Cell<Timestamp>>,
    /// A platform decoder service for the codecs ours does not decode (HEVC), as a host such as a browser would offer.
    pub platform: Option<Rc<dyn rvp_core::PlatformVideo>>,
}

/// How many packets at the start of an audio stream are slow under [`DefaultCodecs::audio_start_us`].
const SLOW_START_PACKETS: usize = 10;

/// Wraps an audio decoder so each of its first packets takes a while.
struct SlowStartAudio {
    inner: Box<dyn AudioDecoder>,
    clock: Rc<VirtualClock>,
    us: Rc<std::cell::Cell<Timestamp>>,
    count: usize,
}

impl AudioDecoder for SlowStartAudio {
    fn send_packet(&mut self, p: &Packet) -> Result<()> {
        if self.count < SLOW_START_PACKETS {
            self.count += 1;
            self.clock.advance(self.us.get());
        }
        self.inner.send_packet(p)
    }

    fn receive_buffer(&mut self) -> Result<Option<rvp_core::AudioBuffer>> {
        self.inner.receive_buffer()
    }

    fn flush(&mut self) {
        self.inner.flush()
    }
}

/// Wraps a decoder and advances the virtual clock inside one `send_packet`, as a slow decode would.
struct StallDecoder {
    inner: Box<dyn VideoDecoder>,
    clock: Rc<VirtualClock>,
    count: usize,
    at: usize,
    us: Timestamp,
    cost: Timestamp,
}

impl VideoDecoder for StallDecoder {
    fn send_packet(&mut self, p: &Packet) -> Result<()> {
        self.count += 1;
        if self.count == self.at {
            self.clock.advance(self.us);
        }
        if self.cost > 0 {
            self.clock.advance(self.cost);
        }
        self.inner.send_packet(p)
    }

    fn receive_frame(&mut self) -> Result<Option<VideoFrame>> {
        self.inner.receive_frame()
    }

    fn flush(&mut self) {
        self.inner.flush()
    }

    fn drain(&mut self) -> Result<()> {
        self.inner.drain()
    }
}

impl CodecFactory for DefaultCodecs {
    fn audio(&self, info: &StreamInfo) -> Result<Box<dyn AudioDecoder>> {
        let dec = rvp_codec_audio::audio_decoder(info)?;
        match &self.clock {
            Some(clock) => Ok(Box::new(SlowStartAudio {
                inner: dec,
                clock: clock.clone(),
                us: self.audio_start_us.clone(),
                count: 0,
            })),
            None => Ok(dec),
        }
    }

    fn video(&self, info: &StreamInfo) -> Result<Box<dyn VideoDecoder>> {
        let ours = rvp_core::screened(
            || match info.codec.as_str() {
                "av1" => rvp_codec_av1::av1_decoder(info),
                "h264" => rvp_codec_h264::h264_decoder(info),
                "vp9" => rvp_codec_vp9::vp9_decoder(info),
                "hevc" => rvp_codec_hevc::sw::hevc_decoder(info),
                other => Err(Error::Unsupported(format!("video codec `{other}`"))),
            },
            info,
        );
        let mut dec = rvp_core::open_video(ours, self.platform.as_ref(), info)?;
        if matches!(info.codec.as_str(), "h264" | "hevc") && self.platform.is_some() {
            dec = Box::new(rvp_core::FallbackVideo::new(dec, self.platform.clone(), info.clone()));
        }
        match (self.stall, self.video_cost_us, &self.clock) {
            (stall, cost, Some(clock)) if stall.is_some() || cost > 0 => {
                let (at, us) = stall.unwrap_or((0, 0));
                Ok(Box::new(StallDecoder { inner: dec, clock: clock.clone(), count: 0, at, us, cost }))
            }
            _ => Ok(dec),
        }
    }
}

/// What to do during playback.
#[derive(Debug, Clone, Default)]
pub struct PlayOptions {
    /// `(when position reaches, seek to)` pairs, in stream microseconds, applied in order.
    pub seeks: Vec<(Timestamp, Timestamp)>,
    /// Stop after this much virtual time (default: generous bound from the duration).
    pub max_virtual_us: Option<Timestamp>,
    /// Make the Nth video packet take this long to decode (virtual microseconds): `(n, us)`.
    pub video_stall: Option<(usize, Timestamp)>,
    /// Virtual time between ticks (default 10 ms). Use an awkward value to test unaligned presentation.
    pub tick_us: Option<Timestamp>,
    /// Playback rate (default 1.0).
    pub rate: Option<f64>,
    /// A sidecar SRT/WebVTT file to load and show.
    pub subtitle_file: Option<String>,
    /// Select this container subtitle track id once the file is open.
    pub subtitle_track: Option<u32>,
    /// Select this container audio track id once the file is open.
    pub audio_track: Option<u32>,
    /// Loop between these stream times (A, B) from the start.
    pub ab_loop: Option<(Timestamp, Timestamp)>,
    /// Offer a visualizer tap and record what it gets.
    pub visualizer: bool,
    /// Files to play after the first one, joined gaplessly (each is queued when the session asks for the next).
    pub chain: Vec<String>,
    /// Crossfade and automatic level (the defaults: both off).
    pub audio_settings: rvp_core::AudioSettings,
    /// What the library would know about the first item's loudness.
    pub loudness_hint: Option<rvp_core::LoudnessTags>,
}

/// Result of a run.
#[derive(Debug)]
pub struct PlayReport {
    /// Everything the audio sink accepted, interleaved stereo `f32` at the sink rate.
    pub audio: Vec<f32>,
    /// Audio chunks as pushed to the sink pipeline.
    pub audio_trace: Vec<TraceEntry>,
    /// `(pts, hash)` of every frame the video sink received.
    pub video_frames: Vec<(Timestamp, u64)>,
    /// Video presentation counters.
    pub video_stats: VideoStats,
    /// Every presentation: clock position and frame pts.
    pub video_trace: Vec<VideoTraceEntry>,
    /// Non-fatal problems.
    pub warnings: Vec<String>,
    /// Virtual time that elapsed.
    pub virtual_us: Timestamp,
    /// Final state.
    pub state: SessionState,
    /// What the visualizer tap received (frames of PCM, and every summary), when it was asked for.
    pub viz_frames: u64,
    /// The summaries the visualizer tap received.
    pub viz: Vec<rvp_host::VizSummary>,
    /// Session events with the virtual host time at which they were raised.
    pub events: Vec<(Timestamp, SessionEvent)>,
    /// Subtitle tracks known at the end.
    pub subtitle_tracks: Vec<SubtitleTrack>,
    /// Container duration.
    pub duration_us: Option<Timestamp>,
    /// First error, if any.
    pub error: Option<Error>,
    /// `(virtual host time, gain of the automatic level in dB)` about every 100 ms of playing.
    pub gains: Vec<(Timestamp, f32)>,
    /// `(virtual host time, playback position)` about every 50 ms of playing.
    pub positions: Vec<(Timestamp, Timestamp)>,
    /// True if a crossfade was being mixed at any time.
    pub crossfaded: bool,
}

/// Play `path` headlessly to the end in virtual time.
pub fn play_file(path: &str, opts: &PlayOptions) -> Result<PlayReport> {
    let mut host = HeadlessHost::new();
    host.audio.capture = Some(Vec::new());
    if opts.visualizer {
        host.tap = Some(rvp_host::RecordingTap::default());
    }
    let clock = host.virtual_clock();
    let codecs = DefaultCodecs {
        stall: opts.video_stall,
        clock: Some(clock.clone()),
        video_cost_us: 0,
        platform: None,
        ..Default::default()
    };
    let mut session = Session::new(FileSource::open(path)?, Rc::new(codecs));
    session.enable_audio_trace();
    session.enable_video_trace();
    if let Some(r) = opts.rate {
        session.set_rate(r, 0);
    }
    if let Some(path) = &opts.subtitle_file {
        let name = Path::new(path).file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        session.add_subtitle_source(FileSource::open(path)?, &name);
    }
    session.set_loop(opts.ab_loop);
    session.set_audio_settings(opts.audio_settings);
    session.set_loudness_hint(opts.loudness_hint);
    session.play();
    let mut gains: Vec<(Timestamp, f32)> = Vec::new();
    let mut positions: Vec<(Timestamp, Timestamp)> = Vec::new();
    let mut crossfaded = false;
    let mut applied = false;
    let mut events = Vec::new();
    let mut chain = opts.chain.iter();
    let mut queued_tag = 0u32;
    let mut seeks = opts.seeks.iter().copied().peekable();
    let mut max_us = opts.max_virtual_us;
    loop {
        session.tick(&mut host);
        let now = rvp_host::HostClock::now_us(&*clock);
        if !applied && session.state() != SessionState::Opening {
            applied = true;
            if let Some(id) = opts.subtitle_track {
                session.select_subtitle(Some(id));
            }
            if let Some(id) = opts.audio_track {
                session.select_audio(id, now);
            }
        }
        while let Some(e) = session.poll_event() {
            events.push((now, e));
        }
        if session.state() == SessionState::Playing {
            if gains.last().is_none_or(|g| now - g.0 >= 100_000) {
                gains.push((now, session.level_gain_db()));
            }
            if positions.last().is_none_or(|p| now - p.0 >= 50_000) {
                positions.push((now, session.position_us(now)));
            }
        }
        crossfaded |= session.crossfading();
        if session.wants_next(now) {
            if let Some(path) = chain.next() {
                queued_tag += 1;
                session.queue_next(FileSource::open(path)?, queued_tag);
            }
        }
        if max_us.is_none() {
            if let Some(d) = session.duration_us() {
                max_us = Some(d * 2 + 20_000_000 + opts.seeks.iter().map(|s| s.0).sum::<i64>());
            }
        }
        match session.state() {
            SessionState::Ended | SessionState::Failed => break,
            _ => {}
        }
        if max_us.is_some_and(|m| now > m) {
            break;
        }
        if let Some(&(at, to)) = seeks.peek() {
            if session.state() == SessionState::Playing && session.position_us(now) >= at {
                session.seek(to);
                seeks.next();
            }
        }
        clock.advance(opts.tick_us.unwrap_or(10_000));
    }
    let now = rvp_host::HostClock::now_us(&*clock);
    Ok(PlayReport {
        audio: host.audio.capture.take().unwrap_or_default(),
        audio_trace: session.audio_trace().to_vec(),
        video_frames: std::mem::take(&mut host.video.frames),
        video_stats: session.video_stats().clone(),
        video_trace: session.video_trace().to_vec(),
        warnings: session.warnings(),
        virtual_us: now,
        state: session.state(),
        viz_frames: host.tap.as_ref().map_or(0, |t| t.frames),
        viz: host.tap.take().map(|t| t.summaries).unwrap_or_default(),
        events,
        subtitle_tracks: session.subtitle_tracks(),
        duration_us: session.duration_us(),
        error: session.error(),
        gains,
        positions,
        crossfaded,
    })
}

/// Write interleaved `f32` samples as a 32-bit float WAV file.
pub fn write_wav_f32(path: &Path, samples: &[f32], rate: u32, channels: u16) -> std::io::Result<()> {
    use std::io::Write;
    let data_len = (samples.len() * 4) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&3u16.to_le_bytes())?; // IEEE float
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * channels as u32 * 4).to_le_bytes())?;
    f.write_all(&(channels * 4).to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}
