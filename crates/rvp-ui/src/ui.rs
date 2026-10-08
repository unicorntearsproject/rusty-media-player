//! The player UI state machine: layout, hit testing, pointer and keyboard handling, menus, auto-hide.
//! Drawing lives in `draw.rs`; this file never touches pixels.
use crate::actions::{self, Action, MenuItem, SPEEDS};
use crate::font::Fonts;
use crate::gfx::RectF;
use crate::icon::IconCache;
use crate::model::UiModel;
use alloc::string::String;
use alloc::vec::Vec;
use rvp_host::{InputEvent, Key, Modifiers, PointerButton};

/// How long the controls stay up after the last pointer or key activity while playing.
pub const HIDE_AFTER_US: i64 = 2_500_000;
pub(crate) const DOUBLE_CLICK_US: i64 = 350_000;
pub(crate) const TOOLTIP_DELAY_US: i64 = 450_000;
const TOAST_US: i64 = 1_400_000;
const SCRUB_THROTTLE_US: i64 = 180_000;

/// UI settings supplied by the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct UiConfig {
    /// The user asked the system for reduced motion: no fades, no spinner animation.
    pub reduce_motion: bool,
}

/// Which mouse cursor the host should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// The normal arrow.
    Default,
    /// Over something clickable.
    Pointer,
    /// While dragging a slider.
    Grabbing,
    /// Hidden (controls are away during playback).
    Hidden,
}

/// A button in the transport bar (or the welcome card).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    /// Play / pause.
    Play,
    /// Back 10 s.
    Back,
    /// Forward 10 s.
    Fwd,
    /// Mute.
    Mute,
    /// Speed menu.
    Speed,
    /// Audio and subtitle menu.
    Tracks,
    /// The playlist menu.
    Playlist,
    /// Open a file.
    Open,
    /// Fullscreen.
    Fullscreen,
    /// The big button on the empty screen.
    Welcome,
    /// Previous track (library bar).
    Prev,
    /// Next track (library bar).
    Next,
    /// Shuffle (both bars).
    Shuffle,
    /// Repeat: off, all, one (both bars).
    Repeat,
    /// The queue view (library bar).
    QueueView,
    /// The visualizer view (library bar).
    VizView,
    /// Switch to the player (library bar), or to the library (player bar).
    ModeSwitch,
    /// Heart or un-heart what is playing (library bar).
    Favorite,
}

/// The player bar shows the shuffle and repeat buttons from this width (logical pixels) up: below it the time readout would
/// run into them.
pub(crate) const SHUFFLE_REPEAT_MIN_W: f32 = 800.0;

/// Tab order of the transport bar (buttons the window is too narrow for are skipped).
const FOCUS_ORDER: [Btn; 12] = [
    Btn::Play,
    Btn::Back,
    Btn::Fwd,
    Btn::Mute,
    Btn::Speed,
    Btn::Tracks,
    Btn::Playlist,
    Btn::Shuffle,
    Btn::Repeat,
    Btn::Open,
    Btn::ModeSwitch,
    Btn::Fullscreen,
];

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Nothing special: the picture.
    Video,
    /// The seek bar.
    Seek,
    /// The volume slider.
    Volume,
    /// A button.
    Btn(Btn),
    /// A menu row (panel, row).
    Menu(usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Drag {
    Seek,
    Volume,
}

/// Computed positions of every control, in physical pixels.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    /// Where the toast sits (its top edge); 0 for the default, near the top.
    pub toast_y: f32,
    /// Pixel scale (device pixel ratio).
    pub s: f32,
    /// Window size.
    pub w: f32,
    /// Window size.
    pub h: f32,
    /// Top of the bottom scrim.
    pub bar_top: f32,
    /// Seek bar hit area (taller than the track).
    pub seek_hit: RectF,
    /// Seek bar track at rest.
    pub seek_track: RectF,
    /// Volume slider hit area, if shown.
    pub vol_hit: Option<RectF>,
    /// Volume slider track.
    pub vol_track: RectF,
    /// Left edge of the time readout.
    pub time_x: f32,
    /// Vertical centre of the button row.
    pub row_cy: f32,
    /// Where the time readout sits (its centre line); the button row's for most windows, above the seek bar on a phone.
    pub time_y: f32,
    /// Buttons that exist at this size and state.
    pub buttons: Vec<(Btn, RectF)>,
    /// The empty-screen card.
    pub card: RectF,
}

impl Layout {
    /// The rectangle of a button, if it is shown.
    pub fn rect_of(&self, b: Btn) -> Option<RectF> {
        self.buttons.iter().find(|(k, _)| *k == b).map(|(_, r)| *r)
    }
}

/// One open menu panel.
#[derive(Debug, Clone)]
pub struct Panel {
    /// Rows.
    pub items: Vec<MenuItem>,
    /// Panel rectangle.
    pub rect: RectF,
    /// Row rectangles, same order as `items`.
    pub rows: Vec<RectF>,
    /// Highlighted row.
    pub hover: Option<usize>,
    /// Row of the parent panel this one hangs off.
    pub parent_row: Option<usize>,
    /// Show a check gutter.
    pub gutter: bool,
}

/// The UI.
pub struct Ui {
    /// Settings.
    pub config: UiConfig,
    pub(crate) fonts: Fonts,
    pub(crate) icons: IconCache,
    pub(crate) w: u32,
    pub(crate) h: u32,
    pub(crate) scale: f32,
    pub(crate) pointer: Option<(f32, f32)>,
    pub(crate) drag: Option<Drag>,
    pub(crate) pressed: Option<Target>,
    pub(crate) hover: Target,
    pub(crate) hover_since: i64,
    /// Tooltips are on (the Settings switch).
    pub(crate) tips_on: bool,
    /// What the library tooltip is about, since when, and whether it was drawn.
    pub(crate) tip_target: Option<crate::lib_ui::LibHit>,
    pub(crate) tip_since: i64,
    pub(crate) tip_drawn: bool,
    /// Since when the keyboard has been on the player face control `focus`.
    pub(crate) focus_since: i64,
    /// The dialog or audio-panel control the tooltip is about, since when, and whether it was drawn.
    pub(crate) dlg_sig: Option<i32>,
    pub(crate) dlg_since: i64,
    pub(crate) dlg_drawn: bool,
    pub(crate) last_activity: i64,
    pub(crate) controls_alpha: f32,
    pub(crate) last_update: i64,
    pub(crate) menu: Vec<Panel>,
    pub(crate) focus: Option<Btn>,
    pub(crate) toast: Option<(String, i64)>,
    pub(crate) last_click: Option<(i64, f32, f32)>,
    pub(crate) wheel_acc: f32,
    pub(crate) drag_over: bool,
    pub(crate) scrub: Option<f32>,
    pub(crate) last_scrub_emit: i64,
    pub(crate) now: i64,
    pub(crate) hover_seek: Option<f32>,
    pub(crate) dirty: bool,
    /// True once a key press has been seen; focus rings only show then.
    pub(crate) keyboard_mode: bool,
    pub(crate) win_focused: bool,
    pub(crate) media_active: bool,
    pub(crate) key_used: bool,
    /// The library face's state.
    pub(crate) lib: crate::lib_ui::LibUi,
    /// What the primary button went down on in library mode.
    pub(crate) pressed_lib: Option<crate::lib_ui::LibHit>,
    /// The Audio settings panel, while it is up.
    pub(crate) audio_panel: Option<crate::audio_panel::AudioPanel>,
    /// The Help overlay, while it is up.
    pub(crate) help: Option<crate::help::HelpState>,
    /// The state of the application's modal dialog (focus, hover), while one is up.
    pub(crate) dialog: crate::dialog::DialogState,
    /// A dialog was up at the last draw or input.
    pub(crate) dialog_open: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new(UiConfig::default())
    }
}

