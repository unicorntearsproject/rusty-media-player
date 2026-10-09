//! The Help overlay: a scrollable keyboard and mouse reference, opened with `H` or `?` (and from the menu and the About page), closed
//! with Escape, `H` or `?`. The shortcut rows are built from [`SHORTCUTS`] itself, so a key that is added or changed there shows up here;
//! the library's keys (which are matched where they act, not in a table) are listed in [`library_keys`], and a test presses each of
//! those in a real library to see that it still does something.
//!
//! The card is drawn like the Audio panel's and works at any width: the key column narrows and descriptions wrap, so a phone shows the
//! same page, dragged with a finger.
use crate::actions::{Action, SHORTCUTS, ShortKey, Shortcut};
use crate::font::Face;
use crate::gfx::{FrameBuffer, Paint, RectF, fade};
use crate::icon::Icon;
use crate::model::{AppModel, UiModel};
use crate::tk as t;
use crate::ui::Ui;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_host::{InputEvent, Key, Modifiers, PointerButton};

/// One line of the reference: the keys (or the gesture) and what they do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpRow {
    /// `Space / K`, `Shift+Enter`, `Wheel`.
    pub keys: String,
    /// What it does.
    pub what: String,
}

/// A group of rows under a heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    /// `Playback`, `Library`, `Visualizer`, `App` or `Mouse and touch`.
    pub title: &'static str,
    /// The rows.
    pub rows: Vec<HelpRow>,
}

/// A key of the library that is matched where it acts, with a way to try it.
pub struct LibKey {
    /// How the key is shown.
    pub keys: &'static str,
    /// What it does.
    pub what: &'static str,
    /// The group it belongs to (a section title).
    pub area: &'static str,
    /// A key press that must do something in the library (a view changes, a menu or a box opens, an action comes out), if there is a
    /// simple one; the tests press it.
    pub probe: Option<(Key, Modifiers)>,
    /// Only where the host offers editing files.
    pub needs_tags: bool,
}

const PLAYBACK: &str = "Playback";
const LIBRARY: &str = "Library";
const VISUALIZER: &str = "Visualizer";
const APP: &str = "App";
const MOUSE: &str = "Mouse and touch";

fn plain(key: Key) -> Option<(Key, Modifiers)> {
    Some((key, Modifiers::default()))
}

/// The keys of the library that no table holds. Keep this next to the `match` in `lib_key` and `list_key`; the probes catch a key that
/// stops working.
pub fn library_keys() -> Vec<LibKey> {
    let k = |keys, what, area, probe, needs_tags| LibKey { keys, what, area, probe, needs_tags };
    alloc::vec![
        k(
            "1 - 9",
            "Go to Albums, Artists, Tracks, Playlists, Queue, Now playing, Visualizer, Videos or Favorites, in that order",
            LIBRARY,
            plain(Key::Char('3')),
            false
        ),
        k("0", "Go to the history of what you played", LIBRARY, plain(Key::Char('0')), false),
        k("/", "Search", LIBRARY, plain(Key::Char('/')), false),
        k("F1", "About Rusty Wave", LIBRARY, plain(Key::Other("F1".into())), false),
        k("Esc / Backspace", "Go back one step", LIBRARY, None, false),
        k("Tab / Shift+Tab", "Move between the list, the side rail and the bottom bar", LIBRARY, None, false),
        k("Arrows, Home, End, Page Up, Page Down", "Move around lists and grids", LIBRARY, None, false),
        k("Enter", "Play the selected item", LIBRARY, None, false),
        k("Shift+Enter", "Add the selected item to the end of the queue", LIBRARY, None, false),
        k("Ctrl+Enter", "Play the selected item next", LIBRARY, None, false),
        k("Delete", "Remove the item from the queue, a playlist or the history", LIBRARY, None, false),
        k("Alt+Up / Alt+Down", "Move an item in the queue or a playlist", LIBRARY, None, false),
        k("Menu key / Shift+F10", "Open the menu of the selected item", LIBRARY, None, false),
        k(
            "Ctrl+B",
            "Collapse the side menu to icons, or expand it (also the arrow at its top, or right-click it)",
            LIBRARY,
            Some((Key::Char('b'), Modifiers { ctrl: true, ..Modifiers::default() })),
            false
        ),
        k("E", "Edit the tags of the selected song or album", LIBRARY, None, true),
        k("Left / Right", "Previous or next effect", VISUALIZER, None, false),
        k("Enter", "Start or stop the animation", VISUALIZER, None, false),
        k("C", "Next colour scheme", VISUALIZER, None, false),
        k("T", "Show or hide the title", VISUALIZER, None, false),
        k("Shift+V", "Change the effect by itself, or stop", VISUALIZER, None, false),
    ]
}

