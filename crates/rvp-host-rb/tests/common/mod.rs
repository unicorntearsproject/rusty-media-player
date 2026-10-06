//! Shared set-up for the adapter tests: a mock Rusty Bucket host on virtual time and a player running against it.
#![allow(dead_code)]
use bucket_v0_mock::{FileSpec, MockHost, State, events};
use bucket_v0_sys::{self as sys, backend::Guard};
use rvp_core::{AudioDecoder, CodecFactory, Error, Result as CoreResult, StreamInfo, VideoDecoder};
use rvp_host_rb::RbPlayer;
use rvp_ui::UiConfig;
use std::rc::Rc;
use std::sync::Arc;

/// Audio decoders only: enough for WAV, which is raw PCM.
pub struct TestCodecs;

impl CodecFactory for TestCodecs {
    fn audio(&self, info: &StreamInfo) -> CoreResult<Box<dyn AudioDecoder>> {
        rvp_codec_audio::audio_decoder(info)
    }

    fn video(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        Err(Error::Unsupported(format!("video codec `{}`", info.codec)))
    }
}

/// A 440 Hz tone, stereo 16-bit PCM WAV at 48 kHz.
pub fn tone_wav(secs: f32) -> Vec<u8> {
    let rate = 48_000u32;
    let frames = (secs * rate as f32) as usize;
    let mut pcm = Vec::with_capacity(frames * 4);
    for i in 0..frames {
        let v = ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 12_000.0) as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&2u16.to_le_bytes()); // channels
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(&pcm);
    w
}

pub struct Harness {
    pub mock: Arc<MockHost>,
    pub player: RbPlayer,
    _guard: Guard,
}

impl Harness {
    /// A player on a fresh mock host (default capabilities: canvas, audio, now-playing, library, video layer).
    pub fn new() -> Self {
        Self::with(|_| {})
    }

    /// As [`Harness::new`], after `setup` changed the mock (capabilities, limits, canvas) and before the player started.
    pub fn with(setup: impl FnOnce(&mut State)) -> Self {
        let mock = MockHost::new();
        let guard = mock.install();
        mock.with(|s| {
            s.canvas.width = 320;
            s.canvas.height = 180;
            setup(s)
        });
        let player = RbPlayer::new(Rc::new(TestCodecs), UiConfig { reduce_motion: true });
        Self { mock, player, _guard: guard }
    }

    pub fn now(&self) -> i64 {
        self.mock.with(|s| s.now_us)
    }

    /// Run the loop until `ms` of virtual time have passed.
    pub fn run_ms(&mut self, ms: i64) {
        let end = self.now() + ms * 1000;
        let mut guard = 0;
        while self.now() < end {
            assert!(self.player.run_once(), "the app ended");
            guard += 1;
            assert!(guard < 200_000, "the loop is not advancing virtual time");
        }
    }

    /// Run the loop until `cond` holds (checked after every turn) or `ms` of virtual time pass; returns whether it held.
    pub fn run_until(&mut self, ms: i64, mut cond: impl FnMut(&mut Harness) -> bool) -> bool {
        let end = self.now() + ms * 1000;
        while self.now() < end {
            if cond(self) {
                return true;
            }
            if !self.player.run_once() {
                return false;
            }
        }
        cond(self)
    }

    pub fn push(&self, e: sys::Event) {
        self.mock.with(|s| s.push(e));
    }

    /// Open `data` as `name` the way the shell does when the app is started with a file: an `OPEN` event with a handle.
    pub fn open_file(&mut self, spec: FileSpec) {
        let h = self.mock.with(|s| {
            s.add_file(spec.clone());
            s.new_handle(spec)
        });
        self.push(events::open(h, false, false));
    }

    /// Start playing a tone of `secs` seconds.
    pub fn play_tone(&mut self, secs: f32) {
        self.open_file(FileSpec::new("tone.wav", tone_wav(secs)));
        assert!(
            self.run_until(3000, |h| h.mock.with(|s| !s.audio.is_empty())),
            "the audio stream was never opened"
        );
    }
}