impl Ui {
    /// A UI for a 1280 x 720 surface until the host says otherwise.
    pub fn new(config: UiConfig) -> Self {
        Self {
            config,
            fonts: Fonts::new(),
            icons: IconCache::new(),
            w: 1280,
            h: 720,
            scale: 1.0,
            pointer: None,
            drag: None,
            pressed: None,
            hover: Target::Video,
            hover_since: 0,
            tips_on: true,
            tip_target: None,
            tip_since: 0,
            tip_drawn: false,
            focus_since: 0,
            dlg_sig: None,
            dlg_since: 0,
            dlg_drawn: false,
            last_activity: 0,
            controls_alpha: 1.0,
            last_update: 0,
            menu: Vec::new(),
            focus: None,
            toast: None,
            last_click: None,
            wheel_acc: 0.0,
            drag_over: false,
            scrub: None,
            last_scrub_emit: 0,
            now: 0,
            hover_seek: None,
            dirty: true,
            keyboard_mode: false,
            win_focused: true,
            media_active: false,
            key_used: false,
            lib: crate::lib_ui::LibUi::default(),
            pressed_lib: None,
            audio_panel: None,
            help: None,
            dialog: crate::dialog::DialogState::default(),
            dialog_open: false,
        }
    }

    /// Let the host find a system font for characters the bundled fonts do not have (CJK, Arabic, ...). It is asked
    /// when such a character is first drawn, and its answer is kept.
    pub fn set_font_loader(&mut self, loader: crate::font::FontLoader) {
        self.fonts.set_loader(loader);
        self.dirty = true;
    }

    /// Add a font file that backs up the bundled fonts. Returns false if it does not parse.
    pub fn add_fallback_font(&mut self, bytes: &[u8], index: u32) -> bool {
        self.dirty = true;
        self.fonts.add_fallback(bytes, index)
    }

    /// Surface size in physical pixels and the device pixel ratio.
    pub fn set_size(&mut self, w: u32, h: u32, dpr: f32) {
        if (self.w, self.h) != (w, h) || (self.scale - dpr).abs() > 1e-3 {
            self.w = w.max(1);
            self.h = h.max(1);
            self.scale = dpr.max(0.5);
            self.menu.clear();
            self.dirty = true;
        }
    }

    /// Current surface size.
    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// Pixel scale.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Ask for a redraw.
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Show a short message at the top of the picture.
    pub fn show_toast(&mut self, text: &str, now_us: i64) {
        self.toast = Some((text.into(), now_us + TOAST_US));
        self.dirty = true;
    }

    /// The message of the toast that is up, if any (tests wait for it to go before they compare a picture).
    pub fn toast_text(&self) -> Option<&str> {
        self.toast.as_ref().map(|(t, _)| t.as_str())
    }

    /// True while a context menu or popup is open.
    pub fn menu_open(&self) -> bool {
        !self.menu.is_empty()
    }

    /// True while a slider is being dragged.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Fraction under a seek drag, if one is in progress.
    pub fn scrub_fraction(&self) -> Option<f32> {
        self.scrub
    }

    /// Where the picture goes: `src` fitted into the window, centred.
    pub fn video_rect(&self, src_w: u32, src_h: u32) -> RectF {
        let (ww, wh) = (self.w as f32, self.h as f32);
        if src_w == 0 || src_h == 0 {
            return RectF::new(0.0, 0.0, ww, wh);
        }
        let k = (ww / src_w as f32).min(wh / src_h as f32);
        let (vw, vh) = (src_w as f32 * k, src_h as f32 * k);
        RectF::new(
            libm::floorf((ww - vw) * 0.5 + 0.5),
            libm::floorf((wh - vh) * 0.5 + 0.5),
            libm::floorf(vw),
            libm::floorf(vh),
        )
    }

    /// Tell the UI whether the Player face has nothing loaded (then it is drawn and handled inside the library's frame).
    pub fn set_player_empty(&mut self, empty: bool) {
        if self.lib.player_empty != empty {
            self.lib.player_empty = empty;
            self.dirty = true;
        }
    }

    /// The library's frame (rail, header, bar) is what is on screen: the Library face, or the Player face with nothing loaded.
    pub fn lib_chrome(&self) -> bool {
        self.lib.mode == crate::lib_ui::Mode::Library || self.lib.player_empty
    }

    /// The cursor the host should show.
    pub fn cursor(&self, model: &UiModel) -> Cursor {
        if self.lib_chrome() {
            use crate::lib_ui::{LibDrag, LibHit};
            if matches!(
                self.lib.drag,
                Some(
                    LibDrag::Scroll { .. }
                        | LibDrag::Seek
                        | LibDrag::Volume
                        | LibDrag::Reorder { moved: true, .. }
                )
            ) {
                return Cursor::Grabbing;
            }
            if self.lib.view == crate::lib_ui::View::Visualizer
                && model.state.is_active()
                && self.controls_alpha < 0.05
                && self.menu.is_empty()
            {
                return Cursor::Hidden;
            }
            return match self.lib.hover {
                LibHit::None => Cursor::Default,
                _ => Cursor::Pointer,
            };
        }
        if self.drag.is_some() {
            return Cursor::Grabbing;
        }
        match self.hover {
            Target::Btn(_) | Target::Seek | Target::Volume | Target::Menu(..) => Cursor::Pointer,
            Target::Video => {
                if model.state.is_active() && self.controls_alpha < 0.05 && self.menu.is_empty() {
                    Cursor::Hidden
                } else {
                    Cursor::Default
                }
            }
        }
    }

    /// True if anything is drawn over the picture right now (controls, a menu, a toast, the drop outline).
    pub fn has_overlay(&self) -> bool {
        self.lib_chrome()
            || self.controls_alpha > 0.005
            || !self.menu.is_empty()
            || self.toast.is_some()
            || self.drag_over
            || self.audio_panel.is_some()
            || self.help.is_some()
            || self.dialog_open
    }

    /// Whether the transport bar is (becoming) visible.
    pub fn controls_visible(&self) -> bool {
        self.lib_chrome() || self.controls_alpha > 0.01
    }

    /// Current controls opacity (0.0..=1.0), for tests and screenshots.
    pub fn controls_opacity(&self) -> f32 {
        self.controls_alpha
    }

    /// The control currently highlighted by the pointer.
    pub fn hover_target(&self) -> Target {
        self.hover
    }

    // ---- layout ----------------------------------------------------------------------------------------