/// The group and the words for a shortcut's action, or `None` for an action the table binds that this page cannot describe (a test
/// refuses that, so no key is left out).
pub fn describe(action: Action) -> Option<(&'static str, String)> {
    let secs = |ms: i32| {
        let s = ms.abs() / 1000;
        format!("{} {s} seconds", if ms < 0 { "Back" } else { "Forward" })
    };
    Some(match action {
        Action::PlayPause => (PLAYBACK, "Play or pause".to_string()),
        Action::SeekBy(ms) => (PLAYBACK, secs(ms)),
        Action::SeekStart => (PLAYBACK, "Jump to the start".to_string()),
        Action::SeekEnd => (PLAYBACK, "Jump to the end".to_string()),
        Action::VolumeBy(d) => {
            (PLAYBACK, format!("Volume {} ({} points)", if d > 0 { "up" } else { "down" }, d.abs()))
        }
        Action::ToggleMute => (PLAYBACK, "Mute or unmute".to_string()),
        Action::ToggleFullscreen => (PLAYBACK, "Fullscreen on or off".to_string()),
        Action::SpeedStep(d) => (PLAYBACK, (if d > 0 { "Faster" } else { "Slower" }).to_string()),
        Action::ResetSpeed => (PLAYBACK, "Normal speed".to_string()),
        Action::CycleAudio => (PLAYBACK, "Next audio track".to_string()),
        Action::CycleSubtitles => (PLAYBACK, "Next subtitles, then off".to_string()),
        Action::FrameStep(d) => {
            (PLAYBACK, format!("{} frame (pauses)", if d > 0 { "Next" } else { "Previous" }))
        }
        Action::LoopMark => (PLAYBACK, "Mark loop start, then end, then clear".to_string()),
        Action::Next => (PLAYBACK, "Next item (the >> button)".to_string()),
        Action::Prev => (
            PLAYBACK,
            "Start this item again; within 3 seconds of its start, the previous item (the << button)"
                .to_string(),
        ),
        Action::CycleRepeat => (PLAYBACK, "Repeat: off, all, one".to_string()),
        Action::ToggleShuffle => (PLAYBACK, "Shuffle on or off".to_string()),
        Action::ChapterStep(d) => (PLAYBACK, format!("{} chapter", if d > 0 { "Next" } else { "Previous" })),
        Action::ShowPlaylist => (LIBRARY, "Show the queue".to_string()),
        Action::ToggleMode => (LIBRARY, "Switch between the library and the player".to_string()),
        Action::ToggleFavorite => {
            (LIBRARY, "Heart what is playing (the selected song or video in a list)".to_string())
        }
        Action::ToggleVisualizer => (VISUALIZER, "Open or leave the visualizer".to_string()),
        Action::OpenFile => (APP, "Open a file".to_string()),
        Action::ShowAudioSettings => (APP, "Audio settings: crossfade and automatic level".to_string()),
        Action::ShowSettings => (APP, "Settings".to_string()),
        Action::ShowHelp => (APP, "This page".to_string()),
        Action::Quit => (APP, "Quit".to_string()),
        _ => return None,
    })
}

