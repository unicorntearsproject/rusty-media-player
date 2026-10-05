//! The now-playing sink for a page: a thin Rust side over the JavaScript `RvpMediaSession` object
//! (`web/mediasession.js`), which owns the Media Session API (`navigator.mediaSession`).
use rvp_host::{NowPlaying, NowPlayingMeta, PlayState, Playback, TransportCommand};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// The page's Media Session adapter.
    pub type JsMediaSession;

    /// Show what is playing. `art` is an encoded image (empty for none) with its MIME type.
    #[wasm_bindgen(method, js_name = setMetadata)]
    fn set_metadata(this: &JsMediaSession, title: &str, artist: &str, album: &str, art: &[u8], mime: &str);
    /// Report the transport state; `state` is 0 stopped, 1 playing, 2 paused. Times are in seconds, `duration` is
    /// negative when unknown.
    #[wasm_bindgen(method, js_name = setPlayback)]
    fn set_playback(
        this: &JsMediaSession,
        state: u8,
        position: f64,
        duration: f64,
        rate: f64,
        can_next: bool,
        can_prev: bool,
        can_seek: bool,
    );
    /// The next command from the browser (`play`, `pause`, `stop`, `next`, `prev`, `seekto:<s>`, `seekby:<s>`), or
    /// an empty string.
    #[wasm_bindgen(method, js_name = takeCommand)]
    fn take_command(this: &JsMediaSession) -> String;
}

/// Media Session now-playing sink.
pub struct WebNowPlaying {
    js: JsMediaSession,
    duration_s: f64,
}

impl WebNowPlaying {
    /// Wrap the page's adapter.
    pub fn new(js: JsMediaSession) -> Self {
        Self { js, duration_s: -1.0 }
    }
}

impl NowPlaying for WebNowPlaying {
    fn set_metadata(&mut self, meta: &NowPlayingMeta) {
        self.duration_s = meta.duration_us.map_or(-1.0, |d| d as f64 / 1e6);
        let (art, mime) = meta.art.as_ref().map_or((&[][..], ""), |a| (a.data.as_slice(), a.mime.as_str()));
        self.js.set_metadata(&meta.title, &meta.artist, &meta.album, art, mime);
    }

    fn set_playback(&mut self, p: &Playback) {
        let state = match p.state {
            PlayState::Stopped => 0,
            PlayState::Playing => 1,
            PlayState::Paused => 2,
        };
        self.js.set_playback(
            state,
            p.position_us as f64 / 1e6,
            self.duration_s,
            p.rate as f64,
            p.can_next,
            p.can_prev,
            p.can_seek,
        );
    }

    fn poll_command(&mut self) -> Option<TransportCommand> {
        let c = self.js.take_command();
        let (name, arg) = c.split_once(':').unwrap_or((c.as_str(), ""));
        let secs = || arg.parse::<f64>().ok().map(|s| (s * 1e6) as i64);
        match name {
            "play" => Some(TransportCommand::Play),
            "pause" => Some(TransportCommand::Pause),
            "stop" => Some(TransportCommand::Stop),
            "next" => Some(TransportCommand::Next),
            "prev" => Some(TransportCommand::Prev),
            "seekto" => secs().map(TransportCommand::SeekTo),
            "seekby" => secs().map(TransportCommand::SeekBy),
            _ => None,
        }
    }
}
