//! Run a [`Session`] to completion in virtual time: the engine behind `rvp-headless play`.
use crate::{FileSource, HeadlessHost};
use rvp_core::{AudioDecoder, CodecFactory, Error, Result, StreamInfo, Timestamp, VideoDecoder};
use rvp_player::{Session, SessionState, TraceEntry};
use std::path::Path;
use std::rc::Rc;

/// Codecs linked into this build.
pub struct DefaultCodecs;

impl CodecFactory for DefaultCodecs {
    fn audio(&self, info: &StreamInfo) -> Result<Box<dyn AudioDecoder>> {
        rvp_codec_audio::audio_decoder(info)
    }

    fn video(&self, info: &StreamInfo) -> Result<Box<dyn VideoDecoder>> {
        Err(Error::Unsupported(format!("video codec `{}`", info.codec)))
    }
}

/// What to do during playback.
#[derive(Debug, Clone, Default)]
pub struct PlayOptions {
    /// `(when position reaches, seek to)` pairs, in stream microseconds, applied in order.
    pub seeks: Vec<(Timestamp, Timestamp)>,
    /// Stop after this much virtual time (default: generous bound from the duration).
    pub max_virtual_us: Option<Timestamp>,
}

/// Result of a run.
#[derive(Debug)]
pub struct PlayReport {
    /// Everything the audio sink accepted, interleaved stereo `f32` at the sink rate.
    pub audio: Vec<f32>,
    /// Audio chunks as pushed to the sink pipeline.
    pub audio_trace: Vec<TraceEntry>,
    /// Virtual time that elapsed.
    pub virtual_us: Timestamp,
    /// Final state.
    pub state: SessionState,
    /// Container duration.
    pub duration_us: Option<Timestamp>,
    /// First error, if any.
    pub error: Option<Error>,
}

/// Play `path` headlessly to the end in virtual time.
pub fn play_file(path: &str, opts: &PlayOptions) -> Result<PlayReport> {
    let mut host = HeadlessHost::new();
    host.audio.capture = Some(Vec::new());
    let clock = host.virtual_clock();
    let mut session = Session::new(FileSource::open(path)?, Rc::new(DefaultCodecs));
    session.enable_audio_trace();
    session.play();
    let mut seeks = opts.seeks.iter().copied().peekable();
    let mut max_us = opts.max_virtual_us;
    loop {
        session.tick(&mut host);
        let now = rvp_host::HostClock::now_us(&*clock);
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
        clock.advance(10_000);
    }
    let now = rvp_host::HostClock::now_us(&*clock);
    Ok(PlayReport {
        audio: host.audio.capture.take().unwrap_or_default(),
        audio_trace: session.audio_trace().to_vec(),
        virtual_us: now,
        state: session.state(),
        duration_us: session.duration_us(),
        error: session.error(),
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
