//! `WebPlayer`: the object the page talks to. It owns the host and the app, turns DOM events into
//! [`InputEvent`]s, and exposes `tick` and a JSON `snapshot` for tests.
use crate::audio::{JsAudio, WebAudio};
use crate::host::{WebClock, WebHost, WebInput, WebStorage, WebSurface};
use crate::source::WebSource;
use rvp_app::{App, Effect};
use rvp_core::{AudioDecoder, CodecFactory, Error, Result as CoreResult, StreamInfo, VideoDecoder};
use rvp_host::{FrameSink, InputEvent, Key, Modifiers, PointerButton};
use rvp_ui::{Cursor, UiConfig};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

/// The decoders linked into the browser build.
struct WebCodecs;

impl CodecFactory for WebCodecs {
    fn audio(&self, info: &StreamInfo) -> CoreResult<Box<dyn AudioDecoder>> {
        rvp_codec_audio::audio_decoder(info)
    }

    fn video(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        match info.codec.as_str() {
            "av1" => rvp_codec_av1::av1_decoder(info),
            "h264" => rvp_codec_h264::h264_decoder(info),
            other => Err(Error::Unsupported(format!("video codec `{other}`"))),
        }
    }
}

fn key_from(name: &str) -> Key {
    match name {
        " " | "Space" | "Spacebar" => Key::Space,
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        "ArrowLeft" => Key::Left,
        "ArrowRight" => Key::Right,
        "ArrowUp" => Key::Up,
        "ArrowDown" => Key::Down,
        "Home" => Key::Home,
        "End" => Key::End,
        n if n.chars().count() == 1 => Key::Char(n.chars().next().unwrap_or(' ')),
        n => Key::Other(n.to_string()),
    }
}

fn button_from(b: u8) -> PointerButton {
    match b {
        1 => PointerButton::Middle,
        2 => PointerButton::Secondary,
        3 => PointerButton::Back,
        4 => PointerButton::Forward,
        _ => PointerButton::Primary,
    }
}

/// The player in a page.
#[wasm_bindgen]
pub struct WebPlayer {
    host: WebHost,
    app: App,
    next_file: u32,
}

#[wasm_bindgen]
impl WebPlayer {
    /// Create the player on `canvas`, with `audio` as the page's audio engine.
    #[wasm_bindgen(constructor)]
    pub fn new(
        canvas: web_sys::HtmlCanvasElement,
        audio: JsAudio,
        reduce_motion: bool,
    ) -> Result<WebPlayer, JsValue> {
        std::panic::set_hook(Box::new(|info| {
            web_sys::console::error_1(&JsValue::from_str(&info.to_string()))
        }));
        let host = WebHost {
            clock: WebClock::new().map_err(|e| JsValue::from_str(&e))?,
            audio: WebAudio::new(audio),
            video: FrameSink::new(),
            surface: WebSurface::new(canvas).map_err(|e| JsValue::from_str(&e))?,
            input: WebInput::default(),
            storage: WebStorage::new(),
            files: HashMap::new(),
        };
        let app = App::new(Rc::new(WebCodecs), UiConfig { reduce_motion });
        Ok(WebPlayer { host, app, next_file: 0 })
    }

    /// Open a `File` (picker or drop) and start playing it.
    pub fn open_file(&mut self, file: web_sys::File) {
        self.host.input.0.push_back(InputEvent::DragOver(false));
        self.app.open(&mut self.host, WebSource::new(file));
        self.app.pump(&mut self.host);
    }

    /// Remember a dropped file under an id (for hosts that open by id); returns the id.
    pub fn stash_file(&mut self, file: web_sys::File) -> String {
        self.next_file += 1;
        let id = format!("file-{}", self.next_file);
        self.host.files.insert(id.clone(), file);
        id
    }

    /// The canvas was resized to `w` x `h` physical pixels at device pixel ratio `dpr`.
    pub fn resize(&mut self, w: u32, h: u32, dpr: f64) {
        let (w, h) = (w.max(1), h.max(1));
        self.host.surface.canvas.set_width(w);
        self.host.surface.canvas.set_height(h);
        self.host.surface.size = (w, h, dpr as f32);
        self.dispatch(InputEvent::Resize { w, h, dpr: dpr as f32 });
    }

