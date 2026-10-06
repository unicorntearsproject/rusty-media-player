//! `WebPlayer`: the object the page talks to. It owns the host and the app, turns DOM events into
//! [`InputEvent`]s, and exposes `tick` and a JSON `snapshot` for tests.
use crate::audio::{JsAudio, WebAudio};
use crate::host::{WebClock, WebHost, WebInput, WebStorage, WebSurface};
use rvp_app::{App, Effect};
use rvp_core::{AudioDecoder, CodecFactory, Error, Result as CoreResult, StreamInfo, VideoDecoder};
use rvp_host::{FrameSink, InputEvent, Key, Modifiers, PointerButton};
use rvp_ui::{Cursor, UiConfig};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// The decoders linked into the browser build.
struct WebCodecs;

impl CodecFactory for WebCodecs {
    fn audio(&self, info: &StreamInfo) -> CoreResult<Box<dyn AudioDecoder>> {
        rvp_codec_audio::audio_decoder(info)
    }

    fn video_light(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        // Posters: on the calling thread with the least memory, never on the worker threads the film being watched needs.
        match info.codec.as_str() {
            "av1" => rvp_codec_av1::av1_decoder_light(info),
            "h264" => rvp_codec_h264::h264_decoder_with(info, None, None),
            _ => build_video(info),
        }
    }

    fn video(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        // With shared memory the decoder runs on a worker thread of its own, so decoding never competes with the UI
        // thread; without it the decoder runs inside the tick as before.
        if rvp_par::available() && matches!(info.codec.as_str(), "av1" | "h264" | "vp9") {
            let info = info.clone();
            return Ok(Box::new(rvp_par::ThreadedVideoDecoder::new(Box::new(move || build_video(&info)))));
        }
        build_video(info)
    }
}

/// Test hook (`window.rvp.debugCrash("decoder")`): the next packet any video decoder is given makes it panic, so the
/// page's recovery from a crashed decoder (a panic aborts a WebAssembly module) can be tested.
static CRASH_NEXT_PACKET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Wraps a decoder to honour [`CRASH_NEXT_PACKET`].
struct CrashProbe(Box<dyn VideoDecoder>);

impl VideoDecoder for CrashProbe {
    fn send_packet(&mut self, packet: &rvp_core::Packet) -> CoreResult<()> {
        if CRASH_NEXT_PACKET.swap(false, std::sync::atomic::Ordering::AcqRel) {
            panic!("debug: decoder crash requested");
        }
        self.0.send_packet(packet)
    }
    fn receive_frame(&mut self) -> CoreResult<Option<rvp_core::VideoFrame>> {
        self.0.receive_frame()
    }
    fn flush(&mut self) {
        self.0.flush()
    }
    fn drain(&mut self) -> CoreResult<()> {
        self.0.drain()
    }
    fn pending(&self) -> usize {
        self.0.pending()
    }
}

fn build_video(info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
    build_video_inner(info).map(|d| Box::new(CrashProbe(d)) as Box<dyn VideoDecoder>)
}

