//! The Audio settings panel: crossfade (on or off, length 2 to 10 s) and the automatic level (on or off, target -23 to -10 LUFS, by
//! track or by album). A modal card over either face of the app, drawn with the same tokens as the rest.
//!
//! Everything here is an [`Action`]: the panel never changes a setting itself, it asks, and the application answers with a new
//! [`UiModel`] (so the panel always shows what is really set). Mouse: click a switch or the segmented control, drag a slider (it moves
//! in whole steps), the wheel over a slider steps it, a click outside closes. Keyboard: Tab and Shift+Tab (or Up and Down) move
//! between the controls, Space or Enter flip a switch, Left and Right step a slider or the segmented control (Home and End jump to
//! its ends), Escape closes.
use crate::actions::Action;
use crate::font::Face;
use crate::gfx::{FrameBuffer, Paint, RectF, fade};
use crate::icon::Icon;
use crate::model::UiModel;
use crate::ui::Ui;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use rvp_core::settings::{
    CROSSFADE_MAX_SECS, CROSSFADE_MIN_SECS, LevelMode, TARGET_MAX_LUFS, TARGET_MIN_LUFS,
};
use rvp_host::{InputEvent, Key, Modifiers, PointerButton};
use theme::tokens as t;

/// One control of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioControl {
    /// The crossfade switch.
    Crossfade,
    /// The crossfade length slider.
    CrossfadeLength,
    /// The automatic level switch.
    AutoLevel,
    /// The target level slider.
    Target,
    /// Track or album.
    Mode,
    /// The button that closes the panel.
    Done,
}

/// The controls in tab order.
const ORDER: [AudioControl; 6] = [
    AudioControl::Crossfade,
    AudioControl::CrossfadeLength,
    AudioControl::AutoLevel,
    AudioControl::Target,
    AudioControl::Mode,
    AudioControl::Done,
];

impl AudioControl {
    /// A stable name, for the snapshot and tests.
    pub fn name(self) -> &'static str {
        match self {
            AudioControl::Crossfade => "crossfade",
            AudioControl::CrossfadeLength => "crossfade_length",
            AudioControl::AutoLevel => "auto_level",
            AudioControl::Target => "target",
            AudioControl::Mode => "mode",
            AudioControl::Done => "done",
        }
    }

    fn is_slider(self) -> bool {
        matches!(self, AudioControl::CrossfadeLength | AudioControl::Target)
    }
}

/// The state of an open panel.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AudioPanel {
    /// Index into [`ORDER`] of the control the keyboard is on.
    pub focus: usize,
    /// The control under the pointer.
    pub hover: Option<AudioControl>,
    /// The slider being dragged.
    pub drag: Option<AudioControl>,
}

/// Where everything is, in pixels.
#[derive(Debug, Clone)]
pub struct AudioPanelGeom {
    /// The card.
    pub card: RectF,
    /// The close button in the corner.
    pub close: RectF,
    /// Every control: the area that reacts to the pointer, and the drawn part of it (a switch, a slider track, the segments).
    pub controls: Vec<(AudioControl, RectF, RectF)>,
    /// Rows: `(label, description or value)` text positions are derived from these tops.
    pub rows: [f32; 5],
    /// The rule between the two groups.
    pub rule_y: f32,
    /// The footer line's centre.
    pub footer_cy: f32,
}

impl AudioPanelGeom {
    fn control(&self, c: AudioControl) -> (RectF, RectF) {
        let none = RectF::new(0.0, 0.0, 0.0, 0.0);
        self.controls.iter().find(|x| x.0 == c).map_or((none, none), |x| (x.1, x.2))
    }
}

/// Step `v` within `min..=max` by `d` and clamp.
fn step_i32(v: i32, d: i32, min: i32, max: i32) -> i32 {
    (v + d).clamp(min, max)
}

impl Ui {
    /// Show the Audio settings panel.
    pub fn open_audio_settings(&mut self) {
        self.menu.clear();
        self.audio_panel = Some(AudioPanel { focus: 0, hover: None, drag: None });
        self.dirty = true;
    }

    /// Close the panel.
    pub fn close_audio_settings(&mut self) {
        if self.audio_panel.take().is_some() {
            self.dirty = true;
        }
    }

    /// True while the panel is up.
    pub fn audio_settings_open(&self) -> bool {
        self.audio_panel.is_some()
    }

    /// The control the keyboard is on, while the panel is up.
    pub fn audio_panel_focus(&self) -> Option<AudioControl> {
        self.audio_panel.as_ref().map(|p| ORDER[p.focus])
    }