    /// Pointer moved (physical pixels).
    pub fn pointer_move(&mut self, x: f32, y: f32) {
        self.dispatch(InputEvent::PointerMove { x, y });
    }

    /// Pointer button pressed: 0 primary, 1 middle, 2 secondary, 3 back, 4 forward.
    pub fn pointer_down(&mut self, x: f32, y: f32, button: u8) {
        self.dispatch(InputEvent::PointerDown { x, y, button: button_from(button) });
    }

    /// Pointer button released.
    pub fn pointer_up(&mut self, x: f32, y: f32, button: u8) {
        self.dispatch(InputEvent::PointerUp { x, y, button: button_from(button) });
    }

    /// Wheel (pixels; positive scrolls down).
    pub fn wheel(&mut self, dx: f32, dy: f32) {
        self.dispatch(InputEvent::Wheel { dx, dy });
    }

    /// Key pressed. Returns true if the player used it (the page should then `preventDefault`).
    pub fn key_down(
        &mut self,
        key: &str,
        shift: bool,
        ctrl: bool,
        alt: bool,
        meta: bool,
        repeat: bool,
    ) -> bool {
        let mods = Modifiers { shift, ctrl, alt, logo: meta };
        self.dispatch(InputEvent::KeyDown { key: key_from(key), mods, repeat });
        self.app.ui().last_key_used()
    }

    /// Key released.
    pub fn key_up(&mut self, key: &str, shift: bool, ctrl: bool, alt: bool, meta: bool) {
        let mods = Modifiers { shift, ctrl, alt, logo: meta };
        self.dispatch(InputEvent::KeyUp { key: key_from(key), mods });
    }

    /// A file is being dragged over the page.
    pub fn drag_over(&mut self, on: bool) {
        self.dispatch(InputEvent::DragOver(on));
    }

    /// The window gained or lost focus.
    pub fn focus(&mut self, on: bool) {
        self.dispatch(InputEvent::Focus(on));
    }

    /// The system's reduced-motion preference changed.
    pub fn set_reduce_motion(&mut self, on: bool) {
        self.app.ui_mut().config.reduce_motion = on;
    }

    /// The browser entered or left fullscreen on its own (Esc key).
    pub fn set_fullscreen_state(&mut self, on: bool) {
        self.app.set_fullscreen_state(on);
    }

    /// Show a short message over the picture.
    pub fn toast(&mut self, text: &str) {
        let now = rvp_host::HostClock::now_us(&self.host.clock);
        self.app.ui_mut().show_toast(text, now);
    }

    /// Advance playback and redraw. Call from `requestAnimationFrame` (and a timer while the tab is hidden).
    pub fn tick(&mut self) {
        self.app.tick(&mut self.host);
    }

    /// Things the page must do: `"pick"` (show the file picker) and `"fullscreen:true"` / `"fullscreen:false"`.
    /// Call right after forwarding an input event, inside the same DOM event handler.
    pub fn take_effects(&mut self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for e in self.app.take_effects() {
            match e {
                Effect::PickFile => v.push("pick".into()),
            }
        }
        if let Some(on) = self.host.surface.fullscreen_request.take() {
            v.push(format!("fullscreen:{on}"));
        }
        v
    }

    /// The CSS cursor to show: `default`, `pointer`, `grabbing` or `none`.
    pub fn cursor(&self) -> String {
        match self.app.cursor() {
            Cursor::Default => "default",
            Cursor::Pointer => "pointer",
            Cursor::Grabbing => "grabbing",
            Cursor::Hidden => "none",
        }
        .into()
    }

    /// JSON description of the player (state, position, geometry of the controls).
    pub fn snapshot(&self) -> String {
        self.app.snapshot().json().to_string()
    }
}

impl WebPlayer {
    fn dispatch(&mut self, ev: InputEvent) {
        self.host.input.0.push_back(ev);
        self.app.pump(&mut self.host);
    }
}
