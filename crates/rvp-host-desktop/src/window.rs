//! The window and the event loop: `winit` events in, input events and ticks for the app, pixels out through `softbuffer`.
use crate::cli::{Options, input_to_path};
use crate::codecs::{DesktopCodecs, start_pool};
use crate::dialogs::{Dialogs, Picked};
use crate::host::DesktopHost;
use crate::input::{WHEEL_LINE_PX, map_button, map_key};
use crate::media::{DesktopNowPlaying, Raise};
use crate::smoke::Smoke;
use rvp_app::{App, Effect};
use rvp_host::{InputEvent, Modifiers, Storage};
use rvp_ui::{Action, Cursor, MediaState, UiConfig};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorIcon, Fullscreen, Icon, Window, WindowId};

/// The application id: the desktop file, the icon, the Wayland app id, the MPRIS name and the Flatpak id.
pub const APP_ID: &str = "io.github.unicorntearsproject.RustyWave";
/// What the window's title bar says when nothing plays.
pub const APP_NAME: &str = "Rusty Wave";

const WINDOW_KEY: &str = "desktop/window";
const ROOTS_KEY: &str = "desktop/roots";
/// Shortest wait between two ticks (a flood of mouse moves must not make one tick each).
const MIN_TICK: Duration = Duration::from_millis(2);

/// Run the application until the window closes. Returns the process exit code.
pub fn run(opts: Options, data_dir: PathBuf) -> i32 {
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("rusty-wave: cannot open a window: {e}");
            return 1;
        }
    };
    let pool = start_pool();
    let mut host = DesktopHost::new(data_dir, opts.no_audio);
    let smoke = Smoke::new(&opts);
    host.surface.keep = smoke.as_ref().is_some_and(|s| s.wants_frames());
    // Update checks and the app-menu offer; a scripted run is not interrupted by their dialogs unless it asks for them.
    if smoke.is_none() || opts.app_services {
        let manifest = crate::services::manifest_url(opts.update_manifest.as_deref());
        host.services =
            Some(crate::services::DesktopServices::new(&manifest, std::env::args_os().skip(1).collect()));
    }
    let restart = host.services.as_ref().map(|s| s.restart_flag());
    let mut app =
        App::new(Rc::new(DesktopCodecs::with_system_decoders()), UiConfig { reduce_motion: reduce_motion() });
    app.ui_mut().set_font_loader(crate::fonts::system_font_loader());
    let _ = pool;
    let mut handler = Handler {
        host,
        app,
        opts,
        window: None,
        dialogs: Dialogs::new(),
        drops: Vec::new(),
        modifiers: Modifiers::default(),
        pointer: (0.0, 0.0),
        dragging_files: false,
        next_tick: Instant::now(),
        last_tick: Instant::now() - Duration::from_secs(1),
        wants_tick: true,
        cursor: Cursor::Default,
        title: String::new(),
        smoke,
        exit_code: 0,
        started_paused: false,
        _lock: None,
        quitting: false,
    };
    if let Err(e) = event_loop.run_app(&mut handler) {
        eprintln!("rusty-wave: {e}");
        return 1;
    }
    let code = handler.exit_code;
    // After "Restart now" the window has closed and its state is saved: only now does the new copy start.
    if restart.is_some_and(|r| r.get()) {
        let services = handler.host.services.take();
        drop(handler);
        if let Some(Err(e)) = services.map(|s| s.finish_restart()) {
            eprintln!("rusty-wave: could not start the new version: {e}");
        }
    }
    code
}

/// The user's reduced-motion setting, where the desktop has one (GNOME's `enable-animations`).
fn reduce_motion() -> bool {
    std::env::var_os("RVP_REDUCE_MOTION").is_some_and(|v| v == "1")
}

struct Handler {
    host: DesktopHost,
    app: App,
    opts: Options,
    window: Option<Arc<Window>>,
    dialogs: Dialogs,
    /// Files dropped since the last tick (several arrive as separate events).
    drops: Vec<PathBuf>,
    modifiers: Modifiers,
    pointer: (f32, f32),
    dragging_files: bool,
    next_tick: Instant,
    last_tick: Instant,
    wants_tick: bool,
    cursor: Cursor,
    title: String,
    smoke: Option<Smoke>,
    exit_code: i32,
    started_paused: bool,
    /// Held for the life of the process by the first instance (decides the MPRIS name).
    _lock: Option<std::fs::File>,
    quitting: bool,
}