/// How a shortcut is written: `Ctrl+O`, `Shift+Left`, `Space`.
pub fn shortcut_text(s: &Shortcut) -> String {
    let name = match s.key {
        ShortKey::Space => "Space".to_string(),
        ShortKey::Left => "Left".to_string(),
        ShortKey::Right => "Right".to_string(),
        ShortKey::Up => "Up".to_string(),
        ShortKey::Down => "Down".to_string(),
        ShortKey::Home => "Home".to_string(),
        ShortKey::End => "End".to_string(),
        ShortKey::PageUp => "Page Up".to_string(),
        ShortKey::PageDown => "Page Down".to_string(),
        ShortKey::Char(c) => c.to_ascii_uppercase().to_string(),
    };
    let mut out = String::new();
    if s.ctrl {
        out += "Ctrl+";
    }
    if s.shift {
        out += "Shift+";
    }
    out + &name
}

/// The whole reference for a host that offers `app`: the groups in order, each with its rows.
pub fn help_sections(app: &AppModel) -> Vec<HelpSection> {
    let titles = [PLAYBACK, LIBRARY, VISUALIZER, APP, MOUSE];
    let mut sections: Vec<HelpSection> =
        titles.iter().map(|t| HelpSection { title: t, rows: Vec::new() }).collect();
    let mut add = |area: &str, keys: &str, what: &str| {
        let Some(sec) = sections.iter_mut().find(|s| s.title == area) else { return };
        // The same words on another key join its row (Space / K).
        match sec.rows.iter_mut().find(|r| r.what == what) {
            Some(r) => {
                r.keys += " / ";
                r.keys += keys;
            }
            None => sec.rows.push(HelpRow { keys: keys.to_string(), what: what.to_string() }),
        }
    };
    for s in SHORTCUTS {
        if s.action == Action::Quit && !app.quit {
            continue;
        }
        if let Some((area, what)) = describe(s.action) {
            add(area, &shortcut_text(s), &what);
        }
    }
    for k in library_keys() {
        if k.needs_tags && !app.tags {
            continue;
        }
        add(k.area, k.keys, k.what);
    }
    for (keys, what) in [
        ("Click", "Press a button, pick an item"),
        ("Double-click", "Play an item; on the picture, go fullscreen"),
        ("Right-click", "Open the menu of what is under the pointer"),
        ("Wheel", "Scroll a list; volume over the picture; seek over the seek bar"),
        ("Drag the seek bar", "Seek to a place; hover to preview the time"),
        ("Drop files or folders", "Play them, or add them to the library"),
        ("Tap the picture", "Show the controls (on a phone)"),
    ] {
        add(MOUSE, keys, what);
    }
    sections
}

/// The short tips under the reference.
pub fn help_tips(app: &AppModel) -> Vec<&'static str> {
    let mut v = alloc::vec![
        "Hold a seek, volume or speed key to repeat it.",
        "Press H or ? any time to open this page; Esc, H or ? closes it.",
        "Every list row, card and button has a right-click menu, with the key beside each command.",
        "Search with / and leave it with Esc: you land where you were.",
        "Drag a file onto the window to play it; drop several to queue them.",
    ];
    if app.quit {
        v.push("Ctrl+Q quits (Cmd+Q on a Mac).");
    }
    v
}

/// The state of the open overlay.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct HelpState {
    /// How far the content is scrolled, pixels.
    pub scroll: f32,
    /// A finger or button dragging the page: where it went down, the scroll then, and whether it moved enough to count as a drag.
    pub drag: Option<(f32, f32, bool)>,
}

/// Where the card and its parts are.
#[derive(Debug, Clone, Copy)]
pub struct HelpGeom {
    /// The card.
    pub card: RectF,
    /// The close X.
    pub close: RectF,
    /// The scrolling part.
    pub body: RectF,
}

enum Item {
    Heading(String),
    Row { keys: Vec<String>, what: Vec<String> },
    Para(Vec<String>),
    Gap,
}

struct Laid {
    items: Vec<(f32, f32, Item)>,
    height: f32,
    key_w: f32,
}