    /// The geometry of the panel for the window as it is now.
    pub fn audio_panel_geom(&self) -> AudioPanelGeom {
        let s = self.scale;
        let (w, h) = (self.w as f32, self.h as f32);
        let pad = 28.0 * s;
        let cw = (480.0f32).min(w / s - 32.0).max(280.0) * s;
        let ch = 520.0 * s;
        let card = RectF::new(((w - cw) * 0.5).max(0.0), ((h - ch) * 0.5).max(8.0 * s), cw, ch);
        let close =
            RectF::new(card.right() - pad - 32.0 * s + 8.0 * s, card.y + 18.0 * s, 36.0 * s, 36.0 * s);
        let x0 = card.x + pad;
        let inner = card.w - 2.0 * pad;
        // Row tops.
        let r_cross = card.y + 76.0 * s;
        let r_len = r_cross + 66.0 * s;
        let rule_y = r_len + 74.0 * s;
        let r_auto = rule_y + 12.0 * s;
        let r_target = r_auto + 66.0 * s;
        let r_mode = r_target + 74.0 * s;
        let footer_top = r_mode + 84.0 * s;
        let switch = |row: f32| RectF::new(x0 + inner - 48.0 * s, row + 18.0 * s, 48.0 * s, 28.0 * s);
        let slider = |row: f32| RectF::new(x0, row + 46.0 * s, inner, 6.0 * s);
        let seg = RectF::new(x0 + inner - 176.0 * s, r_mode + 14.0 * s, 176.0 * s, 38.0 * s);
        let done = RectF::new(x0 + inner - 112.0 * s, footer_top, 112.0 * s, 42.0 * s);
        let hit_row = |row: f32, h: f32| RectF::new(card.x + 8.0 * s, row, card.w - 16.0 * s, h);
        let slider_hit =
            |tr: RectF| RectF::new(tr.x - 12.0 * s, tr.cy() - 16.0 * s, tr.w + 24.0 * s, 32.0 * s);
        let controls = alloc::vec![
            (AudioControl::Crossfade, hit_row(r_cross, 62.0 * s), switch(r_cross)),
            (AudioControl::CrossfadeLength, slider_hit(slider(r_len)), slider(r_len)),
            (AudioControl::AutoLevel, hit_row(r_auto, 62.0 * s), switch(r_auto)),
            (AudioControl::Target, slider_hit(slider(r_target)), slider(r_target)),
            (AudioControl::Mode, seg, seg),
            (AudioControl::Done, done, done),
        ];
        AudioPanelGeom {
            card,
            close,
            controls,
            rows: [r_cross, r_len, r_auto, r_target, r_mode],
            rule_y,
            footer_cy: done.cy(),
        }
    }

    /// The control under a point.
    fn audio_hit(&self, g: &AudioPanelGeom, x: f32, y: f32) -> Option<AudioControl> {
        // Sliders and buttons win over the rows around them.
        for (c, hit, _) in g.controls.iter().rev() {
            if hit.contains(x, y) {
                return Some(*c);
            }
        }
        None
    }

    /// The value of a slider for a pointer at `x`.
    fn slider_value(c: AudioControl, g: &AudioPanelGeom, x: f32) -> i32 {
        let (_, tr) = g.control(c);
        let f = ((x - tr.x) / tr.w.max(1.0)).clamp(0.0, 1.0);
        let (min, max) = Self::slider_range(c);
        min + libm::roundf(f * (max - min) as f32) as i32
    }

    fn slider_range(c: AudioControl) -> (i32, i32) {
        match c {
            AudioControl::CrossfadeLength => (CROSSFADE_MIN_SECS as i32, CROSSFADE_MAX_SECS as i32),
            _ => (TARGET_MIN_LUFS as i32, TARGET_MAX_LUFS as i32),
        }
    }

    fn slider_current(c: AudioControl, model: &UiModel) -> i32 {
        match c {
            AudioControl::CrossfadeLength => model.audio.crossfade_secs as i32,
            _ => model.audio.target_lufs as i32,
        }
    }

    fn slider_action(c: AudioControl, v: i32) -> Action {
        match c {
            AudioControl::CrossfadeLength => Action::SetCrossfadeSecs(v as u8),
            _ => Action::SetTargetLufs(v as i8),
        }
    }

    /// What pressing a control does.
    fn audio_activate(&mut self, c: AudioControl, model: &UiModel, out: &mut Vec<Action>) {
        match c {
            AudioControl::Crossfade => out.push(Action::SetCrossfade(!model.audio.crossfade)),
            AudioControl::AutoLevel => out.push(Action::SetAutoLevel(!model.audio.auto_level)),
            AudioControl::Mode => out.push(Action::SetLevelMode(match model.audio.level_mode {
                LevelMode::Track => LevelMode::Album,
                LevelMode::Album => LevelMode::Track,
            })),
            AudioControl::Done => self.close_audio_settings(),
            AudioControl::CrossfadeLength | AudioControl::Target => {}
        }
    }

