//! Everything the user can ask the player to do, the keyboard map, and the context menu that mirrors it.
//!
//! Rusty Bucket requires full keyboard *and* full pointer control, so every shortcut has a menu entry. The
//! tests at the bottom enforce that against the tables here.
use crate::lib_ui::{Detail, LibAction, Mode, View};
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
    /// Step one frame forward (+1) or back (-1) and stay paused.
    FrameStep(i8),
    /// The loop key: first press marks A, second marks B (the loop starts), third clears.
    LoopMark,
    /// Mark the loop start at the current position.
    SetLoopA,
    /// Mark the loop end at the current position.
    SetLoopB,
    /// Clear the A-B loop.
    ClearLoop,
    /// The next playlist item.
    Next,
    /// The previous playlist item (or the start of this one, when it has played for a while).
    Prev,
    /// Off, then repeat all, then repeat one.
    CycleRepeat,
    /// Shuffle on or off.
    ToggleShuffle,
    /// Play a playlist item.
    PlayItem(u32),
    /// Remove a playlist item.
    RemoveItem(u32),
    /// Move a playlist item earlier (-1) or later (+1).
    MoveItem(u32, i8),
    /// Empty the playlist.
    ClearPlaylist,
    /// Pick more files to add to the playlist.
    AddFiles,
    /// Show the playlist menu.
    ShowPlaylist,
    /// Seek to an absolute position, microseconds (a chapter).
    SeekAbs(i64),
    /// The next chapter (+1) or the start of this one and then the previous one (-1).
    ChapterStep(i8),
    /// Switch to the Library or the Player face of the app.
    SetMode(Mode),
    /// Switch between the two.
    ToggleMode,
    /// The music-note button of the player bar: the Library face, on a music view (Albums when it was last on Videos or About). What is
    /// playing keeps playing.
    ShowMusic,
    /// Show a view of the library (this switches to the Library face).
    ShowView(View),
    /// Open the visualizer, or leave it for the view it was opened from.
    ToggleVisualizer,
    /// Open an album, an artist or a playlist.
    OpenDetail(Detail),
    /// Go back one step in the library.
    GoBack,
    /// An action of the library (play, queue, playlists, folders, visualizer).
    Lib(LibAction),
    /// Look for a newer version (and show what was found).
    CheckForUpdates,
    /// Add the app to the desktop's app menu, or take it out again.
    ToggleIntegration,
    /// Press button `n` of the application's dialog.
    DialogButton(u8),
    /// Flip switch `n` of the application's dialog.
    DialogToggle(u8),
    /// Close the application's dialog (Escape, the X).
    DialogClose,
    /// The dialog's Back control (or Escape, or Backspace) went up one level.
    DialogBack,
    /// The audio panel's Back control went up to the Settings dialog it was opened from.
    AudioBack,
    /// A character typed into the text box of the application's dialog.
    DialogChar(char),
    /// Backspace in the text box of the application's dialog.
    DialogBackspace,
    /// Heart or un-heart what is playing (or, in the library, the selected song or video).
    ToggleFavorite,
    /// Show the keyboard and mouse reference (the Help overlay), or hide it.
    ShowHelp,
    /// End the app (Ctrl+Q, Cmd+Q on a Mac): only where the host has a window of its own to close.
    Quit,
    /// Show the Settings dialog (theme, default player, app menu, updates, audio).
    ShowSettings,
    /// Show the Audio settings panel (crossfade and automatic level).
    ShowAudioSettings,
    /// Crossfade on or off.
    SetCrossfade(bool),
    /// The length of the crossfade, seconds.
    SetCrossfadeSecs(u8),
    /// Automatic level on or off.
    SetAutoLevel(bool),
    /// The loudness the automatic level aims at, LUFS.
    SetTargetLufs(i8),
    /// Level each track on its own, or whole albums.
    SetLevelMode(rvp_core::LevelMode),
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
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
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
    sc(ShortKey::Char('.'), Action::FrameStep(1)),
    sc(ShortKey::Char(','), Action::FrameStep(-1)),
    sc(ShortKey::Char('i'), Action::LoopMark),
    sc(ShortKey::Char('n'), Action::Next),
    sc(ShortKey::Char('p'), Action::Prev),
    sc(ShortKey::Char('r'), Action::CycleRepeat),
    sc(ShortKey::Char('z'), Action::ToggleShuffle),
    sc(ShortKey::Char('q'), Action::ShowPlaylist),
    sc(ShortKey::PageDown, Action::ChapterStep(1)),
    sc(ShortKey::PageUp, Action::ChapterStep(-1)),
    sc(ShortKey::Char('b'), Action::ToggleMode),
    sc(ShortKey::Char('v'), Action::ToggleVisualizer),
    sc(ShortKey::Char('u'), Action::ShowAudioSettings),
    sc(ShortKey::Char('h'), Action::ShowHelp),
    sc(ShortKey::Char('?'), Action::ShowHelp),
    // Ctrl+F hearts what is playing (the selected song or video in the library); search is `/`.
    Shortcut { key: ShortKey::Char('f'), shift: false, ctrl: true, action: Action::ToggleFavorite },
    // Ctrl+Q ends the app where the host offers that (Cmd+Q reaches the UI as Ctrl+Q: the desktop host maps it).
    Shortcut { key: ShortKey::Char('q'), shift: false, ctrl: true, action: Action::Quit },
    Shortcut { key: ShortKey::Char(','), shift: false, ctrl: true, action: Action::ShowSettings },
    // Ctrl+arrows keep seeking and the volume where the plain arrows move around lists in the library.
    Shortcut { key: ShortKey::Left, shift: false, ctrl: true, action: Action::SeekBy(-5_000) },
    Shortcut { key: ShortKey::Right, shift: false, ctrl: true, action: Action::SeekBy(5_000) },
    Shortcut { key: ShortKey::Up, shift: false, ctrl: true, action: Action::VolumeBy(5) },
    Shortcut { key: ShortKey::Down, shift: false, ctrl: true, action: Action::VolumeBy(-5) },
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
        Key::Other(n) if n == "PageUp" => ShortKey::PageUp,
        Key::Other(n) if n == "PageDown" => ShortKey::PageDown,
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
        ShortKey::PageUp => "Page Up".to_string(),
        ShortKey::PageDown => "Page Down".to_string(),
        ShortKey::Char(c) => c.to_ascii_uppercase().to_string(),
    }
}

