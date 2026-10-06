//! A small modal dialog the application describes and the UI draws: a title, a few paragraphs, an optional progress bar, switches and
//! buttons. Used for update prompts and the app-menu offer; it owns no logic, every press is an [`Action`] and the application answers with
//! a new [`DialogSpec`] (so what is shown is always what is true).
//!
//! Mouse: click a switch or a button, the corner X closes. Keyboard: Tab, Shift+Tab and the arrows move between the controls (switches,
//! then buttons), Space and Enter press, Escape closes. The dialog takes every event while it is up.
use crate::actions::Action;
use crate::font::Face;
use crate::gfx::{FrameBuffer, Paint, RectF, fade};
use crate::icon::Icon;
use crate::ui::Ui;
use alloc::string::String;
use alloc::vec::Vec;
use rvp_host::{InputEvent, Key, Modifiers, PointerButton};
use theme::tokens as t;

/// A button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogButton {
    /// What it says.
    pub label: String,
    /// The main choice (drawn with the gradient; the keyboard starts on it).
    pub primary: bool,
    /// False greys it out.
    pub enabled: bool,
}

impl DialogButton {
    /// A button.
    pub fn new(label: &str, primary: bool) -> Self {
        Self { label: label.into(), primary, enabled: true }
    }
}

/// A switch with a label and a line of explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogToggle {
    /// What it says.
    pub label: String,
    /// A smaller line below it.
    pub desc: String,
    /// On or off.
    pub on: bool,
}

/// Everything a dialog shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DialogSpec {
    /// The heading.
    pub title: String,
    /// Paragraphs (wrapped to the card).
    pub body: Vec<String>,
    /// A progress bar, in thousandths (0..=1000).
    pub progress: Option<u16>,
    /// Switches, top to bottom.
    pub toggles: Vec<DialogToggle>,
    /// Buttons, left to right.
    pub buttons: Vec<DialogButton>,
}

/// What can have the keyboard focus or the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogControl {
    /// Switch number `n`.
    Toggle(u8),
    /// Button number `n`.
    Button(u8),
}

impl DialogControl {
    /// A stable name for tests and the snapshot.
    pub fn name(self) -> String {
        match self {
            DialogControl::Toggle(n) => alloc::format!("toggle{n}"),
            DialogControl::Button(n) => alloc::format!("button{n}"),
        }
    }
}

/// Where everything is, in pixels.
#[derive(Debug, Clone)]
pub struct DialogGeom {
    /// The card.
    pub card: RectF,
    /// The close X.
    pub close: RectF,
    /// Where the body paragraphs start (top).
    pub body_top: f32,
    /// The progress bar, if any.
    pub progress: Option<RectF>,
    /// Each switch's row (the whole row reacts) and its drawn switch.
    pub toggles: Vec<(RectF, RectF)>,
    /// Each button.
    pub buttons: Vec<RectF>,
}

/// The state the UI keeps for the dialog.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct DialogState {
    /// The control the keyboard is on (an index: switches first, then buttons).
    pub focus: Option<usize>,
    /// The control under the pointer.
    pub hover: Option<DialogControl>,
    /// Wrapped body lines, per paragraph, for the width last laid out (the geometry and the drawing agree).
    pub lines: Vec<Vec<String>>,
    /// What the controls were when the focus was last set: a dialog whose buttons change starts over on its primary button.
    pub shape: String,
}

const CARD_MAX_W: f32 = 460.0;
const PAD: f32 = 28.0;
const LINE_H: f32 = 21.0;

impl Ui {
    /// The wrapped body lines for the card width.
    fn dialog_lines(&mut self, spec: &DialogSpec, inner: f32) -> Vec<Vec<String>> {
        spec.body.iter().map(|p| self.wrap(Face::Sans, 14.0, p, inner, 12)).collect()
    }