    /// One input event while the panel is up. Everything is taken by the panel.
    pub(crate) fn audio_panel_event(&mut self, ev: &InputEvent, now_us: i64, model: &UiModel) -> Vec<Action> {
        self.now = now_us;
        self.dirty = true;
        let mut out = Vec::new();
        let g = self.audio_panel_geom();
        match ev {
            InputEvent::Resize { w, h, dpr } => self.set_size(*w, *h, *dpr),
            InputEvent::Focus(f) => {
                self.win_focused = *f;
                if !*f {
                    if let Some(p) = &mut self.audio_panel {
                        p.drag = None;
                    }
                }
            }
            InputEvent::PointerMove { x, y } => {
                self.pointer = Some((*x, *y));
                self.keyboard_mode = false;
                let drag = self.audio_panel.as_ref().and_then(|p| p.drag);
                if let Some(c) = drag {
                    let v = Self::slider_value(c, &g, *x);
                    if v != Self::slider_current(c, model) {
                        out.push(Self::slider_action(c, v));
                    }
                }
                let hit = self.audio_hit(&g, *x, *y);
                if let Some(p) = &mut self.audio_panel {
                    p.hover = hit;
                }
            }
            InputEvent::PointerDown { x, y, button } => {
                self.pointer = Some((*x, *y));
                self.keyboard_mode = false;
                if *button != PointerButton::Primary {
                    return out;
                }
                if g.close.contains(*x, *y) {
                    self.close_audio_settings();
                    return out;
                }
                if !g.card.contains(*x, *y) {
                    // A click outside the card only closes it.
                    self.close_audio_settings();
                    return out;
                }
                let Some(c) = self.audio_hit(&g, *x, *y) else { return out };
                if let Some(i) = ORDER.iter().position(|o| *o == c) {
                    if let Some(p) = &mut self.audio_panel {
                        p.focus = i;
                    }
                }
                if c.is_slider() {
                    if let Some(p) = &mut self.audio_panel {
                        p.drag = Some(c);
                    }
                    let v = Self::slider_value(c, &g, *x);
                    if v != Self::slider_current(c, model) {
                        out.push(Self::slider_action(c, v));
                    }
                } else if c == AudioControl::Mode {
                    // The two halves are Track and Album.
                    let (_, seg) = g.control(AudioControl::Mode);
                    let want = if *x < seg.cx() { LevelMode::Track } else { LevelMode::Album };
                    if want != model.audio.level_mode {
                        out.push(Action::SetLevelMode(want));
                    }
                } else {
                    self.audio_activate(c, model, &mut out);
                }
            }
            InputEvent::PointerUp { .. } => {
                if let Some(p) = &mut self.audio_panel {
                    p.drag = None;
                }
            }
            InputEvent::Wheel { dy, .. } => {
                let hover = self.audio_panel.as_ref().and_then(|p| p.hover);
                if let Some(c) = hover.filter(|c| c.is_slider()) {
                    let (min, max) = Self::slider_range(c);
                    let cur = Self::slider_current(c, model);
                    let v = step_i32(cur, if *dy < 0.0 { 1 } else { -1 }, min, max);
                    if v != cur {
                        out.push(Self::slider_action(c, v));
                    }
                }
            }
            InputEvent::KeyDown { key, mods, .. } => {
                self.last_activity = now_us;
                self.keyboard_mode = true;
                self.audio_panel_key(key, mods, model, &mut out);
                self.key_used = true;
            }
            _ => {}
        }
        out
    }