    /// Compute the layout for the current size and model.
    pub fn layout(&self, model: &UiModel) -> Layout {
        let s = self.scale;
        let (w, h) = (self.w as f32, self.h as f32);
        let m = 20.0 * s;
        let mut l = Layout { s, w, h, bar_top: h - 160.0 * s, row_cy: h - 38.0 * s, ..Layout::default() };
        l.seek_hit = RectF::new(m, h - 84.0 * s, w - 2.0 * m, 20.0 * s);
        l.seek_track = RectF::new(m, h - 74.0 * s - 2.0 * s, w - 2.0 * m, 4.0 * s);
        let card_w = (560.0 * s).min(w - 40.0 * s);
        let card_h = (330.0 * s).min(h - 40.0 * s);
        l.card = RectF::new((w - card_w) * 0.5, (h - card_h) * 0.5, card_w, card_h);
        if !model.has_media() {
            let bw = 168.0 * s;
            let bh = 48.0 * s;
            l.buttons.push((
                Btn::Welcome,
                RectF::new(l.card.cx() - bw * 0.5, l.card.bottom() - 112.0 * s, bw, bh),
            ));
            return l;
        }
        l.time_y = l.row_cy;
        if w < 600.0 * s {
            self.layout_phone(&mut l);
            return l;
        }
        let cy = l.row_cy;
        let mut x = m;
        let play = 44.0 * s;
        l.buttons.push((Btn::Play, RectF::new(x, cy - play * 0.5, play, play)));
        x += play + 8.0 * s;
        let b = 36.0 * s;
        if w >= 560.0 * s {
            l.buttons.push((Btn::Back, RectF::new(x, cy - b * 0.5, b, b)));
            x += b + 2.0 * s;
            l.buttons.push((Btn::Fwd, RectF::new(x, cy - b * 0.5, b, b)));
            x += b + 6.0 * s;
        }
        l.buttons.push((Btn::Mute, RectF::new(x, cy - b * 0.5, b, b)));
        x += b + 2.0 * s;
        if w >= 760.0 * s {
            let sw = 84.0 * s;
            l.vol_hit = Some(RectF::new(x, cy - 14.0 * s, sw, 28.0 * s));
            l.vol_track = RectF::new(x + 6.0 * s, cy - 2.0 * s, sw - 12.0 * s, 4.0 * s);
            x += sw + 14.0 * s;
        } else {
            x += 10.0 * s;
        }
        l.time_x = x;
        let mut rx = w - m;
        l.buttons.push((Btn::Fullscreen, RectF::new(rx - b, cy - b * 0.5, b, b)));
        rx -= b + 2.0 * s;
        l.buttons.push((Btn::Open, RectF::new(rx - b, cy - b * 0.5, b, b)));
        rx -= b + 2.0 * s;
        l.buttons.push((Btn::Tracks, RectF::new(rx - b, cy - b * 0.5, b, b)));
        rx -= b + 2.0 * s;
        l.buttons.push((Btn::Playlist, RectF::new(rx - b, cy - b * 0.5, b, b)));
        rx -= b + 2.0 * s;
        // Shuffle and repeat are the first to go when the window narrows (the keys Z and R and the menus stay).
        if w >= SHUFFLE_REPEAT_MIN_W * s {
            l.buttons.push((Btn::Repeat, RectF::new(rx - b, cy - b * 0.5, b, b)));
            rx -= b + 2.0 * s;
            l.buttons.push((Btn::Shuffle, RectF::new(rx - b, cy - b * 0.5, b, b)));
            rx -= b + 2.0 * s;
        }
        l.buttons.push((Btn::ModeSwitch, RectF::new(rx - b, cy - b * 0.5, b, b)));
        rx -= b + 6.0 * s;
        let pw = 56.0 * s;
        l.buttons.push((Btn::Speed, RectF::new(rx - pw, cy - 14.0 * s, pw, 28.0 * s)));
        l
    }

    /// The controls of a phone-width window, in two rows of touch-sized buttons under the seek bar and its times:
    /// mute, back, play, forward; then the mode switch, queue, tracks, speed, open and fullscreen.
    fn layout_phone(&self, l: &mut Layout) {
        let s = l.s;
        let (w, h) = (l.w, l.h);
        let m = 16.0 * s;
        l.bar_top = h - 204.0 * s;
        l.time_y = h - 172.0 * s;
        l.time_x = m;
        l.seek_track = RectF::new(m, h - 148.0 * s - 2.0 * s, w - 2.0 * m, 4.0 * s);
        l.seek_hit = RectF::new(m, h - 148.0 * s - 18.0 * s, w - 2.0 * m, 36.0 * s);
        l.vol_hit = None;
        let gap = 8.0 * s;
        let row = |l: &mut Layout, cy: f32, items: &[(Btn, f32, f32)]| {
            let total: f32 =
                items.iter().map(|(_, bw, _)| bw * s).sum::<f32>() + gap * (items.len() as f32 - 1.0);
            let mut x = (w - total) * 0.5;
            for (b, bw, bh) in items {
                l.buttons.push((*b, RectF::new(x, cy - bh * s * 0.5, bw * s, bh * s)));
                x += bw * s + gap;
            }
        };
        row(
            l,
            h - 96.0 * s,
            &[
                (Btn::Mute, 44.0, 44.0),
                (Btn::Back, 44.0, 44.0),
                (Btn::Play, 56.0, 56.0),
                (Btn::Fwd, 44.0, 44.0),
            ],
        );
        l.row_cy = h - 34.0 * s;
        row(
            l,
            l.row_cy,
            &[
                (Btn::ModeSwitch, 44.0, 44.0),
                (Btn::Playlist, 44.0, 44.0),
                (Btn::Tracks, 44.0, 44.0),
                (Btn::Speed, 60.0, 36.0),
                (Btn::Open, 44.0, 44.0),
                (Btn::Fullscreen, 44.0, 44.0),
            ],
        );
    }

    fn hit_test(&self, x: f32, y: f32, l: &Layout, model: &UiModel) -> Target {
        for (pi, p) in self.menu.iter().enumerate().rev() {
            if p.rect.contains(x, y) {
                for (ri, r) in p.rows.iter().enumerate() {
                    if r.contains(x, y) {
                        return Target::Menu(pi, ri);
                    }
                }
                return Target::Menu(pi, usize::MAX);
            }
        }
        if !model.has_media() {
            return match l.rect_of(Btn::Welcome) {
                Some(r) if r.contains(x, y) => Target::Btn(Btn::Welcome),
                _ => Target::Video,
            };
        }
        if self.controls_alpha > 0.3 {
            for (b, r) in &l.buttons {
                if r.contains(x, y) {
                    return Target::Btn(*b);
                }
            }
            if l.seek_hit.contains(x, y) {
                return Target::Seek;
            }
            if l.vol_hit.is_some_and(|r| r.contains(x, y)) {
                return Target::Volume;
            }
        }
        Target::Video
    }

    fn seek_fraction(&self, x: f32, l: &Layout) -> f32 {
        ((x - l.seek_track.x) / l.seek_track.w.max(1.0)).clamp(0.0, 1.0)
    }

    fn volume_fraction(&self, x: f32, l: &Layout) -> f32 {
        ((x - l.vol_track.x) / l.vol_track.w.max(1.0)).clamp(0.0, 1.0)
    }

    /// Fraction of the duration the pointer would seek to, when hovering the seek bar.
    pub fn hover_seek_fraction(&self) -> Option<f32> {
        self.hover_seek
    }

    // ---- time ------------------------------------------------------------------------------------------

    /// Advance animations and timers. Returns true if the picture should be redrawn.
    pub fn update(&mut self, now_us: i64, model: &UiModel) -> bool {
        let dt = (now_us - self.last_update).clamp(0, 200_000);
        self.last_update = now_us;
        self.now = now_us;
        let mut redraw = core::mem::take(&mut self.dirty);
        if model.state.is_active() != self.media_active {
            self.media_active = model.state.is_active();
            self.last_activity = now_us;
            redraw = true;
        }
        let target = if self.controls_wanted(model) { 1.0 } else { 0.0 };
        if (self.controls_alpha - target).abs() > 1e-3 {
            if self.config.reduce_motion {
                self.controls_alpha = target;
            } else {
                let dur = if target > self.controls_alpha { 220_000.0 } else { 400_000.0 };
                let step = dt as f32 / dur;
                self.controls_alpha = if target > self.controls_alpha {
                    (self.controls_alpha + step).min(target)
                } else {
                    (self.controls_alpha - step).max(target)
                };
            }
            redraw = true;
        }
        if let Some((_, until)) = &self.toast {
            if now_us >= *until {
                self.toast = None;
                redraw = true;
            }
        }
        // The tooltip appears after a delay (for the pointer, or for the keyboard's focus).
        if self.tips_on
            && (matches!(self.hover, Target::Btn(_) | Target::Seek | Target::Volume) || self.focus.is_some())
        {
            let age = now_us
                - if self.focus.is_some() && self.keyboard_mode {
                    self.focus_since
                } else {
                    self.hover_since
                };
            if age >= TOOLTIP_DELAY_US && age - dt < TOOLTIP_DELAY_US + 50_000 {
                redraw = true;
            }
        }
        if !self.config.reduce_motion
            && matches!(model.state, crate::model::MediaState::Buffering | crate::model::MediaState::Opening)
        {
            redraw = true; // the spinner
        }
        if self.overlay_tip_tick(model, now_us) {
            redraw = true;
        }
        if self.lib_chrome() {
            if self.lib_tip_tick(now_us) {
                redraw = true;
            }
            // The equaliser bars and the text caret.
            if self.lib.animated && !self.config.reduce_motion && now_us - self.lib.anim_at >= 120_000 {
                self.lib.anim_at = now_us;
                redraw = true;
            }
            let typing = self.lib.zone == crate::lib_ui::Zone::Search || self.lib.prompt.is_some();
            let blink = (now_us / 530_000) % 2 == 0;
            if typing && blink != self.lib.blink {
                redraw = true;
            }
            self.lib.blink = blink;
        }
        redraw
    }