    /// The geometry of `spec` for the window as it is now.
    pub fn dialog_geom(&mut self, spec: &DialogSpec) -> DialogGeom {
        let s = self.scale;
        let (w, h) = (self.w as f32, self.h as f32);
        let cw = CARD_MAX_W.min(w / s - 32.0).max(280.0) * s;
        let pad = PAD * s;
        let inner = cw - 2.0 * pad;
        let lines = self.dialog_lines(spec, inner);
        let n_lines: usize = lines.iter().map(|p| p.len()).sum();
        let gaps = spec.body.len().saturating_sub(1) as f32 * 8.0;
        let body_h = n_lines as f32 * LINE_H + gaps;
        let mut y = 72.0 + body_h; // in unscaled units, relative to the card top
        let progress_y = y + 8.0;
        if spec.progress.is_some() {
            y += 8.0 + 8.0 + 8.0;
        }
        y += 4.0;
        let toggle_ys: Vec<f32> = spec
            .toggles
            .iter()
            .map(|_| {
                let at = y;
                y += 60.0;
                at
            })
            .collect();
        y += 12.0;
        // Buttons: one row when they fit, else stacked.
        let widths: Vec<f32> = {
            let mut v = Vec::new();
            for b in &spec.buttons {
                let tw = self.text_w(Face::SansBold, 14.0, &b.label, 0.0) / s;
                v.push((tw + 40.0).max(96.0));
            }
            v
        };
        let row_w: f32 = widths.iter().sum::<f32>() + 8.0 * widths.len().saturating_sub(1) as f32;
        let stacked = row_w > inner / s;
        let btn_h = 42.0;
        let buttons_top = y;
        let mut button_rects_rel: Vec<(f32, f32, f32, f32)> = Vec::new();
        if stacked {
            for (i, _) in spec.buttons.iter().enumerate() {
                button_rects_rel.push((0.0, buttons_top + i as f32 * (btn_h + 8.0), inner / s, btn_h));
            }
            y = buttons_top + spec.buttons.len() as f32 * (btn_h + 8.0);
        } else {
            let mut x = inner / s - row_w;
            for wd in &widths {
                button_rects_rel.push((x, buttons_top, *wd, btn_h));
                x += wd + 8.0;
            }
            y = buttons_top + if spec.buttons.is_empty() { 0.0 } else { btn_h };
        }
        let ch = (y + 28.0) * s;
        let card = RectF::new(((w - cw) * 0.5).max(0.0), ((h - ch) * 0.5).max(8.0 * s), cw, ch);
        let x0 = card.x + pad;
        let close =
            RectF::new(card.right() - pad - 32.0 * s + 8.0 * s, card.y + 18.0 * s, 36.0 * s, 36.0 * s);
        let progress = spec.progress.map(|_| RectF::new(x0, card.y + progress_y * s, inner, 8.0 * s));
        let toggles = toggle_ys
            .iter()
            .map(|ty| {
                let row = RectF::new(card.x + 8.0 * s, card.y + ty * s, card.w - 16.0 * s, 56.0 * s);
                let sw = RectF::new(x0 + inner - 48.0 * s, card.y + (ty + 14.0) * s, 48.0 * s, 28.0 * s);
                (row, sw)
            })
            .collect();
        let buttons = button_rects_rel
            .iter()
            .map(|(bx, by, bw, bh)| RectF::new(x0 + bx * s, card.y + by * s, bw * s, bh * s))
            .collect();
        self.dialog.lines = lines;
        DialogGeom { card, close, body_top: card.y + 64.0 * s, progress, toggles, buttons }
    }

    fn dialog_controls(spec: &DialogSpec) -> Vec<DialogControl> {
        let mut v: Vec<DialogControl> = (0..spec.toggles.len() as u8).map(DialogControl::Toggle).collect();
        v.extend((0..spec.buttons.len() as u8).map(DialogControl::Button));
        v
    }

    fn dialog_hit(g: &DialogGeom, x: f32, y: f32) -> Option<DialogControl> {
        for (i, b) in g.buttons.iter().enumerate() {
            if b.contains(x, y) {
                return Some(DialogControl::Button(i as u8));
            }
        }
        for (i, (row, _)) in g.toggles.iter().enumerate() {
            if row.contains(x, y) {
                return Some(DialogControl::Toggle(i as u8));
            }
        }
        None
    }