    fn audio_panel_key(&mut self, key: &Key, mods: &Modifiers, model: &UiModel, out: &mut Vec<Action>) {
        let Some(p) = &mut self.audio_panel else { return };
        let n = ORDER.len();
        let cur = ORDER[p.focus];
        match key {
            Key::Escape => self.close_audio_settings(),
            Key::Other(name) if name == "Tab" => {
                p.focus = if mods.shift { (p.focus + n - 1) % n } else { (p.focus + 1) % n };
            }
            Key::Down => p.focus = (p.focus + 1) % n,
            Key::Up => p.focus = (p.focus + n - 1) % n,
            Key::Space | Key::Enter => {
                let c = cur;
                self.audio_activate(c, model, out);
            }
            Key::Left | Key::Right | Key::Home | Key::End => {
                let dir = match key {
                    Key::Left => -1,
                    Key::Right => 1,
                    _ => 0,
                };
                if cur.is_slider() {
                    let (min, max) = Self::slider_range(cur);
                    let now = Self::slider_current(cur, model);
                    let v = match key {
                        Key::Home => min,
                        Key::End => max,
                        _ => step_i32(now, dir, min, max),
                    };
                    if v != now {
                        out.push(Self::slider_action(cur, v));
                    }
                } else if cur == AudioControl::Mode && dir != 0 {
                    let want = if dir < 0 { LevelMode::Track } else { LevelMode::Album };
                    if want != model.audio.level_mode {
                        out.push(Action::SetLevelMode(want));
                    }
                } else if matches!(cur, AudioControl::Crossfade | AudioControl::AutoLevel) && dir != 0 {
                    // A switch: Right is on, Left is off.
                    let on = dir > 0;
                    let (is_on, action) = if cur == AudioControl::Crossfade {
                        (model.audio.crossfade, Action::SetCrossfade(on))
                    } else {
                        (model.audio.auto_level, Action::SetAutoLevel(on))
                    };
                    if on != is_on {
                        out.push(action);
                    }
                }
            }
            _ => {}
        }
    }

    // ---- drawing --------------------------------------------------------------------------------------------------------

