//! Everything the user can ask the player to do, the keyboard map, and the context menu that mirrors it.
//!
//! Rusty Bucket requires full keyboard *and* full pointer control, so every shortcut has a menu entry. The
//! tests at the bottom enforce that against the tables here.
use crate::model::UiModel;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_host::{Key, Modifiers};

/// Playback speeds offered by the menus, slowest first.
pub const SPEEDS: [f32; 8] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 4.0];

/// Something the user asked for. The application applies it to the player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Ask the host for a file (picker).
    OpenFile,
    /// Toggle play and pause.
    PlayPause,
    /// Seek relative to the current position, in milliseconds.
    SeekBy(i32),
    /// Seek to a fraction (0.0..=1.0) of the duration (the seek bar).
    SeekFraction(f32),
    /// Seek to the start.
    SeekStart,
    /// Seek to the end.
    SeekEnd,
    /// Change the volume by a percentage-point step.
    VolumeBy(i8),
    /// Set the volume (the slider), 0.0..=1.0.
    SetVolume(f32),
    /// Mute or unmute.
    ToggleMute,
    /// Enter or leave fullscreen.
    ToggleFullscreen,
    /// One step along [`SPEEDS`].
    SpeedStep(i8),
    /// Set a speed.
    SetSpeed(f32),
    /// Back to 1x.
    ResetSpeed,
    /// Next audio track.
    CycleAudio,
    /// Next subtitle track, then off.
    CycleSubtitles,
    /// A specific audio track.
    SelectAudio(u32),
    /// A specific subtitle track, or off.
    SelectSubtitle(Option<u32>),
}

/// The physical key of a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortKey {
    /// Space bar.
    Space,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Home.
    Home,
    /// End.
    End,
    /// A character key (lower case).
    Char(char),
}

/// One keyboard shortcut.
#[derive(Debug, Clone, Copy)]
pub struct Shortcut {
    /// The key.
    pub key: ShortKey,
    /// Shift must be held (arrow keys only; letters ignore Shift).
    pub shift: bool,
    /// Ctrl must be held.
    pub ctrl: bool,
    /// What it does.
    pub action: Action,
}

const fn sc(key: ShortKey, action: Action) -> Shortcut {
    Shortcut { key, shift: false, ctrl: false, action }
}

/// The keyboard map. Q2 (step sizes): arrows 5 s, Shift+arrows 30 s, `J`/`L` 10 s (reverse shuttle needs
/// reverse decode, which is post-v1, so `J`/`L` seek like most web players do).
pub const SHORTCUTS: &[Shortcut] = &[
    sc(ShortKey::Space, Action::PlayPause),
    sc(ShortKey::Char('k'), Action::PlayPause),
    sc(ShortKey::Left, Action::SeekBy(-5_000)),
    sc(ShortKey::Right, Action::SeekBy(5_000)),
    Shortcut { key: ShortKey::Left, shift: true, ctrl: false, action: Action::SeekBy(-30_000) },
    Shortcut { key: ShortKey::Right, shift: true, ctrl: false, action: Action::SeekBy(30_000) },
    sc(ShortKey::Char('j'), Action::SeekBy(-10_000)),
    sc(ShortKey::Char('l'), Action::SeekBy(10_000)),
    sc(ShortKey::Home, Action::SeekStart),
    sc(ShortKey::End, Action::SeekEnd),
    sc(ShortKey::Up, Action::VolumeBy(5)),
    sc(ShortKey::Down, Action::VolumeBy(-5)),
    sc(ShortKey::Char('m'), Action::ToggleMute),
    sc(ShortKey::Char('f'), Action::ToggleFullscreen),
    sc(ShortKey::Char('o'), Action::OpenFile),
    Shortcut { key: ShortKey::Char('o'), shift: false, ctrl: true, action: Action::OpenFile },
    sc(ShortKey::Char(']'), Action::SpeedStep(1)),
    sc(ShortKey::Char('['), Action::SpeedStep(-1)),
    sc(ShortKey::Char('\\'), Action::ResetSpeed),
    sc(ShortKey::Char('a'), Action::CycleAudio),
    sc(ShortKey::Char('s'), Action::CycleSubtitles),
];