impl Handler {
    fn create_window(&mut self, el: &ActiveEventLoop) {
        let (lw, lh) = self.opts.size.unwrap_or((1280, 720));
        let mut attrs =
            Window::default_attributes().with_title(APP_NAME).with_min_inner_size(LogicalSize::new(480, 270));
        // Where it was last time, if the user did not ask for a size.
        let saved = self.load_window_state();
        attrs = match (self.opts.size, saved) {
            (None, Some((w, h, _))) => attrs.with_inner_size(PhysicalSize::new(w, h)),
            _ => attrs.with_inner_size(LogicalSize::new(lw, lh)),
        };
        if saved.is_some_and(|s| s.2) && self.opts.size.is_none() {
            attrs = attrs.with_maximized(true);
        }
        if let Some(icon) = window_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use winit::platform::wayland::WindowAttributesExtWayland;
            use winit::platform::x11::WindowAttributesExtX11;
            attrs = WindowAttributesExtWayland::with_name(attrs, APP_ID, "rusty-wave");
            attrs = WindowAttributesExtX11::with_name(attrs, APP_ID, "rusty-wave");
        }
        if self.opts.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("rusty-wave: cannot create the window: {e}");
                self.exit_code = 1;
                el.exit();
                return;
            }
        };
        let size = window.inner_size();
        self.host.surface.size = (size.width.max(1), size.height.max(1), window.scale_factor() as f32);
        self.attach_surface(&window);
        self.host.input.0.push_back(InputEvent::Resize {
            w: self.host.surface.size.0,
            h: self.host.surface.size.1,
            dpr: self.host.surface.size.2,
        });
        if self.opts.fullscreen {
            self.app.set_fullscreen_state(true);
        }
        self.window = Some(window.clone());
        self.start_media_controls(&window);
        self.load_roots();
        let inputs = std::mem::take(&mut self.opts.inputs);
        let paths: Vec<PathBuf> = inputs.iter().map(|s| PathBuf::from(input_to_path(s))).collect();
        self.open_paths(paths, false);
        window.request_redraw();
    }

    fn attach_surface(&mut self, window: &Arc<Window>) {
        let ctx = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("rusty-wave: cannot draw to the window: {e}");
                return;
            }
        };
        let mut sb = match softbuffer::Surface::new(&ctx, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("rusty-wave: cannot draw to the window: {e}");
                return;
            }
        };
        self.host.surface.sink = Some(Box::new(move |rgba: &[u8], w: u32, h: u32| {
            let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else { return };
            if sb.resize(nw, nh).is_err() {
                return;
            }
            let Ok(mut buf) = sb.buffer_mut() else { return };
            let n = (w * h) as usize;
            for (dst, px) in buf.iter_mut().zip(rgba.as_chunks::<4>().0.iter()).take(n) {
                let v = u32::from_le_bytes(*px);
                *dst = (v & 0x0000_ff00) | ((v & 0xff) << 16) | ((v >> 16) & 0xff);
            }
            let _ = buf.present();
            // `ctx` must live as long as the surface.
            let _ = &ctx;
        }));
    }

    fn start_media_controls(&mut self, window: &Window) {
        if self.opts.no_media_keys {
            return;
        }
        #[allow(unused_mut)]
        let mut dbus_name = APP_ID.to_string();
        // The first running instance gets the plain name; others get `.instance<pid>` as MPRIS asks.
        let lock_path = self.host.storage.dir().join("instance.lock");
        let _ = std::fs::create_dir_all(self.host.storage.dir());
        if let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path) {
            if f.try_lock().is_ok() {
                self._lock = Some(f);
            } else {
                dbus_name = format!("{APP_ID}.instance{}", std::process::id());
            }
        }
        let hwnd = hwnd_of(window);
        let cache = self.host.storage.dir().join("cache");
        match DesktopNowPlaying::new(APP_NAME, &dbus_name, hwnd, cache) {
            Ok(m) => self.host.media = Some(m),
            Err(e) => eprintln!("rusty-wave: media controls are not available: {e}"),
        }
    }

    fn load_window_state(&self) -> Option<(u32, u32, bool)> {
        let b = self.host.storage.get(WINDOW_KEY)?;
        let s = String::from_utf8(b).ok()?;
        let mut it = s.split_whitespace();
        let w: u32 = it.next()?.parse().ok()?;
        let h: u32 = it.next()?.parse().ok()?;
        let max = it.next()? == "1";
        ((160..=16384).contains(&w) && (90..=16384).contains(&h)).then_some((w, h, max))
    }

    fn save_window_state(&self) {
        let Some(w) = &self.window else { return };
        if w.fullscreen().is_some() {
            return;
        }
        let size = w.inner_size();
        self.host
            .storage
            .put(WINDOW_KEY, format!("{} {} {}", size.width, size.height, w.is_maximized() as u8).as_bytes());
    }

    /// Library folders the user added before: walk them again so remembered tracks can play.
    fn load_roots(&mut self) {
        let Some(b) = self.host.storage.get(ROOTS_KEY) else { return };
        for line in String::from_utf8_lossy(&b).lines() {
            if line.is_empty() {
                continue;
            }
            let p = PathBuf::from(line);
            if p.is_dir() {
                self.host.library.add(p);
            } else {
                // Not there right now (an unplugged drive): remembered all the same, or saving the list would forget it for good.
                self.host.library.remember(p);
            }
        }
        // What was just read is what is saved: nothing to write back.
        self.host.library.take_roots_dirty();
    }

    fn save_roots(&self) {
        let list: Vec<String> =
            self.host.library.roots.values().map(|p| p.to_string_lossy().into_owned()).collect();
        self.host.storage.put(ROOTS_KEY, list.join("\n").as_bytes());
    }

    /// Open what the user chose, dropped or typed: folders join the library, files become the queue (or join it).
    fn open_paths(&mut self, paths: Vec<PathBuf>, append: bool) {
        let mut items: Vec<(String, String)> = Vec::new();
        let mut folders = false;
        for p in paths {
            if p.is_dir() {
                self.host.library.add(p);
                folders = true;
            } else {
                let name = p
                    .file_name()
                    .map_or_else(|| p.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
                items.push((p.to_string_lossy().into_owned(), name));
            }
        }
        if folders {
            self.save_roots();
        }
        if !items.is_empty() {
            self.app.open_items(&mut self.host, &items, append);
        }
        self.wants_tick = true;
    }

    fn modifiers_from(m: winit::keyboard::ModifiersState) -> Modifiers {
        Modifiers { shift: m.shift_key(), ctrl: m.control_key(), alt: m.alt_key(), logo: m.super_key() }
    }

    fn push(&mut self, ev: InputEvent) {
        self.host.input.0.push_back(ev);
        self.wants_tick = true;
    }

    fn handle_effects(&mut self) {
        // Folders the app added by itself (the first run's Music and Videos) are written down as soon as they are known: the next start reads
        // this list to bring them back.
        if self.host.library.take_roots_dirty() {
            self.save_roots();
        }
        for e in self.app.take_effects() {
            match e {
                Effect::PickFile => self.dialogs.pick_files(false),
                Effect::AddFiles => self.dialogs.pick_files(true),
                Effect::AddFolder => self.dialogs.pick_folder(),
                Effect::ImportPlaylist => self.dialogs.pick_playlists(),
                Effect::PickCover => self.dialogs.pick_cover(),
                Effect::OpenUrl(url) => crate::open_url(&url),
                Effect::Rescan(id) => self.host.library.rescan(&id),
                Effect::Forget(id) => {
                    self.host.library.forget(&id);
                    self.save_roots();
                }
                Effect::Download { name, data, .. } => self.dialogs.save(name, data),
            }
        }
    }

    fn handle_dialogs(&mut self) {
        while let Some(p) = self.dialogs.poll() {
            match p {
                Picked::Files { files, append } => self.open_paths(files, append),
                Picked::Folder(dir) => self.open_paths(vec![dir], true),
                Picked::Cover(name, bytes) => {
                    let now = rvp_host::HostClock::now_us(&self.host.clock);
                    self.app.cover_picked(&name, bytes, now);
                }
                Picked::Saved(msg) => {
                    let now = rvp_host::HostClock::now_us(&self.host.clock);
                    self.app.ui_mut().show_toast(&msg, now);
                }
            }
        }
    }

    fn handle_media_requests(&mut self, el: &ActiveEventLoop) {
        while let Some(r) = self.host.media.as_mut().and_then(|m| m.poll_raise()) {
            match r {
                Raise::Window => {
                    if let Some(w) = &self.window {
                        w.set_minimized(false);
                        w.focus_window();
                    }
                }
                Raise::Quit => self.quit(el),
            }
        }
    }

    fn quit(&mut self, el: &ActiveEventLoop) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        if self.host.library.take_roots_dirty() {
            self.save_roots();
        }
        self.app.save_state(&mut self.host);
        self.save_window_state();
        if let Some(s) = &mut self.smoke {
            s.finish(&self.app, &self.host);
        }
        el.exit();
    }

    fn after_tick(&mut self) {
        let Some(w) = &self.window else { return };
        // The cursor the UI wants.
        let c = self.app.cursor();
        if c != self.cursor {
            self.cursor = c;
            match c {
                Cursor::Hidden => w.set_cursor_visible(false),
                other => {
                    w.set_cursor_visible(true);
                    w.set_cursor(match other {
                        Cursor::Pointer => CursorIcon::Pointer,
                        Cursor::Grabbing => CursorIcon::Grabbing,
                        _ => CursorIcon::Default,
                    });
                }
            }
        }
        // Full screen.
        if let Some(on) = self.host.surface.fullscreen_request.take() {
            w.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
            self.app.set_fullscreen_state(on);
        }
        // The title bar follows what plays.
        let m = self.app.model();
        let title = if m.title.is_empty() {
            APP_NAME.to_string()
        } else if m.artist.is_empty() {
            format!("{} \u{2014} {APP_NAME}", m.title)
        } else {
            format!("{} \u{2013} {} \u{2014} {APP_NAME}", m.artist, m.title)
        };
        if title != self.title {
            w.set_title(&title);
            self.title = title;
        }
    }

    fn tick(&mut self, el: &ActiveEventLoop) {
        self.host.audio.poll();
        if let Some(s) = &mut self.smoke {
            for ev in s.due_input() {
                self.host.input.0.push_back(ev);
            }
        }
        self.handle_dialogs();
        self.handle_media_requests(el);
        if !self.drops.is_empty() {
            let drops = std::mem::take(&mut self.drops);
            self.push(InputEvent::DragOver(false));
            self.dragging_files = false;
            self.open_paths(drops, false);
        }
        self.app.tick(&mut self.host);
        if self.host.services.as_ref().is_some_and(|s| s.restart_flag().get()) {
            self.quit(el);
            return;
        }
        self.handle_effects();
        self.after_tick();
        if self.opts.paused && !self.started_paused && self.app.model().state.is_active() {
            self.started_paused = true;
            let now = rvp_host::HostClock::now_us(&self.host.clock);
            self.app.apply(&mut self.host, Action::PlayPause, now);
        }
        if let Some(s) = &mut self.smoke
            && s.tick(&self.app, &mut self.host)
        {
            self.quit(el);
        }
        if self.host.audio.failed() {
            // The device went away (unplugged, the sound server restarted): open it again at the next play.
            let now = rvp_host::HostClock::now_us(&self.host.clock);
            self.app.ui_mut().show_toast("The audio device went away.", now);
        }
    }

    /// How long until the next tick: fast while sound is being fed, calmer when nothing moves.
    fn interval(&self) -> Duration {
        let m = self.app.model();
        match m.state {
            MediaState::Playing | MediaState::Buffering | MediaState::Opening => Duration::from_millis(4),
            _ => {
                if self.app.scan_status().is_some() {
                    Duration::from_millis(8)
                } else {
                    Duration::from_millis(16)
                }
            }
        }
    }
}