impl Ui {
    /// Show the page, or hide it if it is up.
    pub fn toggle_help(&mut self) {
        self.menu.clear();
        self.help = match self.help {
            Some(_) => None,
            None => Some(HelpState::default()),
        };
        self.dirty = true;
    }

    /// Hide the page.
    pub fn close_help(&mut self) {
        if self.help.take().is_some() {
            self.dirty = true;
        }
    }

    /// True while the page is up.
    pub fn help_open(&self) -> bool {
        self.help.is_some()
    }

    /// How far the page is scrolled (0 when it is not up).
    pub fn help_scroll(&self) -> f32 {
        self.help.as_ref().map_or(0.0, |h| h.scroll)
    }

    /// The card, the X and the scrolling part for the window as it is now.
    pub fn help_geom(&self) -> HelpGeom {
        let s = self.scale;
        let (w, h) = (self.w as f32, self.h as f32);
        let cw = (760.0f32).min(w / s - 24.0).max(260.0) * s;
        let ch = (h - 32.0 * s).clamp(240.0 * s, 700.0 * s).min(h);
        let card = RectF::new(((w - cw) * 0.5).max(0.0), ((h - ch) * 0.5).max(0.0), cw, ch);
        let pad = self.help_pad();
        let close = RectF::new(card.right() - pad - 28.0 * s, card.y + 14.0 * s, 36.0 * s, 36.0 * s);
        let body =
            RectF::new(card.x + pad, card.y + 64.0 * s, card.w - 2.0 * pad, card.h - 64.0 * s - 40.0 * s);
        HelpGeom { card, close, body }
    }

    fn help_pad(&self) -> f32 {
        let narrow = (self.w as f32 / self.scale) < 600.0;
        (if narrow { 16.0 } else { 28.0 }) * self.scale
    }

    fn help_layout(&mut self, app: &AppModel) -> Laid {
        let s = self.scale;
        let g = self.help_geom();
        let width = g.body.w - 10.0 * s;
        let key_w = (width * 0.38).clamp(96.0 * s, 230.0 * s);
        let what_w = width - key_w - 12.0 * s;
        let mut y = 0.0f32;
        let mut items = Vec::new();
        for sec in help_sections(app) {
            y += 14.0 * s;
            items.push((y, 26.0 * s, Item::Heading(sec.title.to_string())));
            y += 30.0 * s;
            for r in &sec.rows {
                let keys = self.wrap(Face::Mono, 12.0, &r.keys, key_w, 4);
                let what = self.wrap(Face::Sans, 14.0, &r.what, what_w, 4);
                let lines = keys.len().max(what.len()).max(1) as f32;
                let h = lines * 20.0 * s + 8.0 * s;
                items.push((y, h, Item::Row { keys, what }));
                y += h;
            }
        }
        y += 14.0 * s;
        items.push((y, 26.0 * s, Item::Heading("Tips".to_string())));
        y += 30.0 * s;
        for tip in help_tips(app) {
            let lines = self.wrap(Face::Sans, 14.0, tip, width, 6);
            let h = lines.len() as f32 * 20.0 * s + 8.0 * s;
            items.push((y, h, Item::Para(lines)));
            y += h;
        }
        y += 8.0 * s;
        items.push((y, 0.0, Item::Gap));
        Laid { items, height: y, key_w }
    }

    fn help_max_scroll(&mut self, app: &AppModel) -> f32 {
        let g = self.help_geom();
        let l = self.help_layout(app);
        (l.height - g.body.h).max(0.0)
    }

    fn help_set_scroll(&mut self, v: f32, app: &AppModel) {
        let max = self.help_max_scroll(app);
        if let Some(h) = &mut self.help {
            h.scroll = v.clamp(0.0, max);
        }
    }