    /// The control the keyboard is on, while a dialog is up.
    pub fn dialog_focus(&self, spec: &DialogSpec) -> Option<DialogControl> {
        let controls = Self::dialog_controls(spec);
        self.dialog.focus.and_then(|i| controls.get(i).copied())
    }

    fn dialog_activate(spec: &DialogSpec, c: DialogControl, out: &mut Vec<Action>) {
        match c {
            DialogControl::Toggle(n) => out.push(Action::DialogToggle(n)),
            DialogControl::Button(n) => {
                if spec.buttons.get(n as usize).is_some_and(|b| b.enabled) {
                    out.push(Action::DialogButton(n));
                }
            }
        }
    }

    /// The controls of `spec` as one string, to notice when they change.
    fn dialog_shape(spec: &DialogSpec) -> String {
        let mut s = String::new();
        for t in &spec.toggles {
            s.push_str(&t.label);
            s.push('\u{1}');
        }
        s.push('\u{2}');
        for b in &spec.buttons {
            s.push_str(&b.label);
            s.push('\u{1}');
        }
        s
    }

    /// Put the keyboard on the primary button when the dialog is new or its controls changed.
    fn dialog_sync_focus(&mut self, spec: &DialogSpec) {
        let shape = Self::dialog_shape(spec);
        if self.dialog.focus.is_none() || self.dialog.shape != shape {
            self.dialog.shape = shape;
            self.dialog.focus = Self::dialog_default_focus(spec);
        }
    }

    /// Where the keyboard starts: the primary button, else the first control.
    fn dialog_default_focus(spec: &DialogSpec) -> Option<usize> {
        let controls = Self::dialog_controls(spec);
        if controls.is_empty() {
            return None;
        }
        let primary =
            spec.buttons.iter().position(|b| b.primary && b.enabled).map(|i| spec.toggles.len() + i);
        Some(primary.unwrap_or(0))
    }

    /// One input event while a dialog is up. Everything is taken by the dialog.
    pub(crate) fn dialog_event(&mut self, ev: &InputEvent, now_us: i64, spec: &DialogSpec) -> Vec<Action> {
        self.now = now_us;
        self.dirty = true;
        let mut out = Vec::new();
        self.dialog_sync_focus(spec);
        let controls = Self::dialog_controls(spec);
        let g = self.dialog_geom(spec);
        match ev {
            InputEvent::Resize { w, h, dpr } => self.set_size(*w, *h, *dpr),
            InputEvent::Focus(f) => self.win_focused = *f,
            InputEvent::PointerMove { x, y } => {
                self.pointer = Some((*x, *y));
                self.keyboard_mode = false;
                self.dialog.hover = Self::dialog_hit(&g, *x, *y);
            }
            InputEvent::PointerDown { x, y, button } => {
                self.pointer = Some((*x, *y));
                self.keyboard_mode = false;
                if *button != PointerButton::Primary {
                    return out;
                }
                if g.close.contains(*x, *y) {
                    out.push(Action::DialogClose);
                    return out;
                }
                // A click outside the card does nothing: the dialog is closed on purpose, with a button or Escape.
                if let Some(c) = Self::dialog_hit(&g, *x, *y) {
                    let idx = controls.iter().position(|o| *o == c);
                    self.dialog.focus = idx;
                    Self::dialog_activate(spec, c, &mut out);
                }
            }
            InputEvent::KeyDown { key, mods, .. } => {
                self.last_activity = now_us;
                self.keyboard_mode = true;
                self.key_used = true;
                self.dialog_key(spec, &controls, key, mods, &mut out);
            }
            _ => {}
        }
        out
    }