/// The action bound to a key press, if any. Browser-style combinations (Ctrl, Alt or Meta with a letter)
/// are never claimed except the ones listed, so the host can leave them to the browser.
pub fn shortcut_for(key: &Key, mods: &Modifiers) -> Option<Action> {
    if mods.alt || mods.logo {
        return None;
    }
    let k = match key {
        Key::Space => ShortKey::Space,
        Key::Left => ShortKey::Left,
        Key::Right => ShortKey::Right,
        Key::Up => ShortKey::Up,
        Key::Down => ShortKey::Down,
        Key::Home => ShortKey::Home,
        Key::End => ShortKey::End,
        Key::Char(c) => ShortKey::Char(c.to_ascii_lowercase()),
        _ => return None,
    };
    let arrow = matches!(k, ShortKey::Left | ShortKey::Right);
    SHORTCUTS
        .iter()
        .find(|s| s.key == k && s.ctrl == mods.ctrl && (!arrow || s.shift == mods.shift))
        .map(|s| s.action)
}

fn key_name(k: ShortKey) -> String {
    match k {
        ShortKey::Space => "Space".to_string(),
        ShortKey::Left => "\u{2190}".to_string(),
        ShortKey::Right => "\u{2192}".to_string(),
        ShortKey::Up => "\u{2191}".to_string(),
        ShortKey::Down => "\u{2193}".to_string(),
        ShortKey::Home => "Home".to_string(),
        ShortKey::End => "End".to_string(),
        ShortKey::Char(c) => c.to_ascii_uppercase().to_string(),
    }
}

/// The shortcut text for an action, for example `Space / K`. Arrow glyphs are not in the bundled font
/// subset, so they are spelled out.
pub fn shortcut_label(action: Action) -> String {
    let mut parts: Vec<String> = Vec::new();
    for s in SHORTCUTS.iter().filter(|s| s.action == action && !s.ctrl) {
        let mut name = match s.key {
            ShortKey::Left => "Left".to_string(),
            ShortKey::Right => "Right".to_string(),
            ShortKey::Up => "Up".to_string(),
            ShortKey::Down => "Down".to_string(),
            k => key_name(k),
        };
        if s.shift {
            name = alloc::format!("Shift+{name}");
        }
        parts.push(name);
    }
    parts.join(" / ")
}

/// One row of a menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    /// Label.
    pub label: String,
    /// Shortcut text (right aligned), may be empty.
    pub hint: String,
    /// What it does; `None` for a submenu parent or a heading.
    pub action: Option<Action>,
    /// A check mark (selected speed, selected track).
    pub checked: bool,
    /// Greyed out when false.
    pub enabled: bool,
    /// A separator line is drawn above this row.
    pub separator: bool,
    /// A section heading (not selectable).
    pub heading: bool,
    /// Submenu rows.
    pub sub: Vec<MenuItem>,
}

impl MenuItem {
    fn act(label: &str, action: Action) -> Self {
        Self {
            label: label.to_string(),
            hint: shortcut_label(action),
            action: Some(action),
            checked: false,
            enabled: true,
            separator: false,
            heading: false,
            sub: Vec::new(),
        }
    }

    fn heading(label: &str) -> Self {
        Self {
            label: label.to_string(),
            hint: String::new(),
            action: None,
            checked: false,
            enabled: false,
            separator: false,
            heading: true,
            sub: Vec::new(),
        }
    }

    fn parent(label: &str, sub: Vec<MenuItem>) -> Self {
        Self {
            label: label.to_string(),
            hint: String::new(),
            action: None,
            checked: false,
            enabled: !sub.is_empty(),
            separator: false,
            heading: false,
            sub,
        }
    }

    fn sep(mut self) -> Self {
        self.separator = true;
        self
    }

    fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
}

