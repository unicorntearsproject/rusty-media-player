//! The audio sink: a thin Rust side over the JavaScript `RvpAudio` object (`web/audio.js`), which owns the
//! `AudioContext`, an AudioWorklet reading a ring buffer (or a ScriptProcessor on browsers without
//! worklets) and reports how many frames have actually been played.
use rvp_core::{AudioParams, Timestamp};
use rvp_host::{AudioSink, HostError};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// The page's audio engine.
    pub type JsAudio;

    /// Sample rate of the output device, or 0 if no audio context exists yet.
    #[wasm_bindgen(method, js_name = sampleRate)]
    fn sample_rate(this: &JsAudio) -> f64;
    /// Frames the device has played since the last `flush` (estimated up to this instant).
    #[wasm_bindgen(method)]
    fn played(this: &JsAudio) -> f64;
    /// Output latency in seconds.
    #[wasm_bindgen(method)]
    fn latency(this: &JsAudio) -> f64;
    /// Queue interleaved stereo samples.
    #[wasm_bindgen(method)]
    fn write(this: &JsAudio, samples: &[f32]);
    /// Drop queued audio and restart the played counter.
    #[wasm_bindgen(method)]
    fn flush(this: &JsAudio);
    /// Pause or resume the output.
    #[wasm_bindgen(method, js_name = setPaused)]
    fn set_paused(this: &JsAudio, paused: bool);
    /// Output gain, 0.0..=1.0.
    #[wasm_bindgen(method, js_name = setVolume)]
    fn set_volume(this: &JsAudio, volume: f32);
    /// Why the output is not running: "interrupted", "closed" or "" (all well; "suspended" before the first click is not a failure).
    #[wasm_bindgen(method)]
    fn issue(this: &JsAudio) -> String;
    /// Ask for an interrupted output back, with a growing pause between tries.
    #[wasm_bindgen(method)]
    fn maintain(this: &JsAudio);
}

/// Frames the sink accepts ahead of playback: a second. It must ride over a slow frame of the page (the player tops the ring up
/// once per turn, and the page's own drawing can take a good fraction of a second on a slow machine); it must not be much longer, because
/// what is queued here has had the level gain and the crossfade applied already, so a change of either is heard only after the ring plays out.
/// (Volume and mute are the output's own gain node, and pause, seek and skip hold or flush the ring: they act at once whatever is queued.)
fn capacity(rate: u32) -> u64 {
    rate as u64
}

/// Audio output through the page.
pub struct WebAudio {
    js: JsAudio,
    params: Option<AudioParams>,
    written: u64,
    /// The output was taken away and has not come back yet.
    away: bool,
    came_back: bool,
}

impl WebAudio {
    /// Wrap the page's audio engine.
    pub fn new(js: JsAudio) -> Self {
        Self { js, params: None, written: 0, away: false, came_back: false }
    }

    fn queued(&self) -> u64 {
        let played = self.js.played().max(0.0) as u64;
        self.written.saturating_sub(played)
    }
}

impl AudioSink for WebAudio {
    fn open(&mut self, want: AudioParams) -> Result<AudioParams, HostError> {
        let rate = self.js.sample_rate();
        let rate = if rate >= 8000.0 { rate as u32 } else { want.sample_rate };
        let p = AudioParams { sample_rate: rate, channels: 2 };
        self.params = Some(p);
        self.written = 0;
        self.js.flush();
        Ok(p)
    }

    fn queued_frames(&self) -> usize {
        self.queued() as usize
    }

    fn output_latency_us(&self) -> Timestamp {
        (self.js.latency() * 1_000_000.0) as Timestamp
    }

    fn write(&mut self, interleaved: &[f32]) -> usize {
        let Some(p) = self.params else { return 0 };
        let room = capacity(p.sample_rate).saturating_sub(self.queued()) as usize;
        let frames = (interleaved.len() / 2).min(room);
        if frames > 0 {
            self.js.write(&interleaved[..frames * 2]);
            self.written += frames as u64;
        }
        frames
    }

    fn flush(&mut self) {
        self.js.flush();
        self.written = 0;
    }

    fn set_paused(&mut self, paused: bool) {
        self.js.set_paused(paused);
    }

    fn set_volume(&mut self, volume: f32) {
        self.js.set_volume(volume);
    }

    fn issue(&self) -> Option<rvp_host::AudioIssue> {
        let state = self.js.issue();
        (!state.is_empty()).then(|| rvp_host::AudioIssue {
            kind: rvp_host::AudioIssueKind::Gone,
            device: String::new(),
            reason: format!("the browser {state} the audio output"),
        })
    }

    fn maintain(&mut self) {
        self.js.maintain();
        let now_away = !self.js.issue().is_empty();
        if self.away && !now_away {
            self.came_back = true;
        }
        self.away = now_away;
    }

    fn take_recovered(&mut self) -> Option<String> {
        std::mem::take(&mut self.came_back).then(String::new)
    }
}