impl ApplicationHandler for Handler {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none() {
            self.create_window(el);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.quit(el),
            WindowEvent::Resized(size) => {
                let dpr = self.window.as_ref().map_or(1.0, |w| w.scale_factor() as f32);
                let (w, h) = (size.width.max(1), size.height.max(1));
                self.host.surface.size = (w, h, dpr);
                self.push(InputEvent::Resize { w, h, dpr });
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let (w, h, _) = self.host.surface.size;
                self.host.surface.size.2 = scale_factor as f32;
                self.push(InputEvent::Resize { w, h, dpr: scale_factor as f32 });
            }
            WindowEvent::RedrawRequested | WindowEvent::Occluded(false) => {
                self.app.invalidate();
                self.wants_tick = true;
            }
            WindowEvent::Focused(on) => self.push(InputEvent::Focus(on)),
            WindowEvent::ModifiersChanged(m) => self.modifiers = Self::modifiers_from(m.state()),
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(key) = map_key(&event.logical_key) {
                    let mods = self.modifiers;
                    // Alt+Enter is full screen too.
                    let key =
                        if mods.alt && key == rvp_host::Key::Enter { rvp_host::Key::Char('f') } else { key };
                    let mods = if key == rvp_host::Key::Char('f')
                        && matches!(event.logical_key, winit::keyboard::Key::Named(_))
                    {
                        Modifiers::default()
                    } else {
                        mods
                    };
                    // Ctrl+V (Cmd+V on a Mac): the clipboard's text arrives as a `Paste` event.
                    if event.state == ElementState::Pressed
                        && !event.repeat
                        && (mods.ctrl || mods.logo)
                        && matches!(key, rvp_host::Key::Char('v' | 'V'))
                        && let Some(text) = clipboard_text()
                    {
                        self.push(InputEvent::Paste(text));
                    }
                    match event.state {
                        ElementState::Pressed => {
                            self.push(InputEvent::KeyDown { key, mods, repeat: event.repeat })
                        }
                        ElementState::Released => self.push(InputEvent::KeyUp { key, mods }),
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (position.x as f32, position.y as f32);
                self.push(InputEvent::PointerMove { x: self.pointer.0, y: self.pointer.1 });
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(b) = map_button(button) {
                    let (x, y) = self.pointer;
                    self.push(match state {
                        ElementState::Pressed => InputEvent::PointerDown { x, y, button: b },
                        ElementState::Released => InputEvent::PointerUp { x, y, button: b },
                    });
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (-x * WHEEL_LINE_PX, -y * WHEEL_LINE_PX),
                    MouseScrollDelta::PixelDelta(p) => (-p.x as f32, -p.y as f32),
                };
                self.push(InputEvent::Wheel { dx, dy });
            }
            WindowEvent::HoveredFile(_) => {
                if !self.dragging_files {
                    self.dragging_files = true;
                    self.push(InputEvent::DragOver(true));
                }
            }
            WindowEvent::HoveredFileCancelled => {
                self.dragging_files = false;
                self.push(InputEvent::DragOver(false));
            }
            WindowEvent::DroppedFile(p) => {
                self.drops.push(p);
                self.wants_tick = true;
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.quitting {
            return;
        }
        let now = Instant::now();
        let due = now >= self.next_tick;
        if (due || self.wants_tick) && now.duration_since(self.last_tick) >= MIN_TICK {
            self.wants_tick = false;
            self.last_tick = now;
            self.tick(el);
            self.next_tick = Instant::now() + self.interval();
        }
        el.set_control_flow(ControlFlow::WaitUntil(
            self.next_tick.min(Instant::now() + MIN_TICK.max(self.interval())),
        ));
    }
}

/// The window icon from the embedded PNG.
fn window_icon() -> Option<Icon> {
    static ICON: &[u8] = include_bytes!(
        "../../../packaging/icons/hicolor/256x256/apps/io.github.unicorntearsproject.RustyWave.png"
    );
    let img = rvp_library::art::decode(ICON, 256)?;
    Icon::from_rgba(img.rgba, img.w, img.h).ok()
}

#[cfg(windows)]
fn hwnd_of(window: &Window) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut std::ffi::c_void),
        _ => None,
    }
}

#[cfg(not(windows))]
fn hwnd_of(_window: &Window) -> Option<*mut std::ffi::c_void> {
    None
}

/// Where the data lives: `--data-dir`, else the user's data directory.
pub fn data_dir(opts: &Options) -> PathBuf {
    if let Some(d) = &opts.data_dir {
        return d.clone();
    }
    directories::ProjectDirs::from("", "", "rusty-wave")
        .map_or_else(|| std::env::temp_dir().join("rusty-wave"), |d| d.data_dir().to_path_buf())
}

// `Storage` is used through `host.storage` in sync helpers above.
#[allow(dead_code)]
fn _assert_storage<S: Storage>() {}

/// The text on the system clipboard (`None` when it holds none or cannot be read, as in a session without a clipboard).
fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok().filter(|t| !t.is_empty())
}