/// Speed label: `1×`, `0.5×`, `1.25×`.
pub fn speed_label(r: f32) -> String {
    let hundredths = (r * 100.0 + 0.5) as i32;
    let (whole, frac) = (hundredths / 100, hundredths % 100);
    if frac == 0 {
        alloc::format!("{whole}\u{d7}")
    } else if frac % 10 == 0 {
        alloc::format!("{whole}.{}\u{d7}", frac / 10)
    } else {
        alloc::format!("{whole}.{frac:02}\u{d7}")
    }
}

/// The speed submenu.
pub fn speed_menu(model: &UiModel) -> Vec<MenuItem> {
    let mut v: Vec<MenuItem> = SPEEDS
        .iter()
        .map(|&r| MenuItem::act(&speed_label(r), Action::SetSpeed(r)).checked((model.rate - r).abs() < 0.001))
        .map(|mut m| {
            m.hint.clear();
            m
        })
        .collect();
    v.push(MenuItem::act("Faster", Action::SpeedStep(1)).sep());
    v.push(MenuItem::act("Slower", Action::SpeedStep(-1)));
    v.push(MenuItem::act("Normal speed", Action::ResetSpeed));
    v
}

/// The audio track submenu.
pub fn audio_menu(model: &UiModel) -> Vec<MenuItem> {
    let mut v: Vec<MenuItem> = model
        .audio_tracks
        .iter()
        .map(|t| {
            let mut m = MenuItem::act(&t.label, Action::SelectAudio(t.id))
                .checked(model.selected_audio == Some(t.id));
            m.hint.clear();
            m
        })
        .collect();
    if v.is_empty() {
        v.push(MenuItem::act("No audio tracks", Action::CycleAudio).enabled(false));
    } else {
        v.push(MenuItem::act("Next track", Action::CycleAudio).sep());
    }
    v
}

/// The subtitle submenu.
pub fn subtitle_menu(model: &UiModel) -> Vec<MenuItem> {
    let mut off =
        MenuItem::act("Off", Action::SelectSubtitle(None)).checked(model.selected_subtitle.is_none());
    off.hint.clear();
    let mut v = alloc::vec![off];
    for t in &model.subtitle_tracks {
        let mut m = MenuItem::act(&t.label, Action::SelectSubtitle(Some(t.id)))
            .checked(model.selected_subtitle == Some(t.id));
        m.hint.clear();
        v.push(m);
    }
    v.push(MenuItem::act("Next subtitle track", Action::CycleSubtitles).sep());
    v
}

/// Audio and subtitles in one panel (the transport bar's track button).
pub fn tracks_menu(model: &UiModel) -> Vec<MenuItem> {
    let mut v = alloc::vec![MenuItem::heading("Audio")];
    v.extend(audio_menu(model).into_iter().map(|mut m| {
        m.separator = false;
        m
    }));
    v.push(MenuItem::heading("Subtitles"));
    v.extend(subtitle_menu(model).into_iter().map(|mut m| {
        m.separator = false;
        m
    }));
    // A separator above each heading except the first.
    for (i, m) in v.iter_mut().enumerate() {
        m.separator = m.heading && i > 0;
    }
    v
}

/// The right-click menu. It contains an entry for every shortcut in [`SHORTCUTS`].
pub fn context_menu(model: &UiModel) -> Vec<MenuItem> {
    let has = model.has_media();
    let mut play =
        MenuItem::act(if model.state.is_active() { "Pause" } else { "Play" }, Action::PlayPause).enabled(has);
    play.hint = shortcut_label(Action::PlayPause);
    let seek = alloc::vec![
        MenuItem::act("Back 5 s", Action::SeekBy(-5_000)),
        MenuItem::act("Forward 5 s", Action::SeekBy(5_000)),
        MenuItem::act("Back 10 s", Action::SeekBy(-10_000)).sep(),
        MenuItem::act("Forward 10 s", Action::SeekBy(10_000)),
        MenuItem::act("Back 30 s", Action::SeekBy(-30_000)).sep(),
        MenuItem::act("Forward 30 s", Action::SeekBy(30_000)),
        MenuItem::act("To start", Action::SeekStart).sep(),
        MenuItem::act("To end", Action::SeekEnd),
    ];
    let volume = alloc::vec![
        MenuItem::act("Louder", Action::VolumeBy(5)),
        MenuItem::act("Quieter", Action::VolumeBy(-5)),
        MenuItem::act(if model.muted { "Unmute" } else { "Mute" }, Action::ToggleMute).sep(),
    ];
    alloc::vec![
        MenuItem::act("Open file\u{2026}", Action::OpenFile),
        play.sep(),
        MenuItem::parent("Seek", seek).enabled(has),
        MenuItem::parent("Speed", speed_menu(model)).enabled(has),
        MenuItem::parent("Volume", volume),
        MenuItem::parent("Audio track", audio_menu(model)).sep().enabled(has),
        MenuItem::parent("Subtitles", subtitle_menu(model)).enabled(has),
        MenuItem::act(
            if model.fullscreen { "Leave fullscreen" } else { "Fullscreen" },
            Action::ToggleFullscreen
        )
        .sep(),
    ]
}

