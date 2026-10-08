//! Tooltips: every control says what it does and, where it has one, its key. One place decides what a control's tooltip says
//! ([`lib_tip_text`] and friends are exhaustive over what can be pointed at, so a new control cannot be forgotten), one place draws it.
//!
//! A tooltip shows when the pointer rests on a control, or when the keyboard reaches it (the focus ring), after a short delay; it is
//! wrapped at 140 characters, kept inside the window and styled with the design system's tokens. The Settings switch "Show tooltips"
//! turns them all off.
use crate::actions::Action;
use crate::dialog::DialogControl;
use crate::font::Face;
use crate::gfx::{FrameBuffer, Paint, RectF, fade};
use crate::lib_ui::rows::EntKind;
use crate::lib_ui::{Detail, LibCtx, LibHit, Mode, View};
use crate::model::UiModel;
use crate::tk as t;
use crate::ui::{Btn, Target, Ui};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use theme::Rgba;

/// How long a tooltip line may be, characters.
pub const TIP_WIDTH_CHARS: usize = 140;

/// A tooltip: what it says, the key (empty if none), and the control it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Tip {
    /// What the control does.
    pub text: String,
    /// Its key, such as `Space` or `Shift+V`.
    pub key: String,
    /// Where the control is.
    pub anchor: RectF,
}

fn tip(text: impl Into<String>, key: &str) -> Option<(String, String)> {
    Some((text.into(), key.to_string()))
}