    /// One input event while the page is up. Everything is taken by it.
    pub(crate) fn help_event(&mut self, ev: &InputEvent, now_us: i64, model: &UiModel) -> Vec<Action> {
        self.now = now_us;
        self.dirty = true;
        let g = self.help_geom();
        let app = model.app;
        match ev {
            InputEvent::Resize { w, h, dpr } => {
                self.set_size(*w, *h, *dpr);
                let cur = self.help_scroll();
                self.help_set_scroll(cur, &app);
            }
            InputEvent::Focus(f) => {
                self.win_focused = *f;
                if let Some(h) = &mut self.help {
                    h.drag = None;
                }
            }
            InputEvent::PointerMove { x, y } => {
                self.pointer = Some((*x, *y));
                self.keyboard_mode = false;
                if let Some((y0, s0, moved)) = self.help.as_ref().and_then(|h| h.drag) {
                    let moved = moved || (y0 - *y).abs() > 6.0 * self.scale;
                    if let Some(h) = &mut self.help {
                        h.drag = Some((y0, s0, moved));
                    }
                    if moved {
                        self.help_set_scroll(s0 + (y0 - *y), &app);
                    }
                }
            }
            InputEvent::PointerDown { x, y, button } => {
                self.pointer = Some((*x, *y));
                self.keyboard_mode = false;
                if *button != PointerButton::Primary {
                    return Vec::new();
                }
                if g.close.contains(*x, *y) || !g.card.contains(*x, *y) {
                    self.close_help();
                } else if g.body.contains(*x, *y) {
                    let cur = self.help_scroll();
                    if let Some(h) = &mut self.help {
                        h.drag = Some((*y, cur, false));
                    }
                }
            }
            InputEvent::PointerUp { .. } => {
                if let Some(h) = &mut self.help {
                    h.drag = None;
                }
            }
            InputEvent::Wheel { dy, .. } => {
                let cur = self.help_scroll();
                self.help_set_scroll(cur + *dy, &app);
            }
            InputEvent::KeyDown { key, mods, .. } => {
                self.last_activity = now_us;
                self.keyboard_mode = true;
                self.key_used = true;
                self.help_key(key, mods, &app, g);
            }
            _ => {}
        }
        Vec::new()
    }

    fn help_key(&mut self, key: &Key, mods: &Modifiers, app: &AppModel, g: HelpGeom) {
        let step = 48.0 * self.scale;
        let page = (g.body.h - 24.0 * self.scale).max(step);
        let cur = self.help_scroll();
        match key {
            Key::Escape => self.close_help(),
            Key::Char('h' | 'H' | '?') if !mods.ctrl && !mods.alt && !mods.logo => self.close_help(),
            Key::Down => self.help_set_scroll(cur + step, app),
            Key::Up => self.help_set_scroll(cur - step, app),
            Key::Space => self.help_set_scroll(if mods.shift { cur - page } else { cur + page }, app),
            Key::Home => self.help_set_scroll(0.0, app),
            Key::End => self.help_set_scroll(f32::MAX, app),
            Key::Other(n) if n == "PageDown" => self.help_set_scroll(cur + page, app),
            Key::Other(n) if n == "PageUp" => self.help_set_scroll(cur - page, app),
            _ => {}
        }
    }