    fn dialog_key(
        &mut self,
        spec: &DialogSpec,
        controls: &[DialogControl],
        key: &Key,
        mods: &Modifiers,
        out: &mut Vec<Action>,
    ) {
        let n = controls.len();
        match key {
            Key::Escape => out.push(Action::DialogClose),
            _ if n == 0 => {}
            Key::Other(name) if name == "Tab" => {
                let f = self.dialog.focus.unwrap_or(0);
                self.dialog.focus = Some(if mods.shift { (f + n - 1) % n } else { (f + 1) % n });
            }
            Key::Down | Key::Right => self.dialog.focus = Some((self.dialog.focus.unwrap_or(0) + 1) % n),
            Key::Up | Key::Left => self.dialog.focus = Some((self.dialog.focus.unwrap_or(0) + n - 1) % n),
            Key::Space | Key::Enter => {
                if let Some(c) = self.dialog.focus.and_then(|f| controls.get(f).copied()) {
                    Self::dialog_activate(spec, c, out);
                }
            }
            _ => {}
        }
    }

    /// Forget the keyboard focus when no dialog is up (so the next one starts on its primary button).
    pub(crate) fn dialog_closed(&mut self) {
        if self.dialog != DialogState::default() {
            self.dialog = DialogState::default();
        }
    }

    /// Draw the model's dialog, if it has one (and forget the dialog state when it has none).
    pub(crate) fn draw_app_dialog(&mut self, fb: &mut FrameBuffer, model: &crate::model::UiModel) {
        match &model.dialog {
            Some(d) => {
                self.dialog_open = true;
                self.draw_dialog(fb, d);
            }
            None => {
                self.dialog_open = false;
                self.dialog_closed();
            }
        }
    }

    // ---- drawing --------------------------------------------------------------------------------------------------