fn build_video_inner(info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
    match info.codec.as_str() {
        "av1" => rvp_codec_av1::av1_decoder(info),
        // With threads the reconstruction runs on a thread of its own, overlapping the parsing of the next picture.
        "h264" if rvp_par::available() => rvp_par::h264::h264_pipelined(info),
        "h264" => rvp_codec_h264::h264_decoder(info),
        "vp9" => rvp_codec_vp9::vp9_decoder(info),
        other => Err(Error::Unsupported(format!("video codec `{other}`"))),
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
            root_files: HashMap::new(),
            library: crate::host::WebLibrary::default(),
            downloads: Vec::new(),
            media: None,
            tap: None,
            net: crate::net::WebNet,
            writer: crate::writer::WebWriter,
        };
        let app = App::new(Rc::new(WebCodecs), UiConfig { reduce_motion });
        Ok(WebPlayer { host, app, next_file: 0 })
    }

    /// Open a `File` (picker or drop) and start playing it.
    pub fn open_file(&mut self, file: web_sys::File) {
        self.open_files(vec![file.into()], false);
    }

    /// Open several files (picker or drop): they become the playlist and the first one plays, or with `append`
    /// they are added to the end. Subtitle files (.srt, .vtt, .ass, .ssa) attach to the video that is playing.
    pub fn open_files(&mut self, files: Vec<JsValue>, append: bool) {
        self.host.input.0.push_back(InputEvent::DragOver(false));
        let items: Vec<(String, String)> = files
            .into_iter()
            .filter_map(|f| f.dyn_into::<web_sys::File>().ok())
            .map(|f| {
                let name = f.name();
                (self.stash_file(f), name)
            })
            .collect();
        self.app.open_items(&mut self.host, &items, append);
        self.app.pump(&mut self.host);
    }

    /// Give the player the page's store for the library's data (IndexedDB).
    pub fn set_store(&mut self, js: crate::host::JsStore) {
        self.host.storage.set_js(js);
    }

    /// A value the page loaded from its store before the player started.
    pub fn store_preload(&mut self, key: &str, data: &[u8]) {
        self.host.storage.preload(key, data);
    }

    /// A library folder was walked: `paths` are the files' paths below the folder (`/` separated) and `files` the `File`
    /// objects, in the same order. The app scans what changed since it last saw the folder.
    pub fn library_listing(&mut self, root: &str, name: &str, paths: Vec<String>, files: Vec<JsValue>) {
        let mut ids = Vec::with_capacity(files.len());
        let mut entries = Vec::with_capacity(files.len());
        for (path, f) in paths.into_iter().zip(files) {
            let Ok(file) = f.dyn_into::<web_sys::File>() else { continue };
            let blob: &web_sys::Blob = file.as_ref();
            let (size, mtime) = (blob.size() as u64, file.last_modified() as i64);
            let id = self.stash_file(file);
            ids.push(id.clone());
            entries.push(rvp_host::FileEntry { id, path, size, mtime_ms: mtime });
        }
        // Keep the two newest listings of a folder; older files are no longer reachable by id (running playback holds its own handle).
        let gens = self.host.root_files.entry(root.to_string()).or_default();
        gens.push(ids);
        while gens.len() > 2 {
            for id in gens.remove(0) {
                self.host.files.remove(&id);
            }
        }
        self.host.library.listings.push_back(rvp_host::Listing {
            root: root.into(),
            name: name.into(),
            files: entries,
        });
    }

    /// The folders the page can read right now.
    pub fn library_connected(&mut self, roots: Vec<String>) {
        self.host.library.connected = roots;
    }

    /// Files for the user, as `[name, mime, Uint8Array]` triples (after a `"download"` effect).
    pub fn take_downloads(&mut self) -> js_sys::Array {
        let out = js_sys::Array::new();
        for (name, mime, data) in std::mem::take(&mut self.host.downloads) {
            let triple = js_sys::Array::new();
            triple.push(&JsValue::from_str(&name));
            triple.push(&JsValue::from_str(&mime));
            triple.push(&js_sys::Uint8Array::from(data.as_slice()));
            out.push(&triple);
        }
        out
    }

    /// Give the player the page's Media Session adapter (`web/mediasession.js`), so lock screens, media keys and
    /// the browser's media controls show what is playing and can control it.
    pub fn set_media_session(&mut self, js: crate::media::JsMediaSession) {
        self.host.media = Some(crate::media::WebNowPlaying::new(js));
    }

    /// Switch the visualizer tap (audio analysis) on or off.
    pub fn enable_visualizer(&mut self, on: bool) {
        self.host.tap = on.then(crate::host::WebTap::default);
    }

    /// JSON with what the visualizer tap has received: counts, and the latest summary (level, bass/mid/treble, the
    /// 32 bands, tempo). `null` while the tap is off.
    pub fn viz_state(&self) -> String {
        let Some(t) = &self.host.tap else { return "null".into() };
        let last = match &t.last {
            Some(s) => format!(
                "{{\"pts_us\":{},\"level\":{:.4},\"peak\":{:.4},\"bass\":{:.4},\"mid\":{:.4},\"treble\":{:.4},\"onset\":{},\"tempo_bpm\":{:.2},\"bands\":[{}]}}",
                s.pts_us,
                s.level,
                s.peak,
                s.bass,
                s.mid,
                s.treble,
                s.onset,
                s.tempo_bpm,
                s.bands.iter().map(|b| format!("{b:.3}")).collect::<Vec<_>>().join(",")
            ),
            None => "null".into(),
        };
        format!(
            "{{\"frames\":{},\"summaries\":{},\"onsets\":{},\"last\":{}}}",
            t.frames, t.summaries, t.onsets, last
        )
    }

    /// Test hook: panic right now (a WebAssembly panic aborts the module, as a bug in a decoder would).
    pub fn debug_panic(&self) {
        panic!("debug: panic requested");
    }

    /// Test hook: make the next video packet crash its decoder (on the decoder's thread when there is one).
    pub fn debug_crash_decoder(&self) {
        CRASH_NEXT_PACKET.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Write what must survive a reload (the resume position). Call before the page unloads.
    pub fn save_state(&mut self) {
        self.app.save_state(&mut self.host);
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

    /// Text the user pasted into the page.
    pub fn paste(&mut self, text: String) {
        self.dispatch(InputEvent::Paste(text));
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
                Effect::AddFiles => v.push("add".into()),
                Effect::AddFolder => v.push("folder".into()),
                Effect::Rescan(id) => v.push(format!("rescan:{id}")),
                Effect::Forget(id) => v.push(format!("forget:{id}")),
                Effect::ImportPlaylist => v.push("import".into()),
                Effect::OpenUrl(url) => v.push(format!("open:{url}")),
                Effect::PickCover => v.push("cover".into()),
                Effect::Download { name, mime, data } => {
                    self.host.downloads.push((name, mime, data));
                    v.push("download".into());
                }
            }
        }
        if let Some(on) = self.host.surface.fullscreen_request.take() {
            v.push(format!("fullscreen:{on}"));
        }
        v
    }

    /// A picture was chosen for the tag editor's cover (its name and bytes).
    pub fn cover_picked(&mut self, name: &str, bytes: Vec<u8>) {
        let now = rvp_host::HostClock::now_us(&self.host.clock);
        self.app.cover_picked(name, bytes, now);
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