/// The shortcut text for an action, for example `Space / K`. Arrow glyphs are not in the bundled font
/// subset, so they are spelled out.
pub fn shortcut_label(action: Action) -> String {
    let mut parts: Vec<String> = Vec::new();
    // Actions with only a Ctrl combination (hearting, quitting) show that.
    let only_ctrl = !SHORTCUTS.iter().any(|s| s.action == action && !s.ctrl);
    for s in SHORTCUTS.iter().filter(|s| s.action == action && (!s.ctrl || only_ctrl)) {
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
        if s.ctrl {
            name = alloc::format!("Ctrl+{name}");
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
    pub(crate) fn act(label: &str, action: Action) -> Self {
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

    pub(crate) fn heading(label: &str) -> Self {
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

    pub(crate) fn parent(label: &str, sub: Vec<MenuItem>) -> Self {
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

    pub(crate) fn sep(mut self) -> Self {
        self.separator = true;
        self
    }

    pub(crate) fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    pub(crate) fn enabled(mut self, on: bool) -> Self {
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

/// The chapter submenu: next and previous, then the chapters around the current one.
pub fn chapter_menu(model: &UiModel) -> Vec<MenuItem> {
    let n = model.chapters.len();
    let mut v = alloc::vec![
        MenuItem::act("Next chapter", Action::ChapterStep(1)).enabled(n > 0),
        MenuItem::act("Previous chapter", Action::ChapterStep(-1)).enabled(n > 0),
    ];
    if n > 0 {
        let cur = model.chapters.iter().rposition(|c| c.start_us <= model.position_us + 500_000).unwrap_or(0);
        let first = cur.saturating_sub(PLAYLIST_ROWS / 3).min(n.saturating_sub(PLAYLIST_ROWS));
        let last = (first + PLAYLIST_ROWS).min(n);
        for (i, c) in model.chapters[first..last].iter().enumerate() {
            let label = if c.title.is_empty() {
                alloc::format!("Chapter {}", first + i + 1)
            } else {
                alloc::format!("{}  {}", crate::model::format_time(c.start_us), c.title)
            };
            let mut m = MenuItem::act(&label, Action::SeekAbs(c.start_us)).checked(first + i == cur);
            m.hint.clear();
            if i == 0 {
                m.separator = true;
            }
            v.push(m);
        }
    }
    v
}

/// Playlist rows shown at most (the list scrolls by showing the part around the current item).
const PLAYLIST_ROWS: usize = 12;

/// The playlist menu: the items (click to play), then what can be done with the list.
pub fn playlist_menu(model: &UiModel) -> Vec<MenuItem> {
    let mut v: Vec<MenuItem> = Vec::new();
    let n = model.playlist.len();
    if n == 0 {
        v.push(MenuItem::act("The playlist is empty", Action::AddFiles).enabled(false));
    } else {
        let cur = model.playlist.iter().position(|e| e.current).unwrap_or(0);
        let first = cur.saturating_sub(PLAYLIST_ROWS / 3).min(n.saturating_sub(PLAYLIST_ROWS));
        let last = (first + PLAYLIST_ROWS).min(n);
        if first > 0 {
            v.push(MenuItem::heading(&alloc::format!("{first} earlier")));
        }
        for e in &model.playlist[first..last] {
            let mut m = MenuItem::act(&e.label, Action::PlayItem(e.id)).checked(e.current);
            m.hint.clear();
            v.push(m);
        }
        if last < n {
            v.push(MenuItem::heading(&alloc::format!("{} more", n - last)));
        }
    }
    let cur_id = model.playlist.iter().find(|e| e.current).map(|e| e.id);
    let has_cur = cur_id.is_some();
    let id = cur_id.unwrap_or(0);
    v.push(MenuItem::act("Add files\u{2026}", Action::AddFiles).sep());
    v.push(MenuItem::act("Move current up", Action::MoveItem(id, -1)).enabled(has_cur));
    v.push(MenuItem::act("Move current down", Action::MoveItem(id, 1)).enabled(has_cur));
    v.push(MenuItem::act("Remove current", Action::RemoveItem(id)).enabled(has_cur));
    v.push(MenuItem::act("Clear playlist", Action::ClearPlaylist).enabled(n > 0));
    v.push(MenuItem::act(model.repeat_label(), Action::CycleRepeat).sep());
    v.push(MenuItem::act(model.shuffle_label(), Action::ToggleShuffle));
    v
}

/// The audio effects submenu: the panel, and every setting of it as menu entries (so a menu alone can set them all).
pub fn audio_effects_menu(model: &UiModel) -> Vec<MenuItem> {
    use rvp_core::settings::{
        CROSSFADE_MAX_SECS, CROSSFADE_MIN_SECS, LevelMode, TARGET_MAX_LUFS, TARGET_MIN_LUFS,
    };
    let a = model.audio;
    let plain = |label: &str, action: Action| {
        let mut m = MenuItem::act(label, action);
        m.hint.clear();
        m
    };
    let lengths: Vec<MenuItem> = (CROSSFADE_MIN_SECS..=CROSSFADE_MAX_SECS)
        .map(|s| plain(&alloc::format!("{s} s"), Action::SetCrossfadeSecs(s)).checked(a.crossfade_secs == s))
        .collect();
    // Loudest first: -10 down to -23.
    let targets: Vec<MenuItem> = (TARGET_MIN_LUFS..=TARGET_MAX_LUFS)
        .rev()
        .map(|l| plain(&alloc::format!("{l} LUFS"), Action::SetTargetLufs(l)).checked(a.target_lufs == l))
        .collect();
    alloc::vec![
        MenuItem::act("Audio settings\u{2026}", Action::ShowAudioSettings),
        plain(
            if a.crossfade { "Crossfade: on" } else { "Crossfade: off" },
            Action::SetCrossfade(!a.crossfade)
        )
        .sep(),
        MenuItem::parent("Crossfade length", lengths),
        plain(
            if a.auto_level { "Auto-level: on" } else { "Auto-level: off" },
            Action::SetAutoLevel(!a.auto_level)
        )
        .sep(),
        MenuItem::parent("Target level", targets),
        plain("Level each track", Action::SetLevelMode(LevelMode::Track))
            .checked(a.level_mode == LevelMode::Track),
        plain("Level whole albums", Action::SetLevelMode(LevelMode::Album))
            .checked(a.level_mode == LevelMode::Album),
    ]
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
        MenuItem::act("Next frame", Action::FrameStep(1)).sep(),
        MenuItem::act("Previous frame", Action::FrameStep(-1)),
    ];
    let loop_state = if model.loop_b.is_some() {
        "Loop: on"
    } else if model.loop_a.is_some() {
        "Loop: A set"
    } else {
        "Loop: off"
    };
    let looping = alloc::vec![
        MenuItem::act(loop_state, Action::LoopMark),
        MenuItem::act("Set start (A)", Action::SetLoopA).sep(),
        MenuItem::act("Set end (B)", Action::SetLoopB),
        MenuItem::act("Clear loop", Action::ClearLoop).enabled(model.loop_a.is_some()).sep(),
    ];
    let volume = alloc::vec![
        MenuItem::act("Louder", Action::VolumeBy(5)),
        MenuItem::act("Quieter", Action::VolumeBy(-5)),
        MenuItem::act(if model.muted { "Unmute" } else { "Mute" }, Action::ToggleMute).sep(),
    ];
    let mut v = alloc::vec![
        MenuItem::act("Open file\u{2026}", Action::OpenFile),
        play.sep(),
        MenuItem::parent("Seek", seek).enabled(has),
        MenuItem::parent("A-B loop", looping).enabled(has),
        MenuItem::parent("Speed", speed_menu(model)).enabled(has),
        MenuItem::parent("Volume", volume),
        MenuItem::parent("Audio effects", audio_effects_menu(model)),
        MenuItem::parent("Audio track", audio_menu(model)).sep().enabled(has),
        MenuItem::parent("Subtitles", subtitle_menu(model)).enabled(has),
        MenuItem::parent("Chapters", chapter_menu(model)).enabled(!model.chapters.is_empty()),
        MenuItem::act("Library", Action::ToggleMode).sep(),
        MenuItem::act("Visualizer", Action::ToggleVisualizer),
        MenuItem::act("Show playlist", Action::ShowPlaylist),
        MenuItem::parent("Playlist", {
            let mut v = alloc::vec![
                MenuItem::act("Next", Action::Next).enabled(has),
                MenuItem::act("Previous", Action::Prev).enabled(has)
            ];
            v.extend(playlist_menu(model).into_iter().map(|mut m| {
                m.separator = false;
                m
            }));
            v
        }),
        {
            let mut m = MenuItem::act(
                if model.now_favorite { "Remove from favorites" } else { "Add to favorites" },
                Action::ToggleFavorite,
            )
            .enabled(model.now_track.is_some());
            m.hint = shortcut_label(Action::ToggleFavorite);
            m
        },
        MenuItem::act(
            if model.fullscreen { "Leave fullscreen" } else { "Fullscreen" },
            Action::ToggleFullscreen
        )
        .sep(),
    ];
    v.push(MenuItem::act("Keyboard shortcuts", Action::ShowHelp).sep());
    v.push(MenuItem::act("Settings\u{2026}", Action::ShowSettings));
    // What the host offers beyond playback: only where it applies.
    if model.app.updates {
        v.push(MenuItem::act("Check for updates\u{2026}", Action::CheckForUpdates).sep());
    }
    if let Some(on) = model.app.integration {
        let label = if on { "Remove from the app menu" } else { "Add to the app menu" };
        let mut m = MenuItem::act(label, Action::ToggleIntegration);
        m.separator = !model.app.updates;
        v.push(m);
    }
    if model.app.quit {
        v.push(MenuItem::act("Quit", Action::Quit).sep());
    }
    v
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
        let mut m = model();
        m.app.quit = true;
        menu_actions(&context_menu(&m), &mut reachable);
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
    fn every_audio_setting_can_be_set_from_the_menu_alone() {
        let mut reachable = Vec::new();
        menu_actions(&context_menu(&model()), &mut reachable);
        use rvp_core::LevelMode;
        assert!(reachable.contains(&Action::ShowAudioSettings));
        // The two switches, every length 2 to 10 s, every target -23 to -10 LUFS, and both modes.
        assert!(
            reachable.contains(&Action::SetCrossfade(true))
                && reachable.contains(&Action::SetAutoLevel(true))
        );
        for secs in 2..=10 {
            assert!(reachable.contains(&Action::SetCrossfadeSecs(secs)), "{secs} s");
        }
        for lufs in -23..=-10 {
            assert!(reachable.contains(&Action::SetTargetLufs(lufs)), "{lufs} LUFS");
        }
        assert!(reachable.contains(&Action::SetLevelMode(LevelMode::Track)));
        assert!(reachable.contains(&Action::SetLevelMode(LevelMode::Album)));
        // The menu shows what is set: the switches say on or off and the chosen values are checked.
        let mut m = model();
        m.audio = rvp_core::AudioSettings {
            crossfade: true,
            crossfade_secs: 7,
            auto_level: true,
            target_lufs: -18,
            level_mode: LevelMode::Album,
        };
        let items = audio_effects_menu(&m);
        let by = |label: &str| items.iter().find(|i| i.label == label).unwrap();
        assert_eq!(by("Crossfade: on").action, Some(Action::SetCrossfade(false)));
        assert_eq!(by("Auto-level: on").action, Some(Action::SetAutoLevel(false)));
        assert!(by("Level whole albums").checked && !by("Level each track").checked);
        let checked = |label: &str| {
            by(label).sub.iter().filter(|i| i.checked).map(|i| i.label.clone()).collect::<Vec<_>>()
        };
        assert_eq!(checked("Crossfade length"), ["7 s"]);
        assert_eq!(checked("Target level"), ["-18 LUFS"]);
        assert_eq!(shortcut_for(&Key::Char('u'), &Modifiers::default()), Some(Action::ShowAudioSettings));
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
        // Help is H and ?, the heart is Ctrl+F (plain F stays fullscreen), Ctrl+Q quits.
        assert_eq!(shortcut_for(&Key::Char('h'), &none), Some(Action::ShowHelp));
        assert_eq!(shortcut_for(&Key::Char('?'), &none), Some(Action::ShowHelp));
        assert_eq!(shortcut_for(&Key::Char('?'), &shift), Some(Action::ShowHelp));
        assert_eq!(shortcut_for(&Key::Char('f'), &none), Some(Action::ToggleFullscreen));
        assert_eq!(shortcut_for(&Key::Char('f'), &ctrl), Some(Action::ToggleFavorite));
        assert_eq!(shortcut_for(&Key::Char('F'), &ctrl), Some(Action::ToggleFavorite));
        assert_eq!(shortcut_for(&Key::Char('q'), &ctrl), Some(Action::Quit));
        assert_eq!(shortcut_for(&Key::Char('q'), &none), Some(Action::ShowPlaylist));
        // Favoriting has no plain key any more.
        assert!(SHORTCUTS.iter().filter(|s| s.action == Action::ToggleFavorite).all(|s| s.ctrl));
    }

    #[test]
    fn labels_and_speeds() {
        assert_eq!(shortcut_label(Action::PlayPause), "Space / K");
        assert_eq!(shortcut_label(Action::SeekBy(-5_000)), "Left");
        assert_eq!(shortcut_label(Action::SeekBy(30_000)), "Shift+Right");
        assert_eq!(shortcut_label(Action::ToggleFavorite), "Ctrl+F");
        assert_eq!(shortcut_label(Action::Quit), "Ctrl+Q");
        assert_eq!(shortcut_label(Action::ShowHelp), "H / ?");
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