    /// Draw the panel over everything, when it is up.
    pub(crate) fn draw_audio_panel(&mut self, fb: &mut FrameBuffer, model: &UiModel) {
        let Some(panel) = self.audio_panel.clone() else { return };
        let s = self.scale;
        let g = self.audio_panel_geom();
        let card = g.card;
        fb.fill_rect_paint(RectF::new(0.0, 0.0, self.w as f32, self.h as f32), Paint::Solid(t::INK_900), 0.6);
        fb.shadow_rrect(card, 22.0 * s, 20.0 * s, 56.0 * s, theme::Rgba::new(5, 2, 15, 190), 1.0);
        fb.fill_rrect(card, 22.0 * s, Paint::Solid(t::INK_800), 1.0);
        fb.stroke_rrect(card, 22.0 * s, 1.0 * s, t::BORDER_SUBTLE, 1.0);
        fb.fill_rrect(
            RectF::new(card.x + 40.0 * s, card.y + 1.0 * s, card.w - 80.0 * s, 2.0 * s),
            1.0 * s,
            Paint::GradientFaded(&t::GRADIENT_TEARS, 0.9),
            1.0,
        );
        let pad = 28.0 * s;
        let x0 = card.x + pad;
        let inner = card.w - 2.0 * pad;
        self.text(fb, Face::SansBold, 20.0, x0, card.y + 36.0 * s, "Audio", t::TEXT_STRONG, 1.0, -0.2);
        // The close button.
        let hot_close = panel.hover.is_none() && self.pointer.is_some_and(|(x, y)| g.close.contains(x, y));
        if hot_close {
            fb.fill_rrect(g.close, g.close.h * 0.5, Paint::Solid(fade(t::WHITE, 0.10)), 1.0);
        }
        self.icon(fb, Icon::X, g.close.cx(), g.close.cy(), 18.0, t::TEXT_MUTED, 1.0, false);

        let a = model.audio;
        let focus = if self.keyboard_mode { Some(ORDER[panel.focus]) } else { None };
        let hot = |c: AudioControl| panel.hover == Some(c) || panel.drag == Some(c);
        let row_text = |ui: &mut Ui, fb: &mut FrameBuffer, y: f32, label: &str, desc: &str, dim: bool| {
            let col = if dim { t::TEXT_MUTED } else { t::TEXT_STRONG };
            ui.text(fb, Face::SansMedium, 15.0, x0, y + 30.0 * s, label, col, 1.0, 0.0);
            let room = inner - 64.0 * s;
            let shown = ui.fonts.fit(Face::Sans, 12.0 * s, desc, room);
            ui.text(fb, Face::Sans, 12.0, x0, y + 50.0 * s, &shown, t::TEXT_DIM, 1.0, 0.0);
        };

        // Crossfade.
        row_text(self, fb, g.rows[0], "Crossfade", "Blend the end of a song into the next one", false);
        self.draw_switch(fb, g.control(AudioControl::Crossfade).1, a.crossfade, hot(AudioControl::Crossfade));
        // Its length.
        let (_, tr) = g.control(AudioControl::CrossfadeLength);
        self.draw_slider_row(
            fb,
            g.rows[1],
            x0,
            inner,
            "Length",
            &format!("{} s", a.crossfade_secs),
            tr,
            (a.crossfade_secs as i32 - CROSSFADE_MIN_SECS as i32) as f32
                / (CROSSFADE_MAX_SECS - CROSSFADE_MIN_SECS) as f32,
            !a.crossfade,
            hot(AudioControl::CrossfadeLength),
        );
        // The rule between the groups.
        fb.fill_rect_paint(RectF::new(x0, g.rule_y, inner, 1.0 * s), Paint::Solid(t::BORDER_SUBTLE), 1.0);
        // Automatic level.
        row_text(self, fb, g.rows[2], "Auto-level", "Play every song at the same loudness", false);
        self.draw_switch(
            fb,
            g.control(AudioControl::AutoLevel).1,
            a.auto_level,
            hot(AudioControl::AutoLevel),
        );
        let (_, tr) = g.control(AudioControl::Target);
        self.draw_slider_row(
            fb,
            g.rows[3],
            x0,
            inner,
            "Target",
            &format!("{} LUFS", a.target_lufs),
            tr,
            (a.target_lufs as i32 - TARGET_MIN_LUFS as i32) as f32
                / (TARGET_MAX_LUFS - TARGET_MIN_LUFS) as f32,
            !a.auto_level,
            hot(AudioControl::Target),
        );
        // Track or album.
        let dim = !a.auto_level;
        row_text(
            self,
            fb,
            g.rows[4],
            "Level by",
            if a.level_mode == LevelMode::Album {
                "Albums keep their own balance"
            } else {
                "Every song on its own"
            },
            dim,
        );
        let seg = g.control(AudioControl::Mode).1;
        self.draw_segments(fb, seg, a.level_mode, hot(AudioControl::Mode), dim);
        // The footer: what the level is doing now, and the button that closes.
        let status: String = match (a.auto_level, model.level_gain_db) {
            (true, Some(db)) => format!("Now {:+.1} dB", db),
            (true, None) => String::from("Measured as songs play"),
            (false, _) => String::from("Songs play at their own level"),
        };
        let room = inner - 128.0 * s;
        let shown = self.fonts.fit(Face::Mono, 12.0 * s, &status, room);
        self.text(fb, Face::Mono, 12.0, x0, g.footer_cy, &shown, t::TEXT_DIM, 1.0, 0.0);
        let done = g.control(AudioControl::Done).1;
        let hot_done = hot(AudioControl::Done);
        let d = done;
        if hot_done || focus == Some(AudioControl::Done) {
            fb.glow_rrect(d, d.h * 0.5, 20.0 * s, t::MAGENTA_500, 0.5);
        }
        fb.fill_rrect(d, d.h * 0.5, Paint::Gradient(&t::GRADIENT_TEARS), 1.0);
        if hot_done {
            fb.fill_rrect(d, d.h * 0.5, Paint::Solid(t::WHITE), 0.12);
        }
        let tw = self.text_w(Face::SansBold, 14.0, "Done", 0.0);
        self.text(fb, Face::SansBold, 14.0, d.cx() - tw * 0.5, d.cy(), "Done", t::WHITE, 1.0, 0.0);
        // The focus ring of the keyboard.
        if let Some(c) = focus {
            let (hit, drawn) = g.control(c);
            let r = match c {
                AudioControl::Crossfade | AudioControl::AutoLevel => drawn,
                AudioControl::CrossfadeLength | AudioControl::Target => hit,
                AudioControl::Mode | AudioControl::Done => drawn,
            };
            let radius = match c {
                AudioControl::CrossfadeLength | AudioControl::Target => 12.0 * s,
                _ => r.h * 0.5,
            };
            self.focus_ring(fb, r, radius, 1.0);
        }
    }

    pub(crate) fn draw_switch(&mut self, fb: &mut FrameBuffer, r: RectF, on: bool, hot: bool) {
        let s = self.scale;
        let rad = r.h * 0.5;
        if on {
            fb.fill_rrect(r, rad, Paint::Horizontal(t::MAGENTA_500, t::VIOLET_400), 1.0);
        } else {
            fb.fill_rrect(r, rad, Paint::Solid(fade(t::WHITE, if hot { 0.24 } else { 0.16 })), 1.0);
            fb.stroke_rrect(r, rad, 1.0 * s, fade(t::WHITE, 0.22), 1.0);
        }
        let k = r.h - 6.0 * s;
        let kx = if on { r.right() - 3.0 * s - k } else { r.x + 3.0 * s };
        fb.shadow_rrect(
            RectF::new(kx, r.y + 3.0 * s, k, k),
            k * 0.5,
            1.0 * s,
            4.0 * s,
            theme::Rgba::new(5, 2, 15, 140),
            1.0,
        );
        fb.fill_rrect(RectF::new(kx, r.y + 3.0 * s, k, k), k * 0.5, Paint::Solid(t::WHITE), 1.0);
    }