/// All actions reachable from a menu, submenus included.
pub fn menu_actions(items: &[MenuItem], out: &mut Vec<Action>) {
    for i in items {
        if let Some(a) = i.action {
            out.push(a);
        }
        menu_actions(&i.sub, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MediaState, TrackItem};

    fn model() -> UiModel {
        UiModel {
            state: MediaState::Paused,
            rate: 1.0,
            audio_tracks: alloc::vec![TrackItem { id: 1, label: "English (opus)".to_string() }],
            ..UiModel::default()
        }
    }

    #[test]
    fn every_shortcut_has_a_context_menu_entry() {
        let mut reachable = Vec::new();
        menu_actions(&context_menu(&model()), &mut reachable);
        for s in SHORTCUTS {
            assert!(
                reachable.contains(&s.action),
                "{:?} -> {:?} is not in the context menu",
                s.key,
                s.action
            );
        }
    }

    #[test]
    fn keymap_resolves_keys() {
        let none = Modifiers::default();
        let shift = Modifiers { shift: true, ..Modifiers::default() };
        let ctrl = Modifiers { ctrl: true, ..Modifiers::default() };
        assert_eq!(shortcut_for(&Key::Space, &none), Some(Action::PlayPause));
        assert_eq!(shortcut_for(&Key::Char('K'), &none), Some(Action::PlayPause));
        assert_eq!(shortcut_for(&Key::Left, &none), Some(Action::SeekBy(-5_000)));
        assert_eq!(shortcut_for(&Key::Left, &shift), Some(Action::SeekBy(-30_000)));
        assert_eq!(shortcut_for(&Key::Char('L'), &shift), Some(Action::SeekBy(10_000)));
        assert_eq!(shortcut_for(&Key::Char('o'), &ctrl), Some(Action::OpenFile));
        assert_eq!(shortcut_for(&Key::Char('m'), &ctrl), None); // leave Ctrl+M to the browser
        assert_eq!(shortcut_for(&Key::Char('x'), &none), None);
        assert_eq!(shortcut_for(&Key::Char('f'), &Modifiers { alt: true, ..none }), None);
    }

    #[test]
    fn labels_and_speeds() {
        assert_eq!(shortcut_label(Action::PlayPause), "Space / K");
        assert_eq!(shortcut_label(Action::SeekBy(-5_000)), "Left");
        assert_eq!(shortcut_label(Action::SeekBy(30_000)), "Shift+Right");
        assert_eq!(speed_label(1.0), "1\u{d7}");
        assert_eq!(speed_label(0.25), "0.25\u{d7}");
        assert_eq!(speed_label(1.5), "1.5\u{d7}");
        let menu = speed_menu(&model());
        assert!(menu.iter().filter(|m| m.checked).count() == 1);
    }

    #[test]
    fn menus_follow_the_model() {
        let idle = UiModel::default();
        let m = context_menu(&idle);
        assert!(m[0].enabled && !m[1].enabled); // open is always there, play needs a file
        let t = tracks_menu(&model());
        assert!(t.iter().any(|i| i.label.contains("English")));
    }
}