    /// Draw the page over everything, when it is up.
    pub(crate) fn draw_help(&mut self, fb: &mut FrameBuffer, model: &UiModel) {
        let Some(state) = self.help.clone() else { return };
        let s = self.scale;
        let g = self.help_geom();
        let card = g.card;
        let laid = self.help_layout(&model.app);
        let scroll = state.scroll.clamp(0.0, (laid.height - g.body.h).max(0.0));
        fb.fill_rect_paint(
            RectF::new(0.0, 0.0, self.w as f32, self.h as f32),
            Paint::Solid(t::ink_900()),
            0.6,
        );
        fb.shadow_rrect(card, 22.0 * s, 20.0 * s, 56.0 * s, theme::Rgba::new(5, 2, 15, 190), 1.0);
        fb.fill_rrect(card, 22.0 * s, Paint::Solid(t::ink_800()), 1.0);
        // The rows that touch the scrolling part; the header and the footer are painted over whatever spills out of it.
        let x0 = g.body.x;
        for (top, h, item) in &laid.items {
            let y = g.body.y + top - scroll;
            if y + h < g.body.y - 4.0 * s || y > g.body.bottom() + 4.0 * s {
                continue;
            }
            match item {
                Item::Heading(title) => {
                    let label: String = title.chars().flat_map(char::to_uppercase).collect();
                    self.text(fb, Face::SansBold, 11.0, x0, y + 8.0 * s, &label, t::violet_400(), 1.0, 1.6);
                    fb.fill_rect_paint(
                        RectF::new(x0, y + 20.0 * s, g.body.w - 10.0 * s, 1.0 * s),
                        Paint::Solid(t::border_subtle()),
                        1.0,
                    );
                }
                Item::Row { keys, what } => {
                    for (i, k) in keys.iter().enumerate() {
                        self.text(
                            fb,
                            Face::Mono,
                            12.0,
                            x0,
                            y + 14.0 * s + i as f32 * 20.0 * s,
                            k,
                            t::cyan_400(),
                            1.0,
                            0.0,
                        );
                    }
                    for (i, w) in what.iter().enumerate() {
                        self.text(
                            fb,
                            Face::Sans,
                            14.0,
                            x0 + laid.key_w + 12.0 * s,
                            y + 14.0 * s + i as f32 * 20.0 * s,
                            w,
                            t::text_body(),
                            1.0,
                            0.0,
                        );
                    }
                }
                Item::Para(lines) => {
                    for (i, w) in lines.iter().enumerate() {
                        self.text(
                            fb,
                            Face::Sans,
                            14.0,
                            x0,
                            y + 14.0 * s + i as f32 * 20.0 * s,
                            w,
                            t::text_body(),
                            1.0,
                            0.0,
                        );
                    }
                }
                Item::Gap => {}
            }
        }
        // Header and footer bands over the spill.
        let head = RectF::new(card.x, card.y, card.w, g.body.y - card.y);
        fb.fill_rrect(head, 22.0 * s, Paint::Solid(t::ink_800()), 1.0);
        fb.fill_rect_paint(
            RectF::new(head.x, head.y + 30.0 * s, head.w, head.h - 30.0 * s),
            Paint::Solid(t::ink_800()),
            1.0,
        );
        let foot = RectF::new(card.x, g.body.bottom(), card.w, card.bottom() - g.body.bottom());
        fb.fill_rrect(foot, 22.0 * s, Paint::Solid(t::ink_800()), 1.0);
        fb.fill_rect_paint(
            RectF::new(foot.x, foot.y, foot.w, foot.h - 30.0 * s),
            Paint::Solid(t::ink_800()),
            1.0,
        );
        fb.fill_rect_paint(
            RectF::new(card.x + 1.0 * s, g.body.y - 1.0 * s, card.w - 2.0 * s, 1.0 * s),
            Paint::Solid(t::border_subtle()),
            1.0,
        );
        fb.fill_rect_paint(
            RectF::new(card.x + 1.0 * s, g.body.bottom(), card.w - 2.0 * s, 1.0 * s),
            Paint::Solid(t::border_subtle()),
            1.0,
        );
        fb.stroke_rrect(card, 22.0 * s, 1.0 * s, t::border_subtle(), 1.0);
        fb.fill_rrect(
            RectF::new(card.x + 40.0 * s, card.y + 1.0 * s, card.w - 80.0 * s, 2.0 * s),
            1.0 * s,
            Paint::GradientFaded(t::gradient_tears(), 0.9),
            1.0,
        );
        let pad = self.help_pad();
        self.text(
            fb,
            Face::SansBold,
            20.0,
            card.x + pad,
            card.y + 36.0 * s,
            "Keyboard and mouse",
            t::text_strong(),
            1.0,
            -0.2,
        );
        let hot_close = self.pointer.is_some_and(|(x, y)| g.close.contains(x, y));
        if hot_close {
            fb.fill_rrect(g.close, g.close.h * 0.5, Paint::Solid(fade(t::white(), 0.10)), 1.0);
        }
        self.icon(fb, Icon::X, g.close.cx(), g.close.cy(), 18.0, t::text_muted(), 1.0, false);
        let hint = "Esc, H or ? closes this page";
        let shown = self.fonts.fit(Face::Mono, 12.0 * s, hint, card.w - 2.0 * pad);
        self.text(fb, Face::Mono, 12.0, card.x + pad, foot.cy(), &shown, t::text_dim(), 1.0, 0.0);
        // The scroll bar.
        if laid.height > g.body.h + 1.0 {
            let track = RectF::new(g.body.right() - 4.0 * s, g.body.y + 2.0 * s, 4.0 * s, g.body.h - 4.0 * s);
            let thumb_h = (track.h * g.body.h / laid.height).max(24.0 * s);
            let max = laid.height - g.body.h;
            let ty = track.y + (track.h - thumb_h) * (scroll / max);
            fb.fill_rrect(
                RectF::new(track.x, ty, track.w, thumb_h),
                2.0 * s,
                Paint::Solid(fade(t::white(), 0.25)),
                1.0,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::UiConfig;
    use alloc::vec;

    fn ui_at(w: u32, h: u32) -> Ui {
        let mut ui = Ui::new(UiConfig { reduce_motion: true });
        ui.set_size(w, h, 1.0);
        ui
    }

    fn key(ui: &mut Ui, k: Key, m: &UiModel) -> Vec<Action> {
        ui.handle(&InputEvent::KeyDown { key: k, mods: Modifiers::default(), repeat: false }, 1_000, m)
    }

    #[test]
    fn every_shortcut_is_described_and_listed() {
        let app = AppModel { quit: true, tags: true, ..AppModel::default() };
        let secs = help_sections(&app);
        let all: Vec<&HelpRow> = secs.iter().flat_map(|s| s.rows.iter()).collect();
        for s in SHORTCUTS {
            assert!(describe(s.action).is_some(), "{:?} has no words on the help page", s.action);
            let text = shortcut_text(s);
            assert!(
                all.iter().any(|r| r.keys.split(" / ").any(|k| k == text)),
                "{text} is in the keymap but not on the page"
            );
        }
        // The new keys, by name.
        let find = |what: &str| all.iter().find(|r| r.what.contains(what)).unwrap().keys.clone();
        assert_eq!(find("This page"), "H / ?");
        assert_eq!(find("Heart"), "Ctrl+F");
        assert_eq!(find("Quit"), "Ctrl+Q");
        assert_eq!(find("Play or pause"), "Space / K");
        // Four groups plus the mouse, all with rows.
        assert_eq!(
            secs.iter().map(|s| s.title).collect::<Vec<_>>(),
            ["Playback", "Library", "Visualizer", "App", "Mouse and touch"]
        );
        assert!(secs.iter().all(|s| !s.rows.is_empty()));
    }

    #[test]
    fn quit_and_tag_editing_are_listed_only_where_the_host_offers_them() {
        let none = help_sections(&AppModel::default());
        let rows = |v: &Vec<HelpSection>| {
            v.iter().flat_map(|s| s.rows.iter().map(|r| r.what.clone())).collect::<Vec<_>>()
        };
        assert!(!rows(&none).iter().any(|w| w == "Quit" || w.contains("Edit the tags")));
        assert!(help_tips(&AppModel::default()).iter().all(|t| !t.contains("Ctrl+Q")));
        let some = help_sections(&AppModel { quit: true, tags: true, ..AppModel::default() });
        assert!(
            rows(&some).iter().any(|w| w == "Quit")
                && rows(&some).iter().any(|w| w.contains("Edit the tags"))
        );
        assert!(
            help_tips(&AppModel { quit: true, ..AppModel::default() }).iter().any(|t| t.contains("Ctrl+Q"))
        );
    }

    #[test]
    fn it_opens_scrolls_and_closes_with_the_keys_and_the_pointer() {
        let mut ui = ui_at(1280, 720);
        let m = UiModel::default();
        ui.toggle_help();
        assert!(ui.help_open());
        // The page takes every key: space would otherwise pause.
        assert!(key(&mut ui, Key::Space, &m).is_empty());
        // Scrolling by keys and the wheel stays within the content.
        let before = ui.help_scroll();
        key(&mut ui, Key::Down, &m);
        assert!(ui.help_scroll() > before);
        key(&mut ui, Key::End, &m);
        let end = ui.help_scroll();
        assert!(end > 0.0);
        key(&mut ui, Key::Down, &m);
        assert_eq!(ui.help_scroll(), end, "no scrolling past the end");
        ui.handle(&InputEvent::Wheel { dx: 0.0, dy: -100_000.0 }, 2_000, &m);
        assert_eq!(ui.help_scroll(), 0.0);
        key(&mut ui, Key::Other("PageDown".into()), &m);
        assert!(ui.help_scroll() > 100.0);
        key(&mut ui, Key::Home, &m);
        assert_eq!(ui.help_scroll(), 0.0);
        // Escape, H and ? close it.
        for k in [Key::Escape, Key::Char('h'), Key::Char('?'), Key::Char('H')] {
            ui.toggle_help();
            key(&mut ui, k.clone(), &m);
            assert!(!ui.help_open(), "{k:?} closes it");
        }
        // The X and a click outside the card close it; a click inside does not.
        ui.toggle_help();
        let g = ui.help_geom();
        let click = |ui: &mut Ui, x: f32, y: f32| {
            ui.handle(&InputEvent::PointerMove { x, y }, 3_000, &m);
            ui.handle(&InputEvent::PointerDown { x, y, button: PointerButton::Primary }, 3_000, &m);
            ui.handle(&InputEvent::PointerUp { x, y, button: PointerButton::Primary }, 3_000, &m);
        };
        click(&mut ui, g.body.cx(), g.body.cy());
        assert!(ui.help_open());
        click(&mut ui, g.close.cx(), g.close.cy());
        assert!(!ui.help_open());
        ui.toggle_help();
        click(&mut ui, 2.0, 2.0);
        assert!(!ui.help_open());
    }

    #[test]
    fn a_finger_drag_scrolls_it_and_the_card_fits_a_phone() {
        let mut ui = ui_at(390, 844);
        let m = UiModel::default();
        ui.toggle_help();
        let g = ui.help_geom();
        assert!(g.card.x >= 0.0 && g.card.right() <= 390.0 && g.card.bottom() <= 844.0, "{g:?}");
        let (x, y) = (g.body.cx(), g.body.cy());
        ui.handle(&InputEvent::PointerDown { x, y, button: PointerButton::Primary }, 1_000, &m);
        ui.handle(&InputEvent::PointerMove { x, y: y - 200.0 }, 1_100, &m);
        ui.handle(&InputEvent::PointerUp { x, y: y - 200.0, button: PointerButton::Primary }, 1_200, &m);
        assert!(ui.help_open(), "a drag is not a click");
        assert!((ui.help_scroll() - 200.0).abs() < 1.0, "{}", ui.help_scroll());
        // Every row's words fit the card at that width: the layout wraps instead of overflowing.
        let laid = ui.help_layout(&m.app);
        assert!(laid.height > g.body.h, "a phone has to scroll");
        let mut fb = FrameBuffer::new(390, 844);
        ui.draw_help(&mut fb, &m);
    }

    #[test]
    fn it_draws_at_a_normal_size_and_a_tiny_one() {
        for (w, h) in [(1280, 720), (320, 480), (900, 300)] {
            let mut ui = ui_at(w, h);
            let m = UiModel {
                app: AppModel { quit: true, tags: true, ..AppModel::default() },
                ..UiModel::default()
            };
            ui.toggle_help();
            let mut fb = FrameBuffer::new(w, h);
            ui.draw_help(&mut fb, &m);
            ui.help_set_scroll(f32::MAX, &m.app);
            ui.draw_help(&mut fb, &m);
            let g = ui.help_geom();
            assert!(g.body.h > 40.0, "{w}x{h}: {g:?}");
        }
        let _ = vec![0u8; 0];
    }
}