    fn draw_slider_row(
        &mut self,
        fb: &mut FrameBuffer,
        row: f32,
        x0: f32,
        inner: f32,
        label: &str,
        value: &str,
        track: RectF,
        frac: f32,
        dim: bool,
        hot: bool,
    ) {
        let s = self.scale;
        let col = if dim { t::TEXT_MUTED } else { t::TEXT_STRONG };
        self.text(fb, Face::SansMedium, 15.0, x0, row + 20.0 * s, label, col, 1.0, 0.0);
        let vw = self.text_w(Face::Mono, 13.0, value, 0.0);
        let vcol = if dim { t::TEXT_DIM } else { t::CYAN_400 };
        self.text(fb, Face::Mono, 13.0, x0 + inner - vw, row + 20.0 * s, value, vcol, 1.0, 0.0);
        let a = if dim { 0.55 } else { 1.0 };
        fb.fill_rrect(track, track.h * 0.5, Paint::Solid(fade(t::WHITE, 0.22)), a);
        let f = frac.clamp(0.0, 1.0);
        let fill = RectF::new(track.x, track.y, (track.w * f).max(track.h), track.h);
        fb.fill_rrect(fill, track.h * 0.5, Paint::Horizontal(t::MAGENTA_500, t::VIOLET_400), a);
        let kr = if hot { 10.0 * s } else { 8.0 * s };
        let kx = track.x + track.w * f;
        let k = RectF::new(kx - kr, track.cy() - kr, kr * 2.0, kr * 2.0);
        if hot {
            fb.glow_rrect(k, kr, 12.0 * s, t::CYAN_500, 0.5 * a);
        }
        fb.fill_rrect(k, kr, Paint::Solid(t::TEXT_STRONG), a);
    }