    /// Draw the dialog over everything.
    pub(crate) fn draw_dialog(&mut self, fb: &mut FrameBuffer, spec: &DialogSpec) {
        let s = self.scale;
        self.dialog_sync_focus(spec);
        let g = self.dialog_geom(spec);
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
        let pad = PAD * s;
        let x0 = card.x + pad;
        let inner = card.w - 2.0 * pad;
        let title = self.fonts.fit(Face::SansBold, 20.0 * s, &spec.title, inner - 44.0 * s);
        self.text(fb, Face::SansBold, 20.0, x0, card.y + 36.0 * s, &title, t::TEXT_STRONG, 1.0, -0.2);
        let hot_close =
            self.dialog.hover.is_none() && self.pointer.is_some_and(|(x, y)| g.close.contains(x, y));
        if hot_close {
            fb.fill_rrect(g.close, g.close.h * 0.5, Paint::Solid(fade(t::WHITE, 0.10)), 1.0);
        }
        self.icon(fb, Icon::X, g.close.cx(), g.close.cy(), 18.0, t::TEXT_MUTED, 1.0, false);

        // The paragraphs.
        let mut y = g.body_top + 4.0 * s;
        let lines = self.dialog.lines.clone();
        for (pi, para) in lines.iter().enumerate() {
            for ln in para {
                self.text(fb, Face::Sans, 14.0, x0, y + LINE_H * s * 0.5, ln, t::TEXT_MUTED, 1.0, 0.0);
                y += LINE_H * s;
            }
            if pi + 1 < lines.len() {
                y += 8.0 * s;
            }
        }
        // The progress bar.
        if let (Some(r), Some(p)) = (g.progress, spec.progress) {
            fb.fill_rrect(r, r.h * 0.5, Paint::Solid(fade(t::WHITE, 0.22)), 1.0);
            let f = (p.min(1000) as f32) / 1000.0;
            let fill = RectF::new(r.x, r.y, (r.w * f).max(r.h), r.h);
            fb.fill_rrect(fill, r.h * 0.5, Paint::Horizontal(t::MAGENTA_500, t::VIOLET_400), 1.0);
        }
        let focus = if self.keyboard_mode { self.dialog_focus(spec) } else { None };
        // The switches.
        for (i, tg) in spec.toggles.iter().enumerate() {
            let (row, sw) = g.toggles[i];
            let c = DialogControl::Toggle(i as u8);
            let room = inner - 64.0 * s;
            let label = self.fonts.fit(Face::SansMedium, 15.0 * s, &tg.label, room);
            self.text(fb, Face::SansMedium, 15.0, x0, row.y + 20.0 * s, &label, t::TEXT_STRONG, 1.0, 0.0);
            if !tg.desc.is_empty() {
                let d = self.fonts.fit(Face::Sans, 12.0 * s, &tg.desc, room);
                self.text(fb, Face::Sans, 12.0, x0, row.y + 40.0 * s, &d, t::TEXT_DIM, 1.0, 0.0);
            }
            self.draw_switch(fb, sw, tg.on, self.dialog.hover == Some(c));
            if focus == Some(c) {
                self.focus_ring(fb, sw, sw.h * 0.5, 1.0);
            }
        }
        // The buttons.
        for (i, b) in spec.buttons.iter().enumerate() {
            let r = g.buttons[i];
            let c = DialogControl::Button(i as u8);
            let hot = self.dialog.hover == Some(c) && b.enabled;
            let a = if b.enabled { 1.0 } else { 0.45 };
            if b.primary {
                if hot || focus == Some(c) {
                    fb.glow_rrect(r, r.h * 0.5, 20.0 * s, t::MAGENTA_500, 0.5);
                }
                fb.fill_rrect(r, r.h * 0.5, Paint::Gradient(&t::GRADIENT_TEARS), a);
                if hot {
                    fb.fill_rrect(r, r.h * 0.5, Paint::Solid(t::WHITE), 0.12);
                }
            } else {
                fb.fill_rrect(r, r.h * 0.5, Paint::Solid(fade(t::WHITE, if hot { 0.14 } else { 0.08 })), a);
                fb.stroke_rrect(r, r.h * 0.5, 1.0 * s, fade(t::WHITE, 0.22), a);
            }
            let label = self.fonts.fit(Face::SansBold, 14.0 * s, &b.label, r.w - 16.0 * s);
            let tw = self.text_w(Face::SansBold, 14.0, &label, 0.0);
            let col = if b.primary { t::WHITE } else { t::TEXT_STRONG };
            self.text(fb, Face::SansBold, 14.0, r.cx() - tw * 0.5, r.cy(), &label, col, a, 0.0);
            if focus == Some(c) {
                self.focus_ring(fb, r, r.h * 0.5, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MediaState, UiModel};
    use crate::ui::UiConfig;

    fn spec() -> DialogSpec {
        DialogSpec {
            title: "Rusty Wave 0.0.3 is available".into(),
            body: alloc::vec!["You have 0.0.2. The update is downloaded, checked against the release key and installed in place.".into()],
            progress: None,
            toggles: alloc::vec![DialogToggle { label: "Check automatically".into(), desc: "Once a day".into(), on: false }],
            buttons: alloc::vec![
                DialogButton::new("Update now", true),
                DialogButton::new("Later", false),
                DialogButton::new("Skip this version", false)
            ],
        }
    }

    fn ui(w: u32) -> Ui {
        let mut ui = Ui::new(UiConfig::default());
        ui.set_size(w, 720, 1.0);
        ui
    }

    fn model(d: Option<DialogSpec>) -> UiModel {
        UiModel { state: MediaState::Paused, volume: 1.0, rate: 1.0, dialog: d, ..UiModel::default() }
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
    fn clicks_press_the_buttons_and_the_switch() {
        let mut ui = ui(1280);
        let sp = spec();
        let m = model(Some(sp.clone()));
        let g = ui.dialog_geom(&sp);
        let c = |r: RectF| (r.cx(), r.cy());
        let (x, y) = c(g.buttons[0]);
        assert_eq!(click(&mut ui, &m, x, y), alloc::vec![Action::DialogButton(0)]);
        let (x, y) = c(g.buttons[2]);
        assert_eq!(click(&mut ui, &m, x, y), alloc::vec![Action::DialogButton(2)]);
        let (x, y) = c(g.toggles[0].1);
        assert_eq!(click(&mut ui, &m, x, y), alloc::vec![Action::DialogToggle(0)]);
        let (x, y) = c(g.close);
        assert_eq!(click(&mut ui, &m, x, y), alloc::vec![Action::DialogClose]);
        // Outside the card, and on empty card, nothing happens.
        assert!(click(&mut ui, &m, 2.0, 2.0).is_empty());
        assert!(click(&mut ui, &m, g.card.x + 10.0, g.card.y + 100.0).is_empty());
    }

    #[test]
    fn the_keyboard_starts_on_the_primary_button_and_walks_the_controls() {
        let mut ui = ui(1280);
        let sp = spec();
        let m = model(Some(sp.clone()));
        // Enter presses the primary button first.
        assert_eq!(key(&mut ui, &m, Key::Enter, false), alloc::vec![Action::DialogButton(0)]);
        // Tab goes on to "Later", Shift+Tab back twice reaches the switch.
        assert!(key(&mut ui, &m, Key::Other("Tab".into()), false).is_empty());
        assert_eq!(key(&mut ui, &m, Key::Space, false), alloc::vec![Action::DialogButton(1)]);
        key(&mut ui, &m, Key::Other("Tab".into()), true);
        key(&mut ui, &m, Key::Other("Tab".into()), true);
        assert_eq!(key(&mut ui, &m, Key::Enter, false), alloc::vec![Action::DialogToggle(0)]);
        // It wraps around.
        key(&mut ui, &m, Key::Other("Tab".into()), true);
        assert_eq!(key(&mut ui, &m, Key::Enter, false), alloc::vec![Action::DialogButton(2)]);
        assert_eq!(key(&mut ui, &m, Key::Escape, false), alloc::vec![Action::DialogClose]);
        // Other keys do not leak through to the player.
        assert!(key(&mut ui, &m, Key::Char('q'), false).is_empty());
    }

    #[test]
    fn a_disabled_button_does_nothing_and_a_dialog_without_controls_still_closes() {
        let mut ui = ui(1280);
        let mut sp = spec();
        sp.buttons[0].enabled = false;
        let m = model(Some(sp.clone()));
        let g = ui.dialog_geom(&sp);
        assert!(click(&mut ui, &m, g.buttons[0].cx(), g.buttons[0].cy()).is_empty());
        let bare = DialogSpec { title: "Checking".into(), ..DialogSpec::default() };
        let m2 = model(Some(bare));
        assert_eq!(key(&mut ui, &m2, Key::Escape, false), alloc::vec![Action::DialogClose]);
        assert!(key(&mut ui, &m2, Key::Enter, false).is_empty());
    }

    #[test]
    fn the_card_fits_a_narrow_window_and_buttons_stack_when_they_must() {
        let mut ui = ui(300);
        let sp = spec();
        let g = ui.dialog_geom(&sp);
        assert!(g.card.x >= 0.0 && g.card.right() <= 300.0 + 0.5, "{:?}", g.card);
        // Stacked: each button is below the one before and as wide as the card's inside.
        assert!(g.buttons[1].y > g.buttons[0].y && g.buttons[2].y > g.buttons[1].y);
        assert!((g.buttons[0].w - g.buttons[1].w).abs() < 0.5);
        assert!(g.buttons.iter().all(|b| b.right() <= g.card.right() && b.x >= g.card.x));
        // Wide: one row, in order.
        let mut wide = self::ui(1280);
        let gw = wide.dialog_geom(&sp);
        assert!(gw.buttons[0].x < gw.buttons[1].x && (gw.buttons[0].y - gw.buttons[1].y).abs() < 0.5);
        assert!(gw.card.bottom() <= 720.0);
    }

    #[test]
    fn drawing_does_not_panic_for_any_shape() {
        let mut ui = ui(800);
        let mut fb = FrameBuffer::new(800, 720);
        let mut with_progress = spec();
        with_progress.progress = Some(430);
        for sp in [
            spec(),
            with_progress,
            DialogSpec::default(),
            DialogSpec {
                title: "x".repeat(200),
                body: alloc::vec!["y ".repeat(500)],
                ..DialogSpec::default()
            },
        ] {
            ui.draw_dialog(&mut fb, &sp);
        }
    }
}