    fn controls_wanted(&self, model: &UiModel) -> bool {
        if self.lib_chrome() {
            // The library's bar is always there; the visualizer's floats and hides like the player's.
            if self.lib.view != crate::lib_ui::View::Visualizer {
                return true;
            }
            return !model.state.is_active()
                || !self.menu.is_empty()
                || self.lib.drag.is_some()
                || matches!(
                    self.lib.hover,
                    crate::lib_ui::LibHit::Bar(_)
                        | crate::lib_ui::LibHit::Seek
                        | crate::lib_ui::LibHit::Volume
                        | crate::lib_ui::LibHit::Viz(_)
                )
                || self.now - self.last_activity < HIDE_AFTER_US;
        }
        if !model.has_media() {
            return false;
        }
        if !model.state.is_active() || !self.menu.is_empty() || self.drag.is_some() || self.focus.is_some() {
            return true;
        }
        if matches!(self.hover, Target::Btn(_) | Target::Seek | Target::Volume) {
            return true;
        }
        self.now - self.last_activity < HIDE_AFTER_US
    }

    /// True if a tooltip should be shown for the hovered control.
    pub(crate) fn tooltip_ready(&self) -> bool {
        self.tips_on
            && self.menu.is_empty()
            && self.drag.is_none()
            && if self.keyboard_mode && self.focus.is_some() {
                self.now - self.focus_since >= TOOLTIP_DELAY_US
            } else {
                matches!(self.hover, Target::Btn(_) | Target::Seek | Target::Volume)
                    && self.now - self.hover_since >= TOOLTIP_DELAY_US
            }
    }

    // ---- input -----------------------------------------------------------------------------------------

    /// Handle one input event; returns what the user asked for.
    pub fn handle(&mut self, ev: &InputEvent, now_us: i64, model: &UiModel) -> Vec<Action> {
        if let Some(d) = &model.dialog {
            self.dialog_open = true;
            return self.dialog_event(ev, now_us, d);
        }
        self.dialog_closed();
        if self.help.is_some() {
            return self.help_event(ev, now_us, model);
        }
        if self.audio_panel.is_some() {
            return self.audio_panel_event(ev, now_us, model);
        }
        self.now = now_us;
        let mut out = Vec::new();
        self.dirty = true;
        match ev {
            InputEvent::Resize { w, h, dpr } => self.set_size(*w, *h, *dpr),
            InputEvent::Focus(f) => {
                self.win_focused = *f;
                if !*f {
                    self.drag = None;
                    self.scrub = None;
                    self.pressed = None;
                }
            }
            InputEvent::DragOver(on) => self.drag_over = *on,
            InputEvent::Drop { .. } => self.drag_over = false,
            InputEvent::PointerMove { x, y } => self.on_move(*x, *y, now_us, model, &mut out),
            InputEvent::PointerDown { x, y, button } => {
                self.on_down(*x, *y, *button, now_us, model, &mut out)
            }
            InputEvent::PointerUp { x, y, button } => self.on_up(*x, *y, *button, now_us, model, &mut out),
            InputEvent::Wheel { dy, .. } => self.on_wheel(*dy, model, &mut out),
            InputEvent::KeyDown { key, mods, repeat } => {
                self.on_key(key, mods, *repeat, now_us, model, &mut out)
            }
            InputEvent::KeyUp { .. } | InputEvent::Paste(_) => {}
        }
        out
    }

    fn set_hover(&mut self, t: Target, now_us: i64) {
        if t != self.hover {
            self.hover = t;
            self.hover_since = now_us;
        }
    }

    fn on_move(&mut self, x: f32, y: f32, now_us: i64, model: &UiModel, out: &mut Vec<Action>) {
        let moved = self.pointer.is_none_or(|(px, py)| (px - x).abs() + (py - y).abs() > 0.5);
        self.pointer = Some((x, y));
        if moved {
            self.last_activity = now_us;
            self.keyboard_mode = false;
        }
        let l = self.layout(model);
        match self.drag {
            Some(Drag::Seek) => {
                let f = self.seek_fraction(x, &l);
                self.scrub = Some(f);
                if now_us - self.last_scrub_emit >= SCRUB_THROTTLE_US {
                    self.last_scrub_emit = now_us;
                    out.push(Action::SeekFraction(f));
                }
                return;
            }
            Some(Drag::Volume) => {
                out.push(Action::SetVolume(self.volume_fraction(x, &l)));
                return;
            }
            None => {}
        }
        let t = self.hit_test(x, y, &l, model);
        self.hover_seek = if t == Target::Seek { Some(self.seek_fraction(x, &l)) } else { None };
        // Menu hover: highlight rows, open or close submenus.
        if let Target::Menu(pi, ri) = t {
            if ri != usize::MAX {
                self.menu_hover(pi, ri);
            }
        }
        self.set_hover(t, now_us);
    }

    fn on_down(
        &mut self,
        x: f32,
        y: f32,
        button: PointerButton,
        now_us: i64,
        model: &UiModel,
        out: &mut Vec<Action>,
    ) {
        self.pointer = Some((x, y));
        self.last_activity = now_us;
        self.keyboard_mode = false;
        self.focus = None;
        let l = self.layout(model);
        match button {
            PointerButton::Secondary => {
                self.open_menu_at(actions::context_menu(model), x, y, None);
            }
            PointerButton::Middle => {
                self.menu.clear();
                if model.has_media() {
                    out.push(Action::PlayPause);
                }
            }
            PointerButton::Primary => {
                let t = self.hit_test(x, y, &l, model);
                self.pressed = Some(t);
                if self.menu.is_empty() || matches!(t, Target::Menu(..)) {
                    match t {
                        Target::Seek if model.duration_us.is_some() => {
                            let f = self.seek_fraction(x, &l);
                            self.drag = Some(Drag::Seek);
                            self.scrub = Some(f);
                            self.last_scrub_emit = now_us;
                            out.push(Action::SeekFraction(f));
                        }
                        Target::Volume => {
                            self.drag = Some(Drag::Volume);
                            out.push(Action::SetVolume(self.volume_fraction(x, &l)));
                        }
                        _ => {}
                    }
                } else {
                    // A click outside an open menu only closes it.
                    self.menu.clear();
                    self.pressed = None;
                }
            }
            PointerButton::Back => {
                self.menu.clear();
                out.push(Action::Prev);
            }
            PointerButton::Forward => {
                self.menu.clear();
                out.push(Action::Next);
            }
        }
    }

