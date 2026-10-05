//! The browser [`Host`]: wires the clock, audio, video sink, canvas surface, input queue and storage together.
use crate::audio::WebAudio;
use crate::source::WebSource;
use rvp_core::Timestamp;
use rvp_host::{
    FrameSink, Host, HostClock, HostError, InputEvent, InputEvents, OpenRequest, Rect, Storage, Surface,
};
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use wasm_bindgen::JsCast;

/// `performance.now()` in microseconds.
pub struct WebClock {
    perf: web_sys::Performance,
    wake: Cell<Timestamp>,
}

impl WebClock {
    pub fn new() -> Result<Self, String> {
        let perf = web_sys::window().and_then(|w| w.performance()).ok_or("no performance object")?;
        Ok(Self { perf, wake: Cell::new(0) })
    }
}

impl HostClock for WebClock {
    fn now_us(&self) -> Timestamp {
        (self.perf.now() * 1000.0) as Timestamp
    }

    fn request_wake(&self, at_us: Timestamp) {
        // The page ticks every animation frame (and on a timer when hidden); nothing to schedule.
        self.wake.set(at_us);
    }
}

/// The `<canvas>` the whole UI is drawn on.
pub struct WebSurface {
    pub canvas: web_sys::HtmlCanvasElement,
    ctx: web_sys::CanvasRenderingContext2d,
    /// One `ImageData` per size, kept so a frame costs a copy and an upload instead of a 4 MB allocation.
    image: Option<(js_sys::Uint8ClampedArray, web_sys::ImageData, u32, u32)>,
    pub size: (u32, u32, f32),
    pub fullscreen_request: Option<bool>,
}

impl WebSurface {
    pub fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, String> {
        let ctx = canvas
            .get_context("2d")
            .map_err(|e| format!("{e:?}"))?
            .ok_or("no 2d context")?
            .dyn_into::<web_sys::CanvasRenderingContext2d>()
            .map_err(|_| "not a 2d context")?;
        let size = (canvas.width().max(1), canvas.height().max(1), 1.0);
        Ok(Self { canvas, ctx, image: None, size, fullscreen_request: None })
    }
}

impl Surface for WebSurface {
    fn size(&self) -> (u32, u32, f32) {
        self.size
    }

    fn present_rgba(&mut self, rgba: &[u8], _dirty: Rect) {
        let (w, h, _) = self.size;
        if rgba.len() != w as usize * h as usize * 4 {
            return;
        }
        if !self.image.as_ref().is_some_and(|(_, _, iw, ih)| (*iw, *ih) == (w, h)) {
            let buf = js_sys::Uint8ClampedArray::new_with_length(w * h * 4);
            match web_sys::ImageData::new_with_js_u8_clamped_array_and_sh(&buf, w, h) {
                Ok(img) => self.image = Some((buf, img, w, h)),
                Err(_) => return,
            }
        }
        if let Some((buf, img, _, _)) = &self.image {
            buf.copy_from(rgba);
            let _ = self.ctx.put_image_data(img, 0.0, 0.0);
        }
    }

    fn set_fullscreen(&mut self, on: bool) {
        self.fullscreen_request = Some(on);
    }
}

/// Input events pushed by the page's DOM listeners.
#[derive(Default)]
pub struct WebInput(pub VecDeque<InputEvent>);

impl InputEvents for WebInput {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

/// Settings in `localStorage` (hex encoded).
pub struct WebStorage(Option<web_sys::Storage>);

impl WebStorage {
    pub fn new() -> Self {
        Self(web_sys::window().and_then(|w| w.local_storage().ok().flatten()))
    }
}

impl Storage for WebStorage {
    async fn load(&mut self, key: &str) -> Option<Vec<u8>> {
        let s = self.0.as_ref()?.get_item(&format!("rvp:{key}")).ok().flatten()?;
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
    }

    async fn store(&mut self, key: &str, value: &[u8]) {
        if let Some(s) = &self.0 {
            let hex: String = value.iter().map(|b| format!("{b:02x}")).collect();
            let _ = s.set_item(&format!("rvp:{key}"), &hex);
        }
    }
}

/// Everything the app needs from the page.
pub struct WebHost {
    pub clock: WebClock,
    pub audio: WebAudio,
    pub video: FrameSink,
    pub surface: WebSurface,
    pub input: WebInput,
    pub storage: WebStorage,
    pub files: HashMap<String, web_sys::File>,
}

impl Host for WebHost {
    type Source = WebSource;
    type Audio = WebAudio;
    type Video = FrameSink;
    type Store = WebStorage;

    fn clock(&self) -> &dyn HostClock {
        &self.clock
    }
    fn audio(&mut self) -> &mut WebAudio {
        &mut self.audio
    }
    fn video(&mut self) -> &mut FrameSink {
        &mut self.video
    }
    fn surface(&mut self) -> &mut dyn Surface {
        &mut self.surface
    }
    fn input(&mut self) -> &mut dyn InputEvents {
        &mut self.input
    }
    fn storage(&mut self) -> &mut WebStorage {
        &mut self.storage
    }
    async fn open(&mut self, req: OpenRequest) -> Result<WebSource, HostError> {
        match req {
            OpenRequest::Id(id) => self
                .files
                .get(&id)
                .cloned()
                .map(WebSource::new)
                .ok_or_else(|| HostError(format!("no dropped file `{id}`"))),
            OpenRequest::Pick => Err(HostError("the page shows the file picker itself".into())),
        }
    }
}