/// Wrap `text` into lines of at most `max` characters, at spaces.
pub fn wrap_chars(text: &str, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let extra = if cur.is_empty() { 0 } else { 1 };
        if !cur.is_empty() && cur.chars().count() + extra + word.chars().count() > max {
            lines.push(core::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
        // A single word longer than the line is cut.
        while cur.chars().count() > max {
            let cut: String = cur.chars().take(max).collect();
            cur = cur.chars().skip(max).collect();
            lines.push(cut);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// What a transport button does, for both faces.
pub(crate) fn btn_text(b: Btn, model: &UiModel, library: bool, view: View) -> Option<(String, String)> {
    match b {
        Btn::Play => tip(
            if model.state.is_active() {
                "Pause. The track or video stops where it is."
            } else {
                "Play. Carries on from where it stopped."
            },
            "Space",
        ),
        Btn::Back => tip("Go back 10 seconds.", "J"),
        Btn::Fwd => tip("Go forward 10 seconds.", "L"),
        Btn::Mute => tip(if model.muted { "Unmute the sound." } else { "Mute the sound." }, "M"),
        Btn::Speed => tip(
            "Playback speed, from a quarter to four times. [ and ] step it, \\ puts it back to normal.",
            "[ ]",
        ),
        Btn::Tracks => {
            tip("Choose the audio track and the subtitles. A switches audio, S switches subtitles.", "A / S")
        }
        Btn::Playlist => tip("The playlist: what is queued, and what plays next.", "Q"),
        Btn::Open => tip("Open a file to play.", "O"),
        Btn::Fullscreen => {
            tip(if model.fullscreen { "Leave fullscreen." } else { "Play the picture fullscreen." }, "F")
        }
        Btn::Welcome => tip("Choose a file to play, or drop one on this window.", "O"),
        Btn::Prev => {
            tip("The previous item. Pressed after a few seconds, it goes back to the start of this one.", "P")
        }
        Btn::Next => tip("The next item in the queue.", "N"),
        Btn::Shuffle => tip(
            if model.shuffle {
                "Shuffle is on: the queue plays in a random order. Click to play in order."
            } else {
                "Shuffle is off. Click to play the queue in a random order."
            },
            "Z",
        ),
        Btn::Repeat => tip(
            match model.repeat {
                1 => "Repeat all: the queue starts over when it ends. Click for repeat one.",
                2 => "Repeat one: this item plays again. Click to turn repeat off.",
                _ => "Repeat is off. Click to repeat the whole queue.",
            },
            "R",
        ),
        Btn::QueueView => tip("The queue: what plays next. Drag rows to reorder, Delete removes one.", "5"),
        Btn::VizView => tip(
            if view == View::Visualizer {
                "Leave the visualizer and go back."
            } else {
                "The visualizer: full-screen visuals that move with the music."
            },
            "V",
        ),
        Btn::ModeSwitch => tip(
            if library {
                "Switch to the player: the picture and its controls."
            } else {
                "Switch to the library."
            },
            "B",
        ),
        Btn::Favorite => tip(
            if model.now_favorite {
                "Remove what is playing from your favorites."
            } else {
                "Add what is playing to your favorites."
            },
            "Ctrl+F",
        ),
    }
}

fn view_text(v: View) -> Option<(String, String)> {
    match v {
        View::Search => tip(
            "Search your whole library: albums, artists, songs and videos. Accents and case do not matter.",
            "/",
        ),
        View::NowPlaying => tip("Now playing: the cover, the title and what comes next.", "6"),
        View::Albums => tip("Albums: your music as covers.", "1"),
        View::Artists => tip("Artists: who is in your library and what they made.", "2"),
        View::Tracks => {
            tip("Tracks: every song, sortable by title, artist, album, length or date added.", "3")
        }
        View::Videos => tip("Videos: your films and clips as posters, with a mark where you stopped.", "8"),
        View::Favorites => {
            tip("Favorites: the songs and videos you hearted, ready to play as one queue.", "9")
        }
        View::History => tip(
            "History: what you played, newest first, by day. Play, queue or heart a row; Delete takes it out of the history.",
            "0",
        ),
        View::Playlists => {
            tip("Playlists: the lists you saved. Import M3U, M3U8 and PLS files here too.", "4")
        }
        View::Queue => tip("Queue: what plays next. Drag rows to reorder them.", "5"),
        View::Visualizer => tip("The visualizer: full-screen visuals that move with the music.", "V"),
        View::About => tip("About Rusty Wave: who made it, the version, the links and the licenses.", "F1"),
    }
}

fn header_button_text(view: View, detail: Option<Detail>, id: u8) -> Option<(String, String)> {
    if let Some(d) = detail {
        return match id {
            0 => tip("Play from the first song.", "Enter"),
            1 => tip("Play in a random order.", ""),
            2 => tip("Add to the end of the queue.", "Shift+Enter"),
            3 => tip("Heart every song here, or take the hearts off if they all have one.", ""),
            4 => tip("Save this playlist as an M3U8 file.", ""),
            5 => tip("Save this playlist as a PLS file.", ""),
            6 => tip("Give this playlist another name.", ""),
            7 => {
                let _ = d;
                tip("Delete this playlist. The songs stay in your library.", "")
            }
            _ => None,
        };
    }
    match (view, id) {
        (View::Playlists, 0) => tip("Make a new, empty playlist.", ""),
        (View::Playlists, 1) => tip("Import an M3U, M3U8 or PLS playlist file.", ""),
        (View::Queue, 0) => tip("Empty the queue.", ""),
        (View::Queue, 1) => tip("Save the queue as a playlist.", ""),
        (View::Tracks | View::Albums, 0) => tip("Shuffle every song in your library and play.", ""),
        (View::Videos, 0) => tip("Show the videos as a list or as posters.", ""),
        (View::Videos, 1) => {
            tip("Change the order: by title, newest first, longest first, most played or last played.", "")
        }
        (View::History, 0) => tip("Forget every play. Your library, favorites and playlists stay.", ""),
        (View::Favorites, 0) => tip("Play every favorite, songs first, from the top.", ""),
        (View::Favorites, 1) => tip("Play every favorite in a random order.", ""),
        (View::About, 0) => tip("Open Rusty Bucket's site, rustybucket.ai, in your browser.", ""),
        (View::About, 1) => tip("Open DJ Unicorn Tears on X in your browser.", ""),
        (View::About, 2) => tip("Save the licenses of everything Rusty Wave is made of.", ""),
        (View::NowPlaying, 0) => tip("Go and pick an album to play.", ""),
        (_, 10) => tip("Add a folder of music or videos to your library. Files stay where they are.", ""),
        (_, 11) => tip("Open a file to play without adding it to the library.", ""),
        _ => None,
    }
}

fn tag_field_text(i: usize, ui: &Ui) -> Option<(String, String)> {
    let f = ui.lib.tagform.as_ref()?;
    let field = f.spec.fields.get(i)?.0;
    use crate::lib_ui::TagField as F;
    let what = match field {
        F::Title => "The title of the song.",
        F::Artist => "Who performs it.",
        F::Album => "The album it belongs to. Songs with the same album name and album artist are grouped.",
        F::AlbumArtist => "The artist of the whole album, such as Various Artists for a compilation.",
        F::Genre => "The genre, as text.",
        F::TrackNo => "The track number on its disc.",
        F::TrackTotal => "How many tracks the disc has.",
        F::DiscNo => "Which disc of the set this is.",
        F::DiscTotal => "How many discs the set has.",
        F::Year => "The year it came out, four digits.",
    };
    let mut text = what.to_string();
    if f.spec.fields.get(i).is_some_and(|x| x.2) {
        text.push_str(" The songs differ here: type to give them all the same, or leave it.");
    }
    tip(text, "Tab")
}

impl Ui {
    /// Turn tooltips on or off (the Settings switch).
    pub fn set_tooltips(&mut self, on: bool) {
        if self.tips_on != on {
            self.tips_on = on;
            self.dirty = true;
        }
    }

    /// Whether tooltips are on.
    pub fn tooltips(&self) -> bool {
        self.tips_on
    }

    /// What a library control says: its tooltip text and key. Exhaustive over [`LibHit`], so every control has an answer
    /// (`None` is for what is not a control, and for menu rows, which carry their own label and key).
    pub(crate) fn lib_tip_text(
        &self,
        hit: LibHit,
        model: &UiModel,
        ctx: &LibCtx<'_>,
    ) -> Option<(String, String)> {
        let lib = ctx.lib;
        match hit {
            LibHit::None | LibHit::Menu(..) | LibHit::Scrim => None,
            LibHit::DrawerBtn => tip("Open the menu: the views, folders and settings.", ""),
            LibHit::Rail(v) => view_text(v),
            LibHit::ModeSwitch(Mode::Library) => tip("Show the library.", "B"),
            LibHit::ModeSwitch(Mode::Player) => tip("Show the player: the picture and its controls.", "B"),
            LibHit::AddFolder => {
                tip("Add a folder of music or videos to your library. Files stay where they are.", "")
            }
            LibHit::Settings => {
                tip("Settings: theme, audio, visualizer, tooltips, updates and more.", "Ctrl+,")
            }
            LibHit::About => view_text(View::About),
            LibHit::Folder(i) => {
                let name = lib.roots().get(i).map_or("This folder", |r| r.name.as_str());
                tip(alloc::format!("{name}. Right-click to scan it again or take it out of the library."), "")
            }
            LibHit::Search => {
                tip("Type to search albums, artists, songs and videos. Accents and case do not matter.", "/")
            }
            LibHit::SearchClear => tip("Clear the search.", "Esc"),
            LibHit::Sort => tip("Choose how the songs are ordered.", ""),
            LibHit::SortCol(c) => tip(
                match c {
                    0 => "Sort by title. Click again to reverse.",
                    1 => "Sort by artist. Click again to reverse.",
                    2 => "Sort by album. Click again to reverse.",
                    _ => "Sort by length. Click again to reverse.",
                },
                "",
            ),
            LibHit::Back => tip("Go back to where you were.", "Backspace"),
            LibHit::Ent(i) | LibHit::EntPlay(i) | LibHit::EntHeart(i) => {
                let kind = self.lib.rows.as_ref()?.ents.get(i)?.kind;
                let name = self.ent_label(kind, model, ctx);
                match (hit, kind) {
                    (LibHit::EntHeart(_), _) => {
                        let on = self.ent_item(kind, ctx, model).is_some_and(|id| lib.is_favorite(id));
                        tip(
                            if on {
                                alloc::format!("Remove {name} from your favorites.")
                            } else {
                                alloc::format!("Add {name} to your favorites.")
                            },
                            "Ctrl+F",
                        )
                    }
                    (LibHit::EntPlay(_), EntKind::Album(_)) => {
                        tip(alloc::format!("Play the album {name}."), "Enter")
                    }
                    (LibHit::EntPlay(_), EntKind::Video { .. }) => {
                        tip(alloc::format!("Play {name}."), "Enter")
                    }
                    (LibHit::EntPlay(_), _) => tip(alloc::format!("Play {name}."), "Enter"),
                    (_, EntKind::Album(_)) => tip(
                        alloc::format!(
                            "{name}. Click to open, double-click to play. Right-click for more, E edits the tags."
                        ),
                        "",
                    ),
                    (_, EntKind::Artist(_)) => {
                        tip(alloc::format!("{name}. Click to see their albums. Right-click for more."), "")
                    }
                    (_, EntKind::Playlist(_)) => tip(
                        alloc::format!(
                            "{name}. Click to open it, right-click to rename, export or delete it."
                        ),
                        "",
                    ),
                    (_, EntKind::Track { .. }) => tip(
                        alloc::format!(
                            "{name}. Double-click to play. Ctrl+F hearts it, E edits its tags, right-click for more."
                        ),
                        "Enter",
                    ),
                    (_, EntKind::Video { .. }) => tip(
                        alloc::format!(
                            "{name}. Double-click to play. Ctrl+F hearts it, right-click for more."
                        ),
                        "Enter",
                    ),
                    (_, EntKind::Queue(_)) => tip(
                        alloc::format!(
                            "{name}. Double-click to play it now, drag to move it, Delete takes it out."
                        ),
                        "Enter",
                    ),
                    (_, EntKind::PlEntry { .. }) => tip(
                        alloc::format!(
                            "{name}. Double-click to play from here, drag to move it, Delete takes it out."
                        ),
                        "Enter",
                    ),
                }
            }
            LibHit::Scrollbar => {
                tip("Scroll the list. The mouse wheel and Page Up and Page Down do it too.", "")
            }
            LibHit::Button(id) => header_button_text(self.lib.view, self.lib.detail, id),
            LibHit::BarInfo => tip("Open the now-playing screen.", "6"),
            LibHit::Bar(b) => btn_text(b, model, true, self.lib.view),
            LibHit::Seek => tip(
                "Seek: click or drag along the bar. Left and Right jump 5 seconds, with Shift 30.",
                "Left / Right",
            ),
            LibHit::Volume => tip(
                alloc::format!(
                    "Volume {}%. Drag it, scroll over it, or use Up and Down.",
                    (model.volume * 100.0 + 0.5) as i32
                ),
                "Up / Down",
            ),
            LibHit::Viz(i) => match i {
                0 => tip("The previous effect.", "Left"),
                1 => tip("The next effect.", "Right"),
                2 => tip("The next colour scheme.", "C"),
                3 => tip("Show or hide the title and artist over the picture.", "T"),
                4 => tip("Pause or resume the animation.", "Enter"),
                _ => tip(
                    if model.viz_cycle {
                        "The effect changes by itself. Click to stop. Order and time are in Settings."
                    } else {
                        "Change the effect by itself, in the order and every so often as Settings say."
                    },
                    "Shift+V",
                ),
            },
            LibHit::UpNext(_) => tip("Play this item next.", "Enter"),
            LibHit::TagField(i) => tag_field_text(i, self),
            LibHit::TagButton(id) => match id {
                0 => {
                    tip("Write these tags into the file or files. Nothing is changed until you do.", "Enter")
                }
                1 => tip(
                    if self.lib.tagform.as_ref().is_some_and(|f| f.spec.read_only.is_some()) {
                        "Close this."
                    } else {
                        "Close without saving."
                    },
                    "Esc",
                ),
                2 => tip("Choose a JPEG or PNG picture for the cover.", ""),
                _ => tip("Take the cover picture out of the file or files, or put it back.", ""),
            },
            LibHit::PromptOk => tip("Go ahead with this name.", "Enter"),
            LibHit::PromptCancel => tip("Close without changing anything.", "Esc"),
        }
    }

    /// The name of what an entity is, for its tooltip.
    fn ent_label(&self, kind: EntKind, model: &UiModel, ctx: &LibCtx<'_>) -> String {
        let lib = ctx.lib;
        match kind {
            EntKind::Album(i) => {
                lib.albums().get(i).map_or(String::new(), |a| alloc::format!("{} by {}", a.title, a.artist))
            }
            EntKind::Artist(i) => lib.artists().get(i).map_or(String::new(), |a| a.name.clone()),
            EntKind::Track { id, .. } => lib
                .track(id)
                .map_or(String::new(), |t| alloc::format!("{} by {}", t.display_title(), t.display_artist())),
            EntKind::Video { id, .. } => {
                lib.video(id).map_or(String::new(), |v| v.display_title().to_string())
            }
            EntKind::Queue(id) => {
                model.playlist.iter().find(|e| e.id == id).map_or(String::new(), |e| e.label.clone())
            }
            EntKind::Playlist(id) => lib.playlist(id).map_or(String::new(), |p| p.name.clone()),
            EntKind::PlEntry { pl, idx } => {
                lib.playlist(pl).and_then(|p| p.entries.get(idx)).map_or(String::new(), |e| {
                    e.title
                        .clone()
                        .unwrap_or_else(|| e.path.rsplit('/').next().unwrap_or(&e.path).to_string())
                })
            }
        }
    }

    /// Where a library control is on screen (the last frame drawn).
    pub(crate) fn lib_tip_anchor(&mut self, hit: LibHit, model: &UiModel, ctx: &LibCtx<'_>) -> Option<RectF> {
        let g = self.lib.last_geom.clone()?;
        let (w, h) = (g.m.w, g.m.h);
        let ent = |ui: &Ui, i: usize| ui.lib.visible.iter().find(|(e, _)| *e == i).map(|(_, r)| *r);
        match hit {
            LibHit::None | LibHit::Menu(..) | LibHit::Scrim => None,
            LibHit::DrawerBtn => g.menu_btn,
            LibHit::Rail(v) => g.nav.iter().find(|(x, _)| *x == v).map(|x| x.1),
            LibHit::ModeSwitch(m) => Some(g.mode[if m == Mode::Library { 0 } else { 1 }]),
            LibHit::AddFolder => Some(g.add_folder),
            LibHit::Settings => Some(g.settings),
            LibHit::About => Some(g.about),
            LibHit::Folder(i) => g.folders.iter().find(|(x, _)| *x == i).map(|x| x.1),
            LibHit::Search => Some(g.search),
            LibHit::SearchClear => Some(g.search_clear),
            LibHit::Sort => g.sort,
            LibHit::SortCol(c) => g.table_cols.iter().find(|(x, _)| *x == c).map(|x| x.1),
            LibHit::Back => g.back,
            LibHit::Ent(i) | LibHit::EntPlay(i) => ent(self, i),
            LibHit::EntHeart(i) => {
                let r = ent(self, i)?;
                let kind = self.lib.rows.as_ref()?.ents.get(i)?.kind;
                crate::lib_ui::rows::heart_rect(kind, self.lib.rows.as_ref()?.video_list, r, self.scale)
            }
            LibHit::Scrollbar => Some(g.scroll_track),
            LibHit::Button(id) => {
                if self.lib.view == View::About && self.lib.detail.is_none() {
                    let row = self.lib.rows.as_ref()?.rows.first()?.clone();
                    let rect =
                        RectF::new(g.m.body.x, g.m.body.y + row.y - self.lib.scroll, g.m.body.w, row.h);
                    return self.about_page(None, rect, model).iter().find(|b| b.id == id).map(|b| b.rect);
                }
                g.header_btns
                    .iter()
                    .find(|b| b.id == id)
                    .map(|b| b.rect)
                    .or_else(|| self.lib.hero.iter().find(|b| b.0 == id).map(|b| b.1))
                    .or_else(|| self.lib.msg_btns.iter().find(|b| b.0 == id).map(|b| b.1))
                    .or_else(|| {
                        let r = self.now_playing_rects(&g, model);
                        if id == 0 { r.browse } else { None }
                    })
            }
            LibHit::BarInfo => Some(g.bar_info),
            LibHit::Bar(b) => g
                .bar_btns
                .iter()
                .find(|(x, _)| *x == b)
                .map(|x| x.1)
                .or_else(|| self.now_playing_rects(&g, model).heart.filter(|_| b == Btn::Favorite)),
            LibHit::Seek => Some(g.seek_hit),
            LibHit::Volume => g.vol_hit,
            LibHit::Viz(i) => g.viz_btns.iter().find(|(x, _)| *x == i).map(|x| x.1),
            LibHit::UpNext(i) => self.now_playing_rects(&g, model).up_next.get(i).copied(),
            LibHit::TagField(i) => self.tagform_layout(w, h)?.fields.get(i).map(|f| f.2),
            LibHit::TagButton(id) => {
                self.tagform_layout(w, h)?.buttons.iter().find(|b| b.id == id).map(|b| b.rect)
            }
            LibHit::PromptOk => Some(self.prompt_buttons(&g).0),
            LibHit::PromptCancel => Some(self.prompt_buttons(&g).1),
        }
        .or_else(|| {
            let _ = ctx;
            None
        })
    }

    /// What the library's tooltip points at: what the pointer rests on, or (after a key press) what the keyboard is on.
    pub(crate) fn lib_tip_target(&self) -> Option<LibHit> {
        if !self.tips_on || !self.menu.is_empty() || self.lib.drag.is_some() || self.pressed_lib.is_some() {
            return None;
        }
        if self.lib.tagform.is_some() {
            return match (self.keyboard_mode, self.lib.hover) {
                (false, h @ (LibHit::TagField(_) | LibHit::TagButton(_))) => Some(h),
                (true, _) => self.lib.tagform.as_ref().map(|f| LibHit::TagField(f.focus)),
                _ => None,
            };
        }
        if self.lib.prompt.is_some() {
            return None;
        }
        if !self.keyboard_mode {
            return match self.lib.hover {
                LibHit::None => None,
                h => Some(h),
            };
        }
        let g = self.lib.last_geom.as_ref()?;
        match self.lib.zone {
            crate::lib_ui::Zone::Rail => g.nav.get(self.lib.rail_focus).map(|(v, _)| LibHit::Rail(*v)),
            crate::lib_ui::Zone::Bar => g.bar_btns.get(self.lib.bar_focus).map(|(b, _)| LibHit::Bar(*b)),
            crate::lib_ui::Zone::Search => Some(LibHit::Search),
            crate::lib_ui::Zone::Content => self.lib.sel.map(LibHit::Ent),
        }
    }

    /// Draw a tooltip next to its control: below it, or above when there is no room, wrapped at 140 characters and kept inside the window.
    pub(crate) fn draw_tip_box(&mut self, fb: &mut FrameBuffer, tip: &Tip, w: f32, h: f32) {
        let s = self.scale;
        let max_px = (w - 24.0 * s).max(120.0 * s);
        // 140 characters, and narrower where the window is.
        let mut lines: Vec<String> = Vec::new();
        for l in wrap_chars(&tip.text, TIP_WIDTH_CHARS) {
            lines.extend(self.wrap(Face::Sans, 12.5, &l, max_px - 28.0 * s, 6));
        }
        if lines.is_empty() {
            return;
        }
        let key = tip.key.clone();
        let kw = if key.is_empty() { 0.0 } else { self.text_w(Face::MonoBold, 11.0, &key, 0.0) + 12.0 * s };
        let widest = lines.iter().map(|l| self.text_w(Face::Sans, 12.5, l, 0.0)).fold(0.0, f32::max);
        let first_w = self.text_w(Face::Sans, 12.5, &lines[0], 0.0);
        let bw =
            (widest.max(first_w + if lines.len() == 1 { kw + 10.0 * s } else { 0.0 }) + 24.0 * s).min(max_px);
        let line_h = 18.0 * s;
        let extra = if kw > 0.0 && lines.len() > 1 { 24.0 * s } else { 0.0 };
        let bh = lines.len() as f32 * line_h + 16.0 * s + extra;
        let a = tip.anchor;
        let below = a.bottom() + 10.0 * s + bh <= h - 8.0 * s;
        let y = if below { a.bottom() + 10.0 * s } else { (a.y - 10.0 * s - bh).max(8.0 * s) };
        let x = (a.cx() - bw * 0.5).clamp(8.0 * s, (w - bw - 8.0 * s).max(8.0 * s));
        let r = RectF::new(x, y, bw, bh);
        fb.shadow_rrect(r, 10.0 * s, 4.0 * s, 14.0 * s, Rgba::new(5, 2, 15, 150), 1.0);
        fb.fill_rrect(r, 10.0 * s, Paint::Solid(t::ink_700()), 1.0);
        fb.stroke_rrect(r, 10.0 * s, 1.0 * s, t::ink_500(), 1.0);
        let mut ty = r.y + 8.0 * s + line_h * 0.5;
        let mut last_x = r.x + 12.0 * s;
        for l in &lines {
            last_x = self.text(fb, Face::Sans, 12.5, r.x + 12.0 * s, ty, l, t::text_body(), 1.0, 0.0);
            ty += line_h;
        }
        if kw > 0.0 {
            let (cx, cy) = if lines.len() == 1 {
                (last_x + 10.0 * s, r.cy())
            } else {
                (r.x + 12.0 * s, r.bottom() - 8.0 * s - 10.0 * s)
            };
            let chip = RectF::new(cx, cy - 9.0 * s, kw, 18.0 * s);
            fb.fill_rrect(chip, 5.0 * s, Paint::Solid(fade(t::cyan_500(), 0.14)), 1.0);
            self.text(fb, Face::MonoBold, 11.0, chip.x + 6.0 * s, chip.cy(), &key, t::cyan_400(), 1.0, 0.0);
        }
    }

    /// The library's tooltip for this frame, once the pointer or the keyboard has rested on a control long enough.
    pub(crate) fn lib_tip_now(&mut self, model: &UiModel, ctx: &LibCtx<'_>) -> Option<Tip> {
        let target = self.lib_tip_target()?;
        if self.tip_target != Some(target) || self.now - self.tip_since < crate::ui::TOOLTIP_DELAY_US {
            return None;
        }
        let (text, key) = self.lib_tip_text(target, model, ctx)?;
        let anchor = self.lib_tip_anchor(target, model, ctx)?;
        Some(Tip { text, key, anchor })
    }

    /// Keep the clock of the library tooltip: it starts when the target changes. Returns true when a redraw is due to show it.
    pub(crate) fn lib_tip_tick(&mut self, now_us: i64) -> bool {
        let target = self.lib_tip_target();
        if target != self.tip_target {
            self.tip_target = target;
            self.tip_since = now_us;
            self.tip_drawn = false;
            return false;
        }
        if target.is_some() && !self.tip_drawn && now_us - self.tip_since >= crate::ui::TOOLTIP_DELAY_US {
            self.tip_drawn = true;
            return true;
        }
        false
    }

    // ---- the player face ----------------------------------------------------------------------------------------------------

    /// What the player face's control under the pointer (or on the keyboard's focus) says.
    pub(crate) fn play_tip_text(&self, target: Target, model: &UiModel) -> Option<(String, String)> {
        match target {
            Target::Btn(b) => btn_text(b, model, false, View::Albums),
            Target::Volume => tip(
                alloc::format!(
                    "Volume {}%. Drag it, scroll over it, or use Up and Down.",
                    (model.volume * 100.0 + 0.5) as i32
                ),
                "Up / Down",
            ),
            Target::Seek => tip(
                "Seek: click or drag along the bar. Left and Right jump 5 seconds, with Shift 30.",
                "Left / Right",
            ),
            Target::Video | Target::Menu(..) => None,
        }
    }

    // ---- dialogs and panels ---------------------------------------------------------------------------------------------------

    /// The dialog or audio-panel control the pointer or the keyboard is on, as a number that changes with it.
    fn overlay_sig(&self, model: &UiModel) -> Option<i32> {
        if let Some(spec) = &model.dialog {
            let control = if self.keyboard_mode {
                self.dialog.focus.map(|i| {
                    if i < spec.toggles.len() {
                        DialogControl::Toggle(i as u8)
                    } else {
                        DialogControl::Button((i - spec.toggles.len()) as u8)
                    }
                })
            } else {
                self.dialog.hover
            };
            return control.map(|c| match c {
                DialogControl::Toggle(n) => n as i32,
                DialogControl::Button(n) => 100 + n as i32,
                DialogControl::Back => 99,
            });
        }
        let panel = self.audio_panel.as_ref()?;
        let c = if self.keyboard_mode { self.audio_panel_focus() } else { panel.hover }?;
        Some(1000 + c as i32)
    }

    /// Keep the clock of the dialog and panel tooltips. Returns true when a redraw is due to show one.
    pub(crate) fn overlay_tip_tick(&mut self, model: &UiModel, now_us: i64) -> bool {
        let sig = if self.tips_on { self.overlay_sig(model) } else { None };
        if sig != self.dlg_sig {
            self.dlg_sig = sig;
            self.dlg_since = now_us;
            self.dlg_drawn = false;
            return false;
        }
        if sig.is_some() && !self.dlg_drawn && now_us - self.dlg_since >= crate::ui::TOOLTIP_DELAY_US {
            self.dlg_drawn = true;
            return true;
        }
        false
    }

    /// The tooltip of the application's dialog control the pointer or the keyboard is on.
    pub(crate) fn dialog_tip_now(&mut self, model: &UiModel) -> Option<Tip> {
        let spec = model.dialog.as_ref()?;
        if !self.tips_on {
            return None;
        }
        let g = self.dialog_geom(spec);
        let control = if self.keyboard_mode {
            self.dialog.focus.map(|i| {
                if i < spec.toggles.len() {
                    DialogControl::Toggle(i as u8)
                } else {
                    DialogControl::Button((i - spec.toggles.len()) as u8)
                }
            })
        } else {
            self.dialog.hover
        };
        let (text, key, anchor) = match control? {
            DialogControl::Back => {
                let to = spec.back.as_ref()?;
                (alloc::format!("Back to {to}."), "Esc / Backspace".to_string(), g.back?)
            }
            DialogControl::Toggle(n) => {
                let t = spec.toggles.get(n as usize)?;
                (
                    if t.desc.is_empty() {
                        t.label.clone()
                    } else {
                        alloc::format!("{}. {}", t.label, t.desc)
                    },
                    "Space".to_string(),
                    g.toggles.get(n as usize)?.0,
                )
            }
            DialogControl::Button(n) => {
                let b = spec.buttons.get(n as usize)?;
                let key = if b.primary { "Enter" } else { "" };
                (dialog_button_text(&b.label), key.to_string(), *g.buttons.get(n as usize)?)
            }
        };
        if self.dlg_sig.is_none() || self.now - self.dlg_since < crate::ui::TOOLTIP_DELAY_US {
            return None;
        }
        Some(Tip { text, key, anchor })
    }

    /// The tooltip of an audio panel control.
    pub(crate) fn audio_tip_now(&mut self) -> Option<Tip> {
        use crate::audio_panel::AudioControl as A;
        if !self.tips_on {
            return None;
        }
        let panel = self.audio_panel.as_ref()?;
        let c = if self.keyboard_mode { self.audio_panel_focus() } else { panel.hover }?;
        let g = self.audio_panel_geom();
        let (text, key) = match c {
            A::Crossfade => (
                "Fade the end of a song into the start of the next one. Gapless albums are left alone.",
                "Space",
            ),
            A::CrossfadeLength => {
                ("How long the fade lasts, 2 to 10 seconds. Left and Right change it.", "Left / Right")
            }
            A::AutoLevel => ("Even out the loudness between songs, aiming at the level below.", "Space"),
            A::Target => (
                "The loudness the automatic level aims at, in LUFS. Left and Right change it.",
                "Left / Right",
            ),
            A::Mode => (
                "Level every song on its own, or whole albums so quiet and loud passages keep their balance.",
                "Space",
            ),
            A::Done => ("Close the audio settings.", "Esc"),
            A::Back => ("Back to Settings.", "Esc / Backspace"),
        };
        let anchor = g.controls.iter().find(|x| x.0 == c).map(|x| x.1)?;
        if self.dlg_sig.is_none() || self.now - self.dlg_since < crate::ui::TOOLTIP_DELAY_US {
            return None;
        }
        Some(Tip { text: text.to_string(), key: key.to_string(), anchor })
    }

    /// Run the action of a key, for the tests: whether `key` is the one `Action` is bound to.
    #[allow(dead_code)]
    pub(crate) fn key_of(action: Action) -> String {
        crate::actions::shortcut_label(action)
    }
}

fn dialog_button_text(label: &str) -> String {
    let l = label.to_lowercase();
    if l.starts_with("close") || l == "done" {
        "Close this dialog.".to_string()
    } else if l.starts_with("later") || l.starts_with("cancel") {
        "Not now: nothing changes.".to_string()
    } else if l.starts_with("no thanks") {
        "No, and do not ask again.".to_string()
    } else {
        let mut s = label.trim_end_matches('\u{2026}').to_string();
        s.push('.');
        s
    }
}