    fn on_up(
        &mut self,
        x: f32,
        y: f32,
        button: PointerButton,
        now_us: i64,
        model: &UiModel,
        out: &mut Vec<Action>,
    ) {
        self.pointer = Some((x, y));
        if button != PointerButton::Primary {
            return;
        }
        let l = self.layout(model);
        let pressed = self.pressed.take();
        match self.drag.take() {
            Some(Drag::Seek) => {
                let f = self.seek_fraction(x, &l);
                self.scrub = None;
                out.push(Action::SeekFraction(f));
                return;
            }
            Some(Drag::Volume) => {
                out.push(Action::SetVolume(self.volume_fraction(x, &l)));
                return;
            }
            None => {}
        }
        let t = self.hit_test(x, y, &l, model);
        if pressed != Some(t) {
            return;
        }
        match t {
            Target::Menu(pi, ri) if ri != usize::MAX => {
                let item = self.menu[pi].items[ri].clone();
                if !item.enabled {
                    return;
                }
                if let Some(a) = item.action {
                    out.push(a);
                    self.menu.clear();
                } else if !item.sub.is_empty() {
                    self.menu_hover(pi, ri);
                }
            }
            Target::Btn(b) => self.activate(b, model, &l, out),
            // On a phone the first tap on the picture shows the controls; the next one plays or pauses.
            Target::Video if model.has_media() && l.w < 600.0 * l.s && self.controls_alpha <= 0.3 => {
                self.last_activity = now_us;
            }
            Target::Video if model.has_media() => {
                let double = self.last_click.is_some_and(|(t0, cx, cy)| {
                    now_us - t0 <= DOUBLE_CLICK_US && (cx - x).abs() + (cy - y).abs() < 12.0 * self.scale
                });
                out.push(Action::PlayPause);
                if double {
                    out.push(Action::ToggleFullscreen);
                    self.last_click = None;
                } else {
                    self.last_click = Some((now_us, x, y));
                }
            }
            _ => {}
        }
    }

    fn activate(&mut self, b: Btn, model: &UiModel, l: &Layout, out: &mut Vec<Action>) {
        match b {
            Btn::Play => out.push(Action::PlayPause),
            Btn::Back => out.push(Action::SeekBy(-10_000)),
            Btn::Fwd => out.push(Action::SeekBy(10_000)),
            Btn::Mute => out.push(Action::ToggleMute),
            Btn::Open | Btn::Welcome => out.push(Action::OpenFile),
            Btn::Fullscreen => out.push(Action::ToggleFullscreen),
            Btn::Speed => {
                if let Some(r) = l.rect_of(Btn::Speed) {
                    self.open_popup(actions::speed_menu(model), r);
                }
            }
            Btn::Tracks => {
                if let Some(r) = l.rect_of(Btn::Tracks) {
                    self.open_popup(actions::tracks_menu(model), r);
                }
            }
            Btn::Playlist => self.open_playlist_popup(model),
            Btn::ModeSwitch => out.push(Action::SetMode(crate::lib_ui::Mode::Library)),
            Btn::Shuffle => out.push(Action::ToggleShuffle),
            Btn::Repeat => out.push(Action::CycleRepeat),
            Btn::Favorite => out.push(Action::ToggleFavorite),
            Btn::Prev | Btn::Next | Btn::QueueView | Btn::VizView => {}
        }
    }

    fn on_wheel(&mut self, dy: f32, model: &UiModel, out: &mut Vec<Action>) {
        if !model.has_media() {
            return;
        }
        self.wheel_acc += dy;
        let notch = 40.0;
        while self.wheel_acc.abs() >= notch {
            let up = self.wheel_acc < 0.0; // scrolling up
            self.wheel_acc -= notch.copysign(self.wheel_acc);
            let over_seek = self.hover == Target::Seek;
            out.push(if over_seek {
                Action::SeekBy(if up { 5_000 } else { -5_000 })
            } else {
                Action::VolumeBy(if up { 5 } else { -5 })
            });
        }
    }

    fn on_key(
        &mut self,
        key: &Key,
        mods: &Modifiers,
        repeat: bool,
        now_us: i64,
        model: &UiModel,
        out: &mut Vec<Action>,
    ) {
        self.last_activity = now_us;
        self.keyboard_mode = true;
        let before = out.len();
        let had_menu = !self.menu.is_empty();
        let focus_before = self.focus;
        self.key_used = false;
        self.on_key_inner(key, mods, repeat, model, out);
        // Report whether the page should keep the key from the browser.
        self.key_used = out.len() > before
            || had_menu
            || !self.menu.is_empty()
            || self.focus != focus_before
            || (matches!(key, Key::Escape) && focus_before.is_some());
    }

    /// True if the last key press did something (the host should then suppress the browser's own handling).
    pub fn last_key_used(&self) -> bool {
        self.key_used
    }

    fn on_key_inner(
        &mut self,
        key: &Key,
        mods: &Modifiers,
        repeat: bool,
        model: &UiModel,
        out: &mut Vec<Action>,
    ) {
        if !self.menu.is_empty() {
            self.menu_key(key, out);
            return;
        }
        match key {
            Key::Escape => {
                if self.focus.is_some() {
                    self.focus = None;
                } else if model.fullscreen {
                    out.push(Action::ToggleFullscreen);
                }
                return;
            }
            Key::Enter => {
                if let Some(b) = self.focus {
                    let l = self.layout(model);
                    self.activate(b, model, &l, out);
                } else if !model.has_media() {
                    out.push(Action::OpenFile);
                }
                return;
            }
            Key::Other(name) if name != "PageUp" && name != "PageDown" => {
                match name.as_str() {
                    "Tab" => self.cycle_focus(mods.shift, model),
                    "ContextMenu" => self.open_key_menu(model),
                    "F10" if mods.shift => self.open_key_menu(model),
                    _ => {}
                }
                return;
            }
            _ => {}
        }
        if let Some(a) = actions::shortcut_for(key, mods).filter(|a| *a != Action::Quit || model.app.quit) {
            // Holding a key repeats seeks, volume and speed steps; play/pause, mute, fullscreen and open do not.
            let repeatable = matches!(
                a,
                Action::SeekBy(_) | Action::VolumeBy(_) | Action::SpeedStep(_) | Action::FrameStep(_)
            );
            if !repeat || repeatable {
                out.push(a);
            }
        }
    }

    fn cycle_focus(&mut self, back: bool, model: &UiModel) {
        let l = self.layout(model);
        let order: Vec<Btn> = if model.has_media() {
            FOCUS_ORDER.iter().copied().filter(|b| l.rect_of(*b).is_some()).collect()
        } else {
            alloc::vec![Btn::Welcome]
        };
        let cur = self.focus.and_then(|f| order.iter().position(|b| *b == f));
        let n = order.len();
        // Tabbing past either end leaves the player so keyboard users are not trapped in the canvas.
        if (cur == Some(n - 1) && !back) || (cur == Some(0) && back) {
            self.focus = None;
            return;
        }
        let next = match (cur, back) {
            (None, false) => 0,
            (None, true) => n - 1,
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
        };
        self.focus = Some(order[next]);
        self.focus_since = self.now;
    }

    // ---- menus -----------------------------------------------------------------------------------------