    fn draw_segments(&mut self, fb: &mut FrameBuffer, r: RectF, mode: LevelMode, hot: bool, dim: bool) {
        let s = self.scale;
        let a = if dim { 0.55 } else { 1.0 };
        fb.fill_rrect(r, r.h * 0.5, Paint::Solid(t::INK_900), a);
        fb.stroke_rrect(
            r,
            r.h * 0.5,
            1.0 * s,
            if hot { fade(t::CYAN_500, 0.7) } else { fade(t::WHITE, 0.22) },
            a,
        );
        let half = r.w * 0.5;
        for (i, (label, me)) in
            [("Track", LevelMode::Track), ("Album", LevelMode::Album)].into_iter().enumerate()
        {
            let seg =
                RectF::new(r.x + half * i as f32 + 3.0 * s, r.y + 3.0 * s, half - 6.0 * s, r.h - 6.0 * s);
            let on = mode == me;
            if on {
                fb.fill_rrect(seg, seg.h * 0.5, Paint::Gradient(&t::GRADIENT_TEARS), a);
            }
            let tw = self.text_w(Face::SansMedium, 14.0, label, 0.0);
            self.text(
                fb,
                Face::SansMedium,
                14.0,
                seg.cx() - tw * 0.5,
                seg.cy(),
                label,
                if on { t::WHITE } else { t::TEXT_MUTED },
                a,
                0.0,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MediaState;
    use crate::ui::UiConfig;
    use alloc::vec;
    use rvp_core::AudioSettings;

    fn ui() -> Ui {
        let mut ui = Ui::new(UiConfig::default());
        ui.set_size(1280, 720, 1.0);
        ui
    }

    fn model(a: AudioSettings) -> UiModel {
        UiModel { state: MediaState::Paused, volume: 1.0, rate: 1.0, audio: a, ..UiModel::default() }
    }

    fn click(ui: &mut Ui, m: &UiModel, x: f32, y: f32) -> Vec<Action> {
        let mut out = ui.handle(&InputEvent::PointerMove { x, y }, 0, m);
        out.extend(ui.handle(&InputEvent::PointerDown { x, y, button: PointerButton::Primary }, 1, m));
        out.extend(ui.handle(&InputEvent::PointerUp { x, y, button: PointerButton::Primary }, 2, m));
        out
    }

    fn key(ui: &mut Ui, m: &UiModel, k: Key, shift: bool) -> Vec<Action> {
        ui.handle(
            &InputEvent::KeyDown { key: k, mods: Modifiers { shift, ..Modifiers::default() }, repeat: false },
            3,
            m,
        )
    }

    #[test]
    fn the_panel_opens_closes_and_is_drawn_inside_the_window() {
        let mut ui = ui();
        let m = model(AudioSettings::default());
        assert!(!ui.audio_settings_open());
        ui.open_audio_settings();
        assert!(ui.audio_settings_open());
        let g = ui.audio_panel_geom();
        assert!(g.card.x >= 0.0 && g.card.right() <= 1280.0 && g.card.y >= 0.0 && g.card.bottom() <= 720.0);
        for (c, hit, drawn) in &g.controls {
            assert!(g.card.contains(hit.cx(), hit.cy()) && g.card.contains(drawn.cx(), drawn.cy()), "{c:?}");
        }
        let mut fb = FrameBuffer::new(1280, 720);
        ui.draw_audio_panel(&mut fb, &m);
        assert_eq!(key(&mut ui, &m, Key::Escape, false), vec![]);
        assert!(!ui.audio_settings_open());
        // Narrow windows still fit it.
        ui.set_size(360, 800, 2.0);
        ui.open_audio_settings();
        let g = ui.audio_panel_geom();
        assert!(g.card.x >= 0.0 && g.card.right() <= 720.0, "{:?}", g.card);
    }

    #[test]
    fn the_mouse_flips_switches_drags_sliders_and_picks_the_mode() {
        let mut ui = ui();
        ui.open_audio_settings();
        let a = AudioSettings::default();
        let m = model(a);
        let g = ui.audio_panel_geom();
        // The crossfade switch, anywhere on its row.
        let (hit, _) = (g.controls[0].1, g.controls[0].2);
        assert_eq!(click(&mut ui, &m, hit.x + 40.0, hit.cy()), vec![Action::SetCrossfade(true)]);
        let (_, sw) = (g.controls[2].1, g.controls[2].2);
        assert_eq!(click(&mut ui, &m, sw.cx(), sw.cy()), vec![Action::SetAutoLevel(true)]);
        // The length slider: its ends are 2 s and 10 s, the middle 6 s, and a drag follows the pointer in whole steps.
        let tr = g.controls[1].2;
        assert_eq!(click(&mut ui, &m, tr.x, tr.cy()), vec![Action::SetCrossfadeSecs(2)]);
        assert_eq!(click(&mut ui, &m, tr.right(), tr.cy()), vec![Action::SetCrossfadeSecs(10)]);
        let mut out = ui.handle(
            &InputEvent::PointerDown { x: tr.x + tr.w * 0.5, y: tr.cy(), button: PointerButton::Primary },
            5,
            &m,
        );
        out.extend(ui.handle(&InputEvent::PointerMove { x: tr.x + tr.w * 0.9, y: tr.cy() + 40.0 }, 6, &m));
        out.extend(ui.handle(
            &InputEvent::PointerUp { x: tr.x + tr.w * 0.9, y: tr.cy(), button: PointerButton::Primary },
            7,
            &m,
        ));
        assert_eq!(out, vec![Action::SetCrossfadeSecs(6), Action::SetCrossfadeSecs(9)]);
        // The target slider spans -23 to -10.
        let tg = g.controls[3].2;
        assert_eq!(click(&mut ui, &m, tg.x, tg.cy()), vec![Action::SetTargetLufs(-23)]);
        assert_eq!(click(&mut ui, &m, tg.right(), tg.cy()), vec![Action::SetTargetLufs(-10)]);
        // The segmented control: left half track, right half album (nothing if it is already so).
        let seg = g.controls[4].2;
        assert_eq!(
            click(&mut ui, &m, seg.x + seg.w * 0.75, seg.cy()),
            vec![Action::SetLevelMode(LevelMode::Album)]
        );
        assert_eq!(click(&mut ui, &m, seg.x + seg.w * 0.25, seg.cy()), vec![]);
        // The wheel over a slider steps it.
        ui.handle(&InputEvent::PointerMove { x: tr.cx(), y: tr.cy() }, 8, &m);
        assert_eq!(
            ui.handle(&InputEvent::Wheel { dx: 0.0, dy: -40.0 }, 9, &m),
            vec![Action::SetCrossfadeSecs(6)]
        );
        assert_eq!(
            ui.handle(&InputEvent::Wheel { dx: 0.0, dy: 40.0 }, 10, &m),
            vec![Action::SetCrossfadeSecs(4)]
        );
        // Done and a click outside close the panel; a click inside on nothing does not.
        let g = ui.audio_panel_geom();
        click(&mut ui, &m, g.card.x + 10.0, g.card.y + g.card.h * 0.5);
        assert!(ui.audio_settings_open());
        let done = g.controls[5].2;
        click(&mut ui, &m, done.cx(), done.cy());
        assert!(!ui.audio_settings_open());
        ui.open_audio_settings();
        click(&mut ui, &m, 5.0, 5.0);
        assert!(!ui.audio_settings_open());
        ui.open_audio_settings();
        let g = ui.audio_panel_geom();
        click(&mut ui, &m, g.close.cx(), g.close.cy());
        assert!(!ui.audio_settings_open());
    }

    #[test]
    fn the_keyboard_reaches_every_control() {
        let mut ui = ui();
        ui.open_audio_settings();
        let mut a = AudioSettings::default();
        let m = |a| model(a);
        // Tab walks the controls in order and wraps; Shift+Tab goes back.
        let tab = Key::Other("Tab".into());
        let mut seen = vec![ui.audio_panel_focus().unwrap()];
        for _ in 0..6 {
            key(&mut ui, &m(a), tab.clone(), false);
            seen.push(ui.audio_panel_focus().unwrap());
        }
        assert_eq!(seen[0], AudioControl::Crossfade);
        assert_eq!(seen[5], AudioControl::Done);
        assert_eq!(seen[6], AudioControl::Crossfade);
        key(&mut ui, &m(a), tab.clone(), true);
        assert_eq!(ui.audio_panel_focus(), Some(AudioControl::Done));
        key(&mut ui, &m(a), Key::Up, false);
        assert_eq!(ui.audio_panel_focus(), Some(AudioControl::Mode));
        // Switches: Space and Enter flip, Left is off, Right is on.
        key(&mut ui, &m(a), tab.clone(), false); // Done
        key(&mut ui, &m(a), tab.clone(), false); // Crossfade
        assert_eq!(key(&mut ui, &m(a), Key::Space, false), vec![Action::SetCrossfade(true)]);
        assert_eq!(key(&mut ui, &m(a), Key::Right, false), vec![Action::SetCrossfade(true)]);
        a.crossfade = true;
        assert_eq!(key(&mut ui, &m(a), Key::Right, false), vec![]);
        assert_eq!(key(&mut ui, &m(a), Key::Enter, false), vec![Action::SetCrossfade(false)]);
        assert_eq!(key(&mut ui, &m(a), Key::Left, false), vec![Action::SetCrossfade(false)]);
        // The length slider: arrows step, Home and End jump, and nothing happens past an end.
        key(&mut ui, &m(a), Key::Down, false);
        assert_eq!(ui.audio_panel_focus(), Some(AudioControl::CrossfadeLength));
        assert_eq!(key(&mut ui, &m(a), Key::Right, false), vec![Action::SetCrossfadeSecs(6)]);
        assert_eq!(key(&mut ui, &m(a), Key::Left, false), vec![Action::SetCrossfadeSecs(4)]);
        assert_eq!(key(&mut ui, &m(a), Key::Home, false), vec![Action::SetCrossfadeSecs(2)]);
        assert_eq!(key(&mut ui, &m(a), Key::End, false), vec![Action::SetCrossfadeSecs(10)]);
        a.crossfade_secs = 10;
        assert_eq!(key(&mut ui, &m(a), Key::Right, false), vec![]);
        // The target slider and the mode.
        key(&mut ui, &m(a), Key::Down, false);
        assert_eq!(key(&mut ui, &m(a), Key::Space, false), vec![Action::SetAutoLevel(true)]);
        key(&mut ui, &m(a), Key::Down, false);
        assert_eq!(ui.audio_panel_focus(), Some(AudioControl::Target));
        assert_eq!(key(&mut ui, &m(a), Key::Right, false), vec![Action::SetTargetLufs(-13)]);
        assert_eq!(key(&mut ui, &m(a), Key::Home, false), vec![Action::SetTargetLufs(-23)]);
        key(&mut ui, &m(a), Key::Down, false);
        assert_eq!(ui.audio_panel_focus(), Some(AudioControl::Mode));
        assert_eq!(key(&mut ui, &m(a), Key::Right, false), vec![Action::SetLevelMode(LevelMode::Album)]);
        assert_eq!(key(&mut ui, &m(a), Key::Space, false), vec![Action::SetLevelMode(LevelMode::Album)]);
        // Done closes; so does Escape. Keys the panel does not know are swallowed (no shortcut runs under it).
        assert_eq!(key(&mut ui, &m(a), Key::Char('n'), false), vec![]);
        assert!(ui.audio_settings_open());
        key(&mut ui, &m(a), Key::Down, false);
        key(&mut ui, &m(a), Key::Enter, false);
        assert!(!ui.audio_settings_open());
    }
}
