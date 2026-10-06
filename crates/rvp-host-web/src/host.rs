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

#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    /// The page's store for big values (IndexedDB, `web/library.js`): the library index and its thumbnails.
    pub type JsStore;

    /// Keep `data` under `key`; an empty `data` deletes it.
    #[wasm_bindgen::prelude::wasm_bindgen(method)]
    fn put(this: &JsStore, key: &str, data: &[u8]);
}

/// Settings in `localStorage` (hex encoded); the library's data (keys under `library/`) is too big for it and goes to the
/// page's IndexedDB store: reads come from a copy the page loaded before the player started, writes go to both.
pub struct WebStorage {
    local: Option<web_sys::Storage>,
    big: HashMap<String, Vec<u8>>,
    js: Option<JsStore>,
}

impl WebStorage {
    pub fn new() -> Self {
        Self {
            local: web_sys::window().and_then(|w| w.local_storage().ok().flatten()),
            big: HashMap::new(),
            js: None,
        }
    }

    /// Hand over the page's IndexedDB bridge.
    pub fn set_js(&mut self, js: JsStore) {
        self.js = Some(js);
    }

    /// A value the page loaded from IndexedDB at start-up (not written back).
    pub fn preload(&mut self, key: &str, data: &[u8]) {
        self.big.insert(key.to_string(), data.to_vec());
    }
}

fn is_big(key: &str) -> bool {
    key.starts_with("library/")
}

impl Storage for WebStorage {
    async fn load(&mut self, key: &str) -> Option<Vec<u8>> {
        if is_big(key) {
            return self.big.get(key).cloned();
        }
        let s = self.local.as_ref()?.get_item(&format!("rvp:{key}")).ok().flatten()?;
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
    }

    async fn store(&mut self, key: &str, value: &[u8]) {
        if is_big(key) {
            if value.is_empty() {
                self.big.remove(key);
            } else {
                self.big.insert(key.to_string(), value.to_vec());
            }
            if let Some(js) = &self.js {
                js.put(key, value);
            }
            return;
        }
        if let Some(s) = &self.local {
            let hex: String = value.iter().map(|b| format!("{b:02x}")).collect();
            let _ = s.set_item(&format!("rvp:{key}"), &hex);
        }
    }
}

/// Folder listings the page walked (File System Access directory handles, or a `webkitdirectory` input), waiting for the app.
#[derive(Default)]
pub struct WebLibrary {
    pub listings: VecDeque<rvp_host::Listing>,
    pub connected: Vec<String>,
}

impl rvp_host::Library for WebLibrary {
    fn take_listing(&mut self) -> Option<rvp_host::Listing> {
        self.listings.pop_front()
    }

    fn connected_roots(&self) -> Vec<String> {
        self.connected.clone()
    }
}

/// The visualizer tap of the page: counts what it gets and keeps the latest summary, which scripts read through
/// `WebPlayer::viz_state` (tests, and later the page's own visualizer).
#[derive(Default)]
pub struct WebTap {
    pub frames: u64,
    pub summaries: u64,
    pub onsets: u64,
    pub last: Option<rvp_host::VizSummary>,
}

impl rvp_host::VisualizerTap for WebTap {
    fn push_block(&mut self, block: &rvp_host::VizBlock<'_>) {
        self.frames += (block.samples.len() / block.channels.max(1) as usize) as u64;
    }

    fn push_summary(&mut self, s: &rvp_host::VizSummary) {
        self.summaries += 1;
        self.onsets += s.onset as u64;
        self.last = Some(*s);
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
    /// The ids of the files of each library folder, one list per listing (the two newest are kept: the queue may still point at
    /// the previous one while the new one is being scanned).
    pub root_files: HashMap<String, Vec<Vec<String>>>,
    /// Directory access for the library.
    pub library: WebLibrary,
    /// Files the app wants the page to hand to the user (exported playlists): name, media type, bytes.
    pub downloads: Vec<(String, String, Vec<u8>)>,
    /// The Media Session adapter, once the page has set one.
    pub media: Option<crate::media::WebNowPlaying>,
    /// The visualizer tap, while the page has it switched on.
    pub tap: Option<WebTap>,
    /// Fetching a link (the theme dialog), through the page.
    pub net: crate::net::WebNet,
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
    fn visualizer(&mut self) -> Option<&mut dyn rvp_host::VisualizerTap> {
        self.tap.as_mut().map(|t| t as &mut dyn rvp_host::VisualizerTap)
    }
    fn library(&mut self) -> Option<&mut dyn rvp_host::Library> {
        Some(&mut self.library)
    }
    fn net(&mut self) -> Option<&mut dyn rvp_host::Net> {
        Some(&mut self.net)
    }
    fn now_playing(&mut self) -> Option<&mut dyn rvp_host::NowPlaying> {
        self.media.as_mut().map(|m| m as &mut dyn rvp_host::NowPlaying)
    }
}