    fn panel_for(&mut self, items: Vec<MenuItem>) -> Panel {
        let s = self.scale;
        let gutter = items.iter().any(|i| i.checked);
        let mut widest = 0.0f32;
        for it in &items {
            let lw = self.fonts.measure(crate::font::Face::Sans, 14.0 * s, &it.label, 0.0);
            let hw = if it.hint.is_empty() {
                0.0
            } else {
                self.fonts.measure(crate::font::Face::Mono, 11.0 * s, &it.hint, 0.0) + 28.0 * s
            };
            let chev = if it.sub.is_empty() { 0.0 } else { 22.0 * s };
            widest = widest.max(lw + hw + chev);
        }
        let w = (widest + 32.0 * s + if gutter { 22.0 * s } else { 0.0 }).max(188.0 * s);
        let pad = 6.0 * s;
        let mut y = pad;
        let mut rows = Vec::new();
        for it in &items {
            if it.separator && !rows.is_empty() {
                y += 9.0 * s;
            }
            let row_h = if it.heading { 28.0 * s } else { 32.0 * s };
            rows.push(RectF::new(0.0, y, w, row_h));
            y += row_h;
        }
        Panel { items, rect: RectF::new(0.0, 0.0, w, y + pad), rows, hover: None, parent_row: None, gutter }
    }

    fn place(&self, p: &mut Panel, x: f32, y: f32) {
        let (ww, wh) = (self.w as f32, self.h as f32);
        let m = 8.0 * self.scale;
        let px = x.min(ww - p.rect.w - m).max(m);
        let py = y.min(wh - p.rect.h - m).max(m);
        let (dx, dy) = (px - p.rect.x, py - p.rect.y);
        p.rect.x = px;
        p.rect.y = py;
        for r in &mut p.rows {
            r.x += dx;
            r.y += dy;
        }
    }

    pub(crate) fn open_menu_at(&mut self, items: Vec<MenuItem>, x: f32, y: f32, hover: Option<usize>) {
        let mut p = self.panel_for(items);
        self.place(&mut p, x, y);
        p.hover = hover;
        self.menu = alloc::vec![p];
    }

    fn open_popup(&mut self, items: Vec<MenuItem>, anchor: RectF) {
        let mut p = self.panel_for(items);
        let x = anchor.cx() - p.rect.w * 0.5;
        let y = anchor.y - 10.0 * self.scale - p.rect.h;
        self.place(&mut p, x, y);
        self.menu = alloc::vec![p];
    }

    /// Open the playlist menu above its button.
    pub fn open_playlist_popup(&mut self, model: &UiModel) {
        let l = self.layout(model);
        if let Some(r) = l.rect_of(Btn::Playlist) {
            self.open_popup(actions::playlist_menu(model), r);
        }
    }

    fn open_key_menu(&mut self, model: &UiModel) {
        let (x, y) = self.pointer.unwrap_or((self.w as f32 * 0.5, self.h as f32 * 0.5));
        let items = actions::context_menu(model);
        let first = items.iter().position(|i| i.enabled);
        self.open_menu_at(items, x, y, first);
    }

    /// Highlight row `ri` of panel `pi`, closing deeper panels and opening its submenu if it has one.
    pub(crate) fn menu_hover(&mut self, pi: usize, ri: usize) {
        if pi >= self.menu.len() {
            return;
        }
        self.menu[pi].hover = Some(ri);
        let (sub, enabled) = {
            let it = &self.menu[pi].items[ri];
            (it.sub.clone(), it.enabled)
        };
        if pi + 1 < self.menu.len() && self.menu[pi + 1].parent_row == Some(ri) {
            return; // already open
        }
        self.menu.truncate(pi + 1);
        if sub.is_empty() || !enabled {
            return;
        }
        let mut child = self.panel_for(sub);
        let parent = &self.menu[pi];
        let row = parent.rows[ri];
        let ww = self.w as f32;
        let overlap = 4.0 * self.scale;
        let mut x = parent.rect.right() - overlap;
        if x + child.rect.w > ww - 8.0 * self.scale {
            x = parent.rect.x - child.rect.w + overlap;
        }
        let y = row.y - 6.0 * self.scale;
        self.place(&mut child, x, y);
        child.parent_row = Some(ri);
        self.menu.push(child);
    }

    pub(crate) fn menu_key(&mut self, key: &Key, out: &mut Vec<Action>) {
        let last = self.menu.len() - 1;
        let n = self.menu[last].items.len();
        let step = |p: &Panel, from: Option<usize>, dir: isize| -> Option<usize> {
            let n = p.items.len() as isize;
            let mut i = from.map_or(if dir > 0 { -1 } else { n }, |v| v as isize);
            for _ in 0..n {
                i = (i + dir).rem_euclid(n);
                if p.items[i as usize].enabled {
                    return Some(i as usize);
                }
            }
            None
        };
        match key {
            Key::Escape => self.menu.clear(),
            Key::Down | Key::Up => {
                let dir = if matches!(key, Key::Down) { 1 } else { -1 };
                let h = step(&self.menu[last], self.menu[last].hover, dir);
                self.menu[last].hover = h;
            }
            Key::Home => self.menu[last].hover = step(&self.menu[last], None, 1),
            Key::End => self.menu[last].hover = step(&self.menu[last], None, -1),
            Key::Right | Key::Enter | Key::Space => {
                let Some(r) = self.menu[last].hover.filter(|r| *r < n) else { return };
                let it = self.menu[last].items[r].clone();
                if !it.sub.is_empty() && it.enabled {
                    self.menu_hover(last, r);
                    if let Some(c) = self.menu.last_mut() {
                        c.hover = c.items.iter().position(|i| i.enabled);
                    }
                } else if let (Some(a), true) = (it.action, it.enabled && !matches!(key, Key::Right)) {
                    out.push(a);
                    self.menu.clear();
                }
            }
            Key::Left => {
                if last > 0 {
                    self.menu.pop();
                } else {
                    self.menu.clear();
                }
            }
            _ => {}
        }
    }

    /// Test helper: step to the next speed from `current`, clamped to the list.
    pub fn next_speed(current: f32, dir: i8) -> f32 {
        let i = SPEEDS.iter().position(|s| (s - current).abs() < 0.001).unwrap_or(3) as i32 + dir as i32;
        SPEEDS[i.clamp(0, SPEEDS.len() as i32 - 1) as usize]
    }
}

impl Ui {
    /// The rows of every open menu panel, outermost first: `(label, rectangle, enabled)`. For tests and tooling.
    pub fn menu_rows(&self) -> Vec<(String, RectF, bool)> {
        let mut v = Vec::new();
        for p in &self.menu {
            for (it, r) in p.items.iter().zip(&p.rows) {
                v.push((it.label.clone(), *r, it.enabled));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MediaState;

    fn media() -> UiModel {
        UiModel {
            state: MediaState::Paused,
            title: "fixture.webm".into(),
            duration_us: Some(60_000_000),
            volume: 1.0,
            rate: 1.0,
            ..UiModel::default()
        }
    }

    fn click(ui: &mut Ui, x: f32, y: f32, m: &UiModel, t: i64) -> Vec<Action> {
        let mut out = ui.handle(&InputEvent::PointerMove { x, y }, t, m);
        out.extend(ui.handle(
            &InputEvent::PointerDown { x, y, button: PointerButton::Primary },
            t + 1_000,
            m,
        ));
        out.extend(ui.handle(&InputEvent::PointerUp { x, y, button: PointerButton::Primary }, t + 2_000, m));
        out
    }

    #[test]
    fn seek_bar_click_seeks_to_the_clicked_fraction() {
        let mut ui = Ui::default();
        let m = media();
        let l = ui.layout(&m);
        let x = l.seek_track.x + l.seek_track.w * 0.5;
        let out = click(&mut ui, x, l.seek_hit.cy(), &m, 1_000_000);
        let f = out
            .iter()
            .filter_map(|a| if let Action::SeekFraction(f) = a { Some(*f) } else { None })
            .next_back()
            .unwrap();
        assert!((f - 0.5).abs() < 0.01, "{f}");
    }

    #[test]
    fn buttons_map_to_actions() {
        let mut ui = Ui::default();
        let m = media();
        let l = ui.layout(&m);
        let r = l.rect_of(Btn::Play).unwrap();
        assert_eq!(click(&mut ui, r.cx(), r.cy(), &m, 1_000_000), [Action::PlayPause]);
        let r = l.rect_of(Btn::Fullscreen).unwrap();
        assert_eq!(click(&mut ui, r.cx(), r.cy(), &m, 2_000_000), [Action::ToggleFullscreen]);
        let r = l.rect_of(Btn::Speed).unwrap();
        assert!(click(&mut ui, r.cx(), r.cy(), &m, 3_000_000).is_empty());
        assert!(ui.menu_open()); // the speed popup
    }

    #[test]
    fn picture_click_toggles_and_double_click_goes_fullscreen() {
        let mut ui = Ui::default();
        let m = media();
        assert_eq!(click(&mut ui, 400.0, 300.0, &m, 1_000_000), [Action::PlayPause]);
        let out = click(&mut ui, 400.0, 300.0, &m, 1_100_000);
        assert_eq!(out, [Action::PlayPause, Action::ToggleFullscreen]);
    }

    #[test]
    fn right_click_opens_the_menu_and_a_row_click_runs_it() {
        let mut ui = Ui::default();
        let m = media();
        ui.handle(
            &InputEvent::PointerDown { x: 300.0, y: 200.0, button: PointerButton::Secondary },
            1_000,
            &m,
        );
        assert!(ui.menu_open());
        let row = ui.menu[0].rows[0]; // Open file...
        let out = click(&mut ui, row.cx(), row.cy(), &m, 2_000);
        assert_eq!(out, [Action::OpenFile]);
        assert!(!ui.menu_open());
    }

    #[test]
    fn menu_is_keyboard_navigable() {
        let mut ui = Ui::default();
        let m = media();
        let key = |ui: &mut Ui, k: Key| {
            ui.handle(&InputEvent::KeyDown { key: k, mods: Modifiers::default(), repeat: false }, 1, &m)
        };
        ui.handle(
            &InputEvent::KeyDown {
                key: Key::Other("ContextMenu".into()),
                mods: Modifiers::default(),
                repeat: false,
            },
            1,
            &m,
        );
        assert!(ui.menu_open());
        key(&mut ui, Key::Down); // Play/Pause
        let out = key(&mut ui, Key::Enter);
        assert_eq!(out, [Action::PlayPause]);
        assert!(!ui.menu_open());
        // Submenus open with Right and close with Left.
        ui.handle(
            &InputEvent::KeyDown {
                key: Key::Other("ContextMenu".into()),
                mods: Modifiers::default(),
                repeat: false,
            },
            2,
            &m,
        );
        for _ in 0..2 {
            key(&mut ui, Key::Down); // Play/Pause, Seek
        }
        key(&mut ui, Key::Right);
        assert_eq!(ui.menu.len(), 2);
        key(&mut ui, Key::Left);
        assert_eq!(ui.menu.len(), 1);
        key(&mut ui, Key::Escape);
        assert!(!ui.menu_open());
    }

    #[test]
    fn controls_auto_hide_while_playing_and_return_on_motion() {
        let mut ui = Ui::new(UiConfig { reduce_motion: true });
        let mut m = media();
        m.state = MediaState::Playing;
        ui.update(0, &m);
        ui.handle(&InputEvent::PointerMove { x: 400.0, y: 300.0 }, 0, &m);
        assert!(ui.update(1_000_000, &m) || ui.controls_visible());
        assert!(ui.controls_visible());
        ui.update(HIDE_AFTER_US + 100_000, &m);
        assert!(!ui.controls_visible());
        assert_eq!(ui.cursor(&m), Cursor::Hidden);
        ui.handle(&InputEvent::PointerMove { x: 420.0, y: 310.0 }, HIDE_AFTER_US + 200_000, &m);
        ui.update(HIDE_AFTER_US + 200_000, &m);
        assert!(ui.controls_visible());
        // Paused: always visible.
        m.state = MediaState::Paused;
        ui.update(HIDE_AFTER_US * 4, &m);
        assert!(ui.controls_visible());
    }

    #[test]
    fn wheel_changes_volume_and_over_the_bar_seeks() {
        let mut ui = Ui::default();
        let m = media();
        let out = ui.handle(&InputEvent::Wheel { dx: 0.0, dy: -100.0 }, 1, &m);
        assert_eq!(out, [Action::VolumeBy(5), Action::VolumeBy(5)]);
        let l = ui.layout(&m);
        ui.handle(&InputEvent::PointerMove { x: l.seek_track.cx(), y: l.seek_hit.cy() }, 2, &m);
        let out = ui.handle(&InputEvent::Wheel { dx: 0.0, dy: 60.0 }, 3, &m);
        assert_eq!(out, [Action::SeekBy(-5_000)]);
    }

    #[test]
    fn keys_hold_repeat_only_for_steps() {
        let mut ui = Ui::default();
        let m = media();
        let down = |ui: &mut Ui, k: Key, rep: bool| {
            ui.handle(&InputEvent::KeyDown { key: k, mods: Modifiers::default(), repeat: rep }, 1, &m)
        };
        assert_eq!(down(&mut ui, Key::Space, false), [Action::PlayPause]);
        assert!(down(&mut ui, Key::Space, true).is_empty());
        assert_eq!(down(&mut ui, Key::Right, true), [Action::SeekBy(5_000)]);
    }

    #[test]
    fn tab_focus_cycles_and_enter_activates() {
        let mut ui = Ui::default();
        let m = media();
        let tab = |ui: &mut Ui| {
            ui.handle(
                &InputEvent::KeyDown {
                    key: Key::Other("Tab".into()),
                    mods: Modifiers::default(),
                    repeat: false,
                },
                1,
                &m,
            )
        };
        tab(&mut ui);
        assert_eq!(ui.focus, Some(Btn::Play));
        let out = ui.handle(
            &InputEvent::KeyDown { key: Key::Enter, mods: Modifiers::default(), repeat: false },
            2,
            &m,
        );
        assert_eq!(out, [Action::PlayPause]);
    }

    #[test]
    fn idle_screen_offers_open() {
        let mut ui = Ui::default();
        let idle = UiModel::default();
        let l = ui.layout(&idle);
        let r = l.rect_of(Btn::Welcome).unwrap();
        assert_eq!(click(&mut ui, r.cx(), r.cy(), &idle, 1), [Action::OpenFile]);
        assert_eq!(
            ui.handle(
                &InputEvent::KeyDown { key: Key::Enter, mods: Modifiers::default(), repeat: false },
                2,
                &idle
            ),
            [Action::OpenFile]
        );
    }

    #[test]
    fn a_phone_gets_two_rows_of_touch_sized_controls_and_a_tap_shows_them_first() {
        for (w, h, dpr) in [(390u32, 844u32, 1.0f32), (360, 800, 1.0), (1170, 2532, 3.0)] {
            let mut ui = Ui::default();
            ui.set_size(w, h, dpr);
            let m = media();
            let l = ui.layout(&m);
            let (sw, sh) = (w as f32, h as f32);
            assert!(l.bar_top < l.seek_hit.y && l.seek_hit.bottom() < sh, "{w}x{h}");
            for (i, (b, r)) in l.buttons.iter().enumerate() {
                let min = if *b == Btn::Speed { 36.0 } else { 44.0 } * dpr - 0.5;
                assert!(r.w >= min.min(44.0 * dpr - 0.5) && r.h >= min, "{b:?} is {r:?} at {w}x{h}");
                assert!(
                    r.x >= 0.0 && r.right() <= sw && r.y >= l.bar_top && r.bottom() <= sh,
                    "{b:?} is {r:?}"
                );
                for (b2, r2) in &l.buttons[i + 1..] {
                    let apart = r.right() <= r2.x + 0.01
                        || r2.right() <= r.x + 0.01
                        || r.bottom() <= r2.y + 0.01
                        || r2.bottom() <= r.y + 0.01;
                    assert!(apart, "{b:?} overlaps {b2:?} at {w}x{h}");
                }
            }
            for want in [
                Btn::Play,
                Btn::Back,
                Btn::Fwd,
                Btn::Mute,
                Btn::Fullscreen,
                Btn::ModeSwitch,
                Btn::Speed,
                Btn::Tracks,
                Btn::Playlist,
                Btn::Open,
            ] {
                assert!(l.rect_of(want).is_some(), "{want:?} missing at {w}x{h}");
            }
            // The time readout sits above the seek bar, clear of every button.
            assert!(l.time_y < l.seek_hit.y && l.buttons.iter().all(|(_, r)| r.y > l.time_y + 8.0 * dpr));
            // Drawing never panics and every message card fits.
            let mut fb = crate::gfx::FrameBuffer::new(w, h);
            ui.draw_overlay(&mut fb, &m);
            // With the controls hidden, a tap on the picture shows them and does not pause; the next one pauses.
            ui.controls_alpha = 0.0;
            assert_eq!(click(&mut ui, sw * 0.5, sh * 0.3, &m, 5_000_000), []);
            ui.controls_alpha = 1.0;
            assert_eq!(click(&mut ui, sw * 0.5, sh * 0.3, &m, 6_000_000), [Action::PlayPause]);
        }
    }

    #[test]
    fn the_player_bar_has_shuffle_and_repeat_that_act_and_say_their_state() {
        let mut ui = Ui::default();
        let mut m = media();
        let l = ui.layout(&m);
        for (b, a) in [(Btn::Shuffle, Action::ToggleShuffle), (Btn::Repeat, Action::CycleRepeat)] {
            let r = l.rect_of(b).expect("shown at the default size");
            assert_eq!(click(&mut ui, r.cx(), r.cy(), &m, 1_000_000), [a], "{b:?}");
        }
        // Tooltips name the state and the key, from the same text as the menus.
        let tip = |ui: &Ui, b: Btn, m: &UiModel| ui.play_tip_text(Target::Btn(b), m).unwrap();
        assert!(
            tip(&ui, Btn::Shuffle, &m).0.starts_with("Shuffle is off") && tip(&ui, Btn::Shuffle, &m).1 == "Z"
        );
        assert!(
            tip(&ui, Btn::Repeat, &m).0.starts_with("Repeat is off") && tip(&ui, Btn::Repeat, &m).1 == "R"
        );
        m.shuffle = true;
        m.repeat = 1;
        assert!(tip(&ui, Btn::Shuffle, &m).0.starts_with("Shuffle is on"));
        assert!(tip(&ui, Btn::Repeat, &m).0.starts_with("Repeat all"));
        m.repeat = 2;
        assert!(tip(&ui, Btn::Repeat, &m).0.starts_with("Repeat one"));
        let menu = actions::playlist_menu(&m);
        let label = |a: Action| menu.iter().find(|i| i.action == Some(a)).unwrap().label.clone();
        assert_eq!(label(Action::CycleRepeat), m.repeat_label());
        assert_eq!(label(Action::ToggleShuffle), m.shuffle_label());
        // The keys are the same actions the buttons send.
        for (k, a) in [('z', Action::ToggleShuffle), ('r', Action::CycleRepeat)] {
            let out = ui.handle(
                &InputEvent::KeyDown { key: Key::Char(k), mods: Modifiers::default(), repeat: false },
                5_000_000,
                &m,
            );
            assert_eq!(out, [a]);
        }
        // Tab reaches them, between the playlist and the open button, and Enter acts.
        ui.focus = Some(Btn::Playlist);
        ui.cycle_focus(false, &m);
        assert_eq!(ui.focus, Some(Btn::Shuffle));
        ui.cycle_focus(false, &m);
        assert_eq!(ui.focus, Some(Btn::Repeat));
        let out = ui.handle(
            &InputEvent::KeyDown { key: Key::Enter, mods: Modifiers::default(), repeat: false },
            6_000_000,
            &m,
        );
        assert_eq!(out, [Action::CycleRepeat]);
    }

    #[test]
    fn the_repeat_modes_have_three_different_icons() {
        use crate::icon::Icon;
        let all: Vec<Icon> = (0..3).map(Icon::for_repeat).collect();
        assert_eq!(all, [Icon::RepeatOff, Icon::Repeat, Icon::Repeat1]);
        let mut fb = crate::gfx::FrameBuffer::new(24, 24);
        let mut cache = crate::icon::IconCache::new();
        let mut shots = Vec::new();
        for ic in all {
            fb.clear(theme::Rgba::new(0, 0, 0, 255));
            cache.draw(&mut fb, ic, 12.0, 12.0, 24.0, theme::Rgba::new(255, 255, 255, 255), 1.0, false);
            assert!(fb.pixels.iter().any(|&b| b > 0 && b < 255), "{ic:?} draws");
            shots.push(fb.pixels.clone());
        }
        assert!(shots[0] != shots[1] && shots[1] != shots[2] && shots[0] != shots[2]);
    }

    #[test]
    fn shuffle_and_repeat_give_way_when_the_window_narrows_and_nothing_overlaps() {
        let m = media();
        for dpr in [1.0f32, 2.0] {
            for w in (480..=2400).step_by(40) {
                let mut ui = Ui::default();
                ui.set_size((w as f32 * dpr) as u32, (720.0 * dpr) as u32, dpr);
                let l = ui.layout(&m);
                let shown = l.rect_of(Btn::Shuffle).is_some();
                assert_eq!(shown, l.rect_of(Btn::Repeat).is_some());
                assert_eq!(shown, w as f32 >= SHUFFLE_REPEAT_MIN_W, "{w} @ {dpr}");
                // Every button lies inside the window and none covers another or the time readout.
                for (i, (b, r)) in l.buttons.iter().enumerate() {
                    assert!(r.x >= 0.0 && r.right() <= l.w, "{b:?} at {w}");
                    for (b2, r2) in &l.buttons[i + 1..] {
                        let apart = r.right() <= r2.x + 0.01
                            || r2.right() <= r.x + 0.01
                            || r.bottom() <= r2.y + 0.01
                            || r2.bottom() <= r.y + 0.01;
                        assert!(apart, "{b:?} overlaps {b2:?} at {w} @ {dpr}");
                    }
                }
                if shown {
                    // Room for "59:59 / 59:59" (13 characters of 13 px mono) before the first button of the right group.
                    let first_right = l.rect_of(Btn::Speed).unwrap().x; // the right group's leftmost control
                    assert!(
                        first_right - l.time_x >= 13.0 * 7.9 * dpr,
                        "{w} @ {dpr}: {}",
                        first_right - l.time_x
                    );
                }
            }
        }
        // Without them Tab skips them.
        let mut ui = Ui::default();
        ui.set_size(640, 360, 1.0);
        ui.focus = Some(Btn::Playlist);
        ui.cycle_focus(false, &m);
        assert_eq!(ui.focus, Some(Btn::Open));
    }

    #[test]
    fn video_rect_letterboxes() {
        let ui = Ui::default(); // 1280 x 720
        let r = ui.video_rect(320, 240);
        assert_eq!((r.w, r.h, r.x), (960.0, 720.0, 160.0));
        assert_eq!(Ui::next_speed(1.0, 1), 1.25);
        assert_eq!(Ui::next_speed(4.0, 1), 4.0);
    }
}
