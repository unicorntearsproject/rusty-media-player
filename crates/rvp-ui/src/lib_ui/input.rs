//! Pointer and keyboard in library mode: navigating the views, the search box, selecting and playing rows, the bar's controls,
//! drags on the scroll bar and the sliders, and what the context menus offer.
use super::geom::Geom;
use super::menus;
use super::rows::{EntKind, RowKind};
use super::{
    Detail, Enqueue, LibAction, LibCtx, LibDrag, LibHit, Mode, NavEntry, Prompt, PromptKind, Scope,
    UiCommand, VideoSort, View, VizReturn, Zone, rows_key,
};
use crate::actions::{Action, MenuItem, shortcut_for};
use crate::gfx::RectF;
use crate::model::UiModel;
use crate::ui::{Btn, DOUBLE_CLICK_US, Ui};
use alloc::string::String;
use alloc::vec::Vec;
use rvp_host::{InputEvent, Key, Modifiers, PointerButton};

/// The library's home: the album grid, at the top.
fn home_nav() -> NavEntry {
    NavEntry { view: View::Albums, detail: None, scroll: 0.0, sel: None }
}

/// The views the digit keys go to.
const DIGITS: [View; 9] = [
    View::Albums,
    View::Artists,
    View::Tracks,
    View::Playlists,
    View::Queue,
    View::NowPlaying,
    View::Visualizer,
    View::Videos,
    View::Favorites,
];

impl Ui {
    // ---- state changes the application and the UI itself use ---------------------------------------------------------

    /// Switch between the Library and the Player face.
    pub fn set_mode(&mut self, mode: Mode) {
        if self.lib.mode != mode {
            self.lib.mode = mode;
            self.menu.clear();
            self.lib.prompt = None;
            self.lib.drag = None;
            self.dirty = true;
        }
    }

    /// The current face.
    pub fn mode(&self) -> Mode {
        self.lib.mode
    }

    /// The library state (read-only), for tests and tooling.
    pub fn lib_state(&self) -> &super::LibUi {
        &self.lib
    }

    /// Show a view (switching to the Library face). Opening Search focuses its box.
    pub fn show_view(&mut self, view: View) {
        if view == View::Visualizer {
            if self.lib.mode == Mode::Library && self.lib.view == View::Visualizer {
                // Already there: the way out stays where the user came from, not the visualizer itself.
                return;
            }
            self.lib.viz_return = Some(self.capture_nav());
        } else {
            self.lib.viz_return = None;
        }
        self.set_mode(Mode::Library);
        let prev = (self.lib.view, self.lib.detail);
        if view != View::Search && self.lib.view == View::Search && !self.lib.query.is_empty() {
            self.lib.query.clear();
        }
        self.lib.view = view;
        self.lib.detail = None;
        self.lib.history.clear();
        // The way out of the search box leads back to where it was opened from.
        self.lib.before_search =
            if view == View::Search && prev.0 != View::Search { Some(prev) } else { None };
        self.lib.scroll = 0.0;
        self.lib.sel = None;
        self.lib.rows = None;
        self.lib.zone = if view == View::Search { Zone::Search } else { Zone::Content };
        self.lib.hover = LibHit::None;
        self.dirty = true;
    }

    /// Where the user is now, to come back to it after the visualizer.
    fn capture_nav(&self) -> VizReturn {
        let l = &self.lib;
        // A visualizer left behind the player face is not a place to return to: use the library's home then.
        let (nav, history, query, before_search) = if l.view == View::Visualizer {
            (home_nav(), Vec::new(), String::new(), None)
        } else {
            (
                NavEntry { view: l.view, detail: l.detail, scroll: l.scroll, sel: l.sel },
                l.history.clone(),
                l.query.clone(),
                l.before_search,
            )
        };
        VizReturn { mode: l.mode, nav, history, query, before_search }
    }

    /// Open an album, an artist or a playlist, keeping the way back.
    pub fn open_detail(&mut self, d: Detail) {
        self.lib.viz_return = None;
        self.set_mode(Mode::Library);
        let entry = NavEntry {
            view: self.lib.view,
            detail: self.lib.detail,
            scroll: self.lib.scroll,
            sel: self.lib.sel,
        };
        self.lib.history.push(entry);
        if self.lib.view == View::Search && !self.lib.query.is_empty() {
            // The way back is the search result.
        }
        if matches!(self.lib.view, View::NowPlaying | View::Visualizer | View::Queue) {
            self.lib.view = View::Albums;
        }
        self.lib.detail = Some(d);
        self.lib.scroll = 0.0;
        self.lib.sel = None;
        self.lib.rows = None;
        self.lib.zone = Zone::Content;
        self.dirty = true;
    }

    /// Leave the visualizer for the view it was entered from: the face, view, album, scroll position and selection the user
    /// came from. When that place is gone (the album was removed, the queue emptied, nothing is loaded any more) it falls
    /// back to the player if a video is loaded and to the library's home otherwise.
    pub fn leave_visualizer(&mut self, model: &UiModel, ctx: &LibCtx<'_>) {
        let ok = |d: Option<Detail>| match d {
            None => true,
            Some(Detail::Album(i)) => ctx.lib.album(i).is_some(),
            Some(Detail::Artist(i)) => ctx.lib.artist(i).is_some(),
            Some(Detail::Playlist(i)) => ctx.lib.playlist(i).is_some(),
        };
        let mut r = self.lib.viz_return.take().unwrap_or_else(|| VizReturn {
            mode: Mode::Library,
            nav: home_nav(),
            history: Vec::new(),
            query: String::new(),
            before_search: None,
        });
        r.history.retain(|e| ok(e.detail));
        if !ok(r.nav.detail) {
            // The album is gone: stay in its list.
            r.nav = NavEntry { detail: None, scroll: 0.0, sel: None, ..r.nav };
        }
        let gone = match (r.mode, r.nav.view) {
            (Mode::Player, _) => !model.has_media(),
            (_, View::Queue) => model.playlist.is_empty(),
            (_, View::NowPlaying) => !model.has_media(),
            _ => false,
        };
        if gone {
            let mode = if model.has_media() && model.has_video { Mode::Player } else { Mode::Library };
            r = VizReturn {
                mode,
                nav: home_nav(),
                history: Vec::new(),
                query: String::new(),
                before_search: None,
            };
        }
        self.menu.clear();
        self.lib.view = r.nav.view;
        self.lib.detail = r.nav.detail;
        self.lib.scroll = r.nav.scroll;
        self.lib.sel = r.nav.sel;
        self.lib.history = r.history;
        self.lib.query = r.query;
        self.lib.before_search = r.before_search;
        self.lib.zone = if r.nav.view == View::Search && self.lib.query.is_empty() {
            Zone::Search
        } else {
            Zone::Content
        };
        self.lib.rows = None;
        self.lib.hover = LibHit::None;
        self.lib.drag = None;
        self.set_mode(r.mode);
        self.dirty = true;
    }

    /// The visualizer's button and key: open it, or leave it for where the user came from.
    pub fn toggle_visualizer(&mut self, model: &UiModel, ctx: &LibCtx<'_>) {
        if self.lib.mode == Mode::Library && self.lib.view == View::Visualizer {
            self.leave_visualizer(model, ctx);
        } else {
            self.show_view(View::Visualizer);
        }
    }

    /// One step back: out of an album or artist, or to the view that was open before; out of the visualizer to where it
    /// was entered from.
    pub fn go_back(&mut self, model: &UiModel, ctx: &LibCtx<'_>) {
        if self.lib.view == View::Visualizer {
            self.leave_visualizer(model, ctx);
            return;
        }
        if let Some(e) = self.lib.history.pop() {
            self.lib.view = e.view;
            self.lib.detail = e.detail;
            self.lib.scroll = e.scroll;
            self.lib.sel = e.sel;
        } else if self.lib.detail.is_some() {
            self.lib.detail = None;
            self.lib.scroll = 0.0;
            self.lib.sel = None;
        } else if self.lib.view == View::Search {
            self.leave_search();
        }
        self.lib.rows = None;
        self.dirty = true;
    }

    fn leave_search(&mut self) {
        self.lib.query.clear();
        let (v, d) = self.lib.before_search.take().unwrap_or((View::Albums, None));
        self.lib.view = v;
        self.lib.detail = d;
        self.lib.scroll = 0.0;
        self.lib.sel = None;
        self.lib.rows = None;
        self.lib.zone = Zone::Content;
    }

    /// The text of the search box changed: results are shown in the Search view.
    fn query_changed(&mut self) {
        if self.lib.query.is_empty() {
            if self.lib.view == View::Search {
                self.lib.scroll = 0.0;
                self.lib.sel = None;
            }
        } else if self.lib.view != View::Search {
            self.lib.before_search = Some((self.lib.view, self.lib.detail));
            self.lib.view = View::Search;
            self.lib.detail = None;
            self.lib.history.clear();
        }
        self.lib.scroll = 0.0;
        self.lib.sel = None;
        self.lib.rows = None;
    }

    /// Text commands that were collected (the application drains them after each event).
    pub fn take_commands(&mut self) -> Vec<UiCommand> {
        core::mem::take(&mut self.lib.commands)
    }

    /// Ask for a playlist name. `from` are the tracks to start it with.
    pub fn ask_playlist_name(&mut self, from: Option<Scope>) {
        self.lib.prompt = Some(Prompt {
            title: "Name your playlist".into(),
            text: String::new(),
            kind: PromptKind::NewPlaylist(from),
        });
        self.menu.clear();
        self.dirty = true;
    }

    /// Ask for a new name for playlist `id` (with its current name filled in by the caller through `current`).
    pub fn ask_playlist_rename(&mut self, id: u32, current: &str) {
        self.lib.prompt = Some(Prompt {
            title: "Rename playlist".into(),
            text: current.into(),
            kind: PromptKind::Rename(id),
        });
        self.menu.clear();
        self.dirty = true;
    }

    /// Switch the visualizer animation on or off.
    pub fn set_viz_on(&mut self, on: bool) {
        self.lib.viz_on = on;
        self.dirty = true;
    }

    /// Show or hide the visualizer's title overlay.
    pub fn set_viz_info(&mut self, on: bool) {
        self.lib.viz_info = on;
        self.dirty = true;
    }

    /// The tracks a [`Scope`] means, in order (the library ids). `ListFrom` and `List` refer to what is on screen.
    pub fn scope_tracks(&self, scope: Scope, ctx: &LibCtx<'_>, model: &UiModel) -> Vec<u32> {
        let lib = ctx.lib;
        match scope {
            Scope::Track(id) => lib.track(id).map(|t| t.id).into_iter().collect(),
            Scope::Video(id) => lib.video(id).map(|v| v.id).into_iter().collect(),
            Scope::Album(id) => lib.album(id).map(|a| a.tracks.clone()).unwrap_or_default(),
            Scope::Artist(id) => lib
                .artist(id)
                .map(|a| a.albums.iter().flat_map(|&i| lib.albums()[i].tracks.iter().copied()).collect())
                .unwrap_or_default(),
            Scope::Playlist(id) => lib.playlist(id).map(|p| p.track_ids()).unwrap_or_default(),
            Scope::PlaylistFrom(id, _) => lib.playlist(id).map(|p| p.track_ids()).unwrap_or_default(),
            Scope::AllTracks => lib.sorted_tracks(self.lib.track_sort, self.lib.track_asc),
            Scope::FavoriteTracks => lib.favorite_tracks(),
            Scope::FavoriteVideos => lib.favorite_videos(),
            Scope::ListFrom(_) | Scope::List => {
                self.lib.rows.as_ref().map(|r| r.list.clone()).unwrap_or_default()
            }
            Scope::Queue => model.playlist.iter().filter_map(|e| e.track).collect(),
        }
    }

    /// Where in [`Ui::scope_tracks`] playback starts: the clicked row for `ListFrom` and `PlaylistFrom`, else the first.
    pub fn scope_start(&self, scope: Scope, ctx: &LibCtx<'_>) -> usize {
        match scope {
            Scope::ListFrom(pos) => pos as usize,
            Scope::PlaylistFrom(id, from) => ctx
                .lib
                .playlist(id)
                .map_or(0, |p| p.entries.iter().take(from as usize).filter(|e| e.track.is_some()).count()),
            _ => 0,
        }
    }

    /// The library id of the song or video an entity stands for (the heart acts on it), if it is one.
    pub(crate) fn ent_item(&self, kind: EntKind, ctx: &LibCtx<'_>, model: &UiModel) -> Option<u32> {
        match kind {
            EntKind::Track { id, .. } | EntKind::Video { id, .. } => Some(id),
            EntKind::PlEntry { pl, idx } => {
                ctx.lib.playlist(pl)?.entries.get(idx)?.track.filter(|t| ctx.lib.track(*t).is_some())
            }
            EntKind::Queue(q) => model.playlist.iter().find(|e| e.id == q)?.track,
            _ => None,
        }
    }

    // ---- layout of rows for the current frame ------------------------------------------------------------------------

    /// Build the rows if the view, the data or the size changed; keep the scroll position and selection valid.
    pub(crate) fn ensure_rows(&mut self, model: &UiModel, ctx: &LibCtx<'_>, g: &Geom) {
        let key = rows_key(&self.lib, model, ctx, &g.m);
        if self.lib.rows.as_ref().map(|r| &r.key) != Some(&key) {
            let rows = super::rows::build(&self.lib, model, ctx, &g.m, key);
            self.lib.rows = Some(rows);
        }
        let (total, n) = self.lib.rows.as_ref().map_or((0.0, 0), |r| (r.total, r.ents.len()));
        let max = (total - g.m.body.h).max(0.0);
        self.lib.scroll = self.lib.scroll.clamp(0.0, max);
        if self.lib.sel.is_some_and(|i| i >= n) {
            self.lib.sel = if n == 0 { None } else { Some(n - 1) };
        }
    }

    fn max_scroll(&self, g: &Geom) -> f32 {
        self.lib.rows.as_ref().map_or(0.0, |r| (r.total - g.m.body.h).max(0.0))
    }

    /// Scroll so the entity `i` is fully visible.
    fn reveal(&mut self, i: usize, g: &Geom) {
        let Some(r) = &self.lib.rows else { return };
        if i >= r.ents.len() {
            return;
        }
        let rect = r.ent_rect(i, &g.m);
        let (top, bottom) = (rect.y, rect.y + rect.h + 8.0 * self.scale);
        let h = g.m.body.h;
        if top < self.lib.scroll {
            self.lib.scroll = top - 8.0 * self.scale;
        } else if bottom > self.lib.scroll + h {
            self.lib.scroll = bottom - h;
        }
        self.lib.scroll = self.lib.scroll.clamp(0.0, self.max_scroll(g));
    }

    // ---- hit testing -----------------------------------------------------------------------------------------------

    /// What is at (`x`, `y`).
    pub(crate) fn lib_hit(&mut self, x: f32, y: f32, g: &Geom, model: &UiModel, ctx: &LibCtx<'_>) -> LibHit {
        let s = self.scale;
        if self.lib.prompt.is_some() {
            let (ok, cancel) = self.prompt_buttons(g);
            return if ok.contains(x, y) {
                LibHit::PromptOk
            } else if cancel.contains(x, y) {
                LibHit::PromptCancel
            } else {
                LibHit::None
            };
        }
        for (pi, p) in self.menu.iter().enumerate().rev() {
            if p.rect.contains(x, y) {
                for (ri, r) in p.rows.iter().enumerate() {
                    if r.contains(x, y) {
                        return LibHit::Menu(pi, ri);
                    }
                }
                return LibHit::Menu(pi, usize::MAX);
            }
        }
        let view = self.lib.view;
        // The bar (in the visualizer it floats over the picture and is only there while it shows).
        if view != View::Visualizer || self.controls_alpha > 0.3 {
            for (b, r) in &g.bar_btns {
                if r.contains(x, y) {
                    return LibHit::Bar(*b);
                }
            }
            if g.seek_hit.contains(x, y) && model.duration_us.is_some() {
                return LibHit::Seek;
            }
            if g.vol_hit.is_some_and(|r| r.contains(x, y)) {
                return LibHit::Volume;
            }
            if g.bar_info.contains(x, y) && model.has_media() {
                return LibHit::BarInfo;
            }
            if y >= g.m.bar.y {
                return LibHit::None;
            }
        }
        if view == View::Visualizer {
            for (i, r) in &g.viz_btns {
                if r.contains(x, y) {
                    return LibHit::Viz(*i);
                }
            }
            return LibHit::None;
        }
        // The rail.
        if g.m.rail.contains(x, y) {
            for (i, r) in g.mode.iter().enumerate() {
                if r.contains(x, y) {
                    return LibHit::ModeSwitch(if i == 0 { Mode::Library } else { Mode::Player });
                }
            }
            for (v, r) in &g.nav {
                if r.contains(x, y) {
                    return LibHit::Rail(*v);
                }
            }
            if g.add_folder.contains(x, y) {
                return LibHit::AddFolder;
            }
            if g.settings.contains(x, y) {
                return LibHit::Settings;
            }
            if g.about.contains(x, y) {
                return LibHit::About;
            }
            for (i, r) in &g.folders {
                if r.contains(x, y) {
                    return LibHit::Folder(*i);
                }
            }
            return LibHit::None;
        }
        // The header.
        if y < g.m.header.bottom() {
            if g.back.is_some_and(|r| r.contains(x, y)) {
                return LibHit::Back;
            }
            if g.search_clear.contains(x, y) && !self.lib.query.is_empty() {
                return LibHit::SearchClear;
            }
            if g.search.contains(x, y) {
                return LibHit::Search;
            }
            if g.sort.is_some_and(|r| r.contains(x, y)) {
                return LibHit::Sort;
            }
            for b in &g.header_btns {
                if b.rect.contains(x, y) {
                    return LibHit::Button(b.id);
                }
            }
            return LibHit::None;
        }
        if let Some(th) = g.m.table_head {
            if th.contains(x, y) {
                for (c, r) in &g.table_cols {
                    if r.contains(x, y) {
                        return LibHit::SortCol(*c);
                    }
                }
                return LibHit::None;
            }
        }
        if !g.m.body.contains(x, y) {
            return LibHit::None;
        }
        // Views that are not lists.
        if view == View::NowPlaying && self.lib.detail.is_none() {
            return self.now_playing_hit(x, y, g, model, ctx);
        }
        self.ensure_rows(model, ctx, g);
        let Some(rows) = &self.lib.rows else { return LibHit::None };
        let max = (rows.total - g.m.body.h).max(0.0);
        if max > 0.0 && x >= g.scroll_track.x - 8.0 * s && x <= g.m.body.right() {
            return LibHit::Scrollbar;
        }
        let by = y - g.m.body.y + self.lib.scroll;
        let bx = x - g.m.body.x;
        // The buttons of the empty-library message.
        if let Some(row) = rows.rows.first().filter(|r| matches!(r.kind, RowKind::Message(..))) {
            if self.empty_view(ctx) && self.lib.detail.is_none() {
                let rect = RectF::new(g.m.body.x, g.m.body.y + row.y - self.lib.scroll, g.m.body.w, row.h);
                for b in self.message_buttons(rect) {
                    if b.rect.contains(x, y) {
                        return LibHit::Button(b.id);
                    }
                }
            }
        }
        let Some(rows) = &self.lib.rows else { return LibHit::None };
        // The hero block's buttons.
        if let Some(d) = self.lib.detail {
            if let Some(row) = rows.rows.first().filter(|r| r.kind == RowKind::Hero) {
                let rect = RectF::new(g.m.body.x, g.m.body.y + row.y - self.lib.scroll, g.m.body.w, row.h);
                if rect.contains(x, y) {
                    for b in self.hero_buttons(rect, d, ctx) {
                        if b.rect.contains(x, y) {
                            return LibHit::Button(b.id);
                        }
                    }
                    return LibHit::None;
                }
            }
        }
        // The About page's buttons.
        if view == View::About && self.lib.detail.is_none() {
            let rect =
                self.lib.rows.as_ref().and_then(|r| r.rows.first().filter(|r| r.kind == RowKind::About)).map(
                    |row| RectF::new(g.m.body.x, g.m.body.y + row.y - self.lib.scroll, g.m.body.w, row.h),
                );
            if let Some(rect) = rect {
                for b in self.about_page(None, rect, model) {
                    if b.rect.contains(x, y) {
                        return LibHit::Button(b.id);
                    }
                }
            }
            return LibHit::None;
        }
        let Some(rows) = &self.lib.rows else { return LibHit::None };
        match rows.ent_at(bx, by, &g.m) {
            Some(i) => {
                let rect = rows.ent_rect(i, &g.m);
                let (lx, ly) = (bx - rect.x, by - rect.y);
                // The heart.
                if let Some(h) = super::rows::heart_rect(rows.ents[i].kind, rows.video_list, rect, s) {
                    if h.contains(bx, by) && self.ent_item(rows.ents[i].kind, ctx, model).is_some() {
                        return LibHit::EntHeart(i);
                    }
                }
                // The play button: the lower right of a card, the number column of a track row.
                let play = match rows.ents[i].kind {
                    EntKind::Album(_) => {
                        let cw = g.m.card_w();
                        let (cx, cy) = (cw - 30.0 * s, cw - 30.0 * s);
                        (lx - cx).abs() < 26.0 * s && (ly - cy).abs() < 26.0 * s
                    }
                    EntKind::Video { .. } if !rows.video_list => {
                        let (cw, ch) = (g.m.vcard_w(), g.m.vcard_poster_h());
                        (lx - cw * 0.5).abs() < 28.0 * s && (ly - ch * 0.5).abs() < 28.0 * s
                    }
                    EntKind::Video { .. } => lx < 84.0 * s,
                    EntKind::Track { .. } | EntKind::PlEntry { .. } | EntKind::Queue(_) => lx < 52.0 * s,
                    _ => false,
                };
                if play { LibHit::EntPlay(i) } else { LibHit::Ent(i) }
            }
            None => LibHit::None,
        }
    }

    fn now_playing_hit(&mut self, x: f32, y: f32, g: &Geom, model: &UiModel, ctx: &LibCtx<'_>) -> LibHit {
        let _ = ctx;
        let rects = self.now_playing_rects(g, model);
        for (i, r) in rects.up_next.iter().enumerate() {
            if r.contains(x, y) {
                return LibHit::UpNext(i);
            }
        }
        if rects.browse.is_some_and(|r| r.contains(x, y)) {
            return LibHit::Button(0);
        }
        if rects.heart.is_some_and(|r| r.contains(x, y)) {
            return LibHit::Bar(Btn::Favorite);
        }
        LibHit::None
    }

    // ---- events -------------------------------------------------------------------------------------------------------

    /// Handle one input event in library mode; returns what the application must do.
    pub fn handle_lib(
        &mut self,
        ev: &InputEvent,
        now_us: i64,
        model: &UiModel,
        ctx: &LibCtx<'_>,
    ) -> Vec<Action> {
        if let Some(d) = &model.dialog {
            self.dialog_open = true;
            return self.dialog_event(ev, now_us, d);
        }
        self.dialog_closed();
        if self.audio_panel.is_some() {
            return self.audio_panel_event(ev, now_us, model);
        }
        self.now = now_us;
        let mut out = Vec::new();
        // A pointer that only moves over the same thing changes nothing on screen (see `lib_move`).
        if !matches!(ev, InputEvent::PointerMove { .. }) {
            self.dirty = true;
        }
        match ev {
            InputEvent::Resize { w, h, dpr } => self.set_size(*w, *h, *dpr),
            InputEvent::Focus(f) => {
                self.win_focused = *f;
                if !*f {
                    self.lib.drag = None;
                    self.scrub = None;
                    self.pressed_lib = None;
                }
            }
            InputEvent::DragOver(on) => self.drag_over = *on,
            InputEvent::Drop { .. } => self.drag_over = false,
            InputEvent::PointerMove { x, y } => self.lib_move(*x, *y, now_us, model, ctx),
            InputEvent::PointerDown { x, y, button } => {
                self.lib_down(*x, *y, *button, now_us, model, ctx, &mut out)
            }
            InputEvent::PointerUp { x, y, button } => {
                self.lib_up(*x, *y, *button, now_us, model, ctx, &mut out)
            }
            InputEvent::Wheel { dy, .. } => self.lib_wheel(*dy, model, ctx, &mut out),
            InputEvent::KeyDown { key, mods, repeat } => {
                self.last_activity = now_us;
                self.keyboard_mode = true;
                let before = out.len();
                let had_menu = !self.menu.is_empty();
                let (zone, view, detail, prompt) =
                    (self.lib.zone, self.lib.view, self.lib.detail, self.lib.prompt.is_some());
                let sel = self.lib.sel;
                self.lib_key(key, mods, *repeat, now_us, model, ctx, &mut out);
                self.key_used = out.len() > before
                    || had_menu
                    || !self.menu.is_empty()
                    || zone != self.lib.zone
                    || view != self.lib.view
                    || detail != self.lib.detail
                    || prompt
                    || self.lib.prompt.is_some()
                    || sel != self.lib.sel
                    || self.lib.zone == Zone::Search
                    || matches!(key, Key::Escape | Key::Enter)
                    || matches!(key, Key::Other(n) if n == "Backspace" || n == "Tab");
            }
            InputEvent::Paste(text) => {
                // Pasted text goes where typing goes: a name prompt, else the search box (one line, no control characters).
                let clean: alloc::string::String =
                    text.chars().filter(|c| !c.is_control()).take(200).collect();
                if let Some(p) = &mut self.lib.prompt {
                    let room = 80usize.saturating_sub(p.text.chars().count());
                    p.text.extend(clean.chars().take(room));
                } else if self.lib.zone == Zone::Search && !clean.is_empty() {
                    self.lib.query.push_str(&clean);
                    self.query_changed();
                }
            }
            InputEvent::KeyUp { .. } => {}
        }
        out
    }

    fn lib_move(&mut self, x: f32, y: f32, now_us: i64, model: &UiModel, ctx: &LibCtx<'_>) {
        let moved = self.pointer.is_none_or(|(px, py)| (px - x).abs() + (py - y).abs() > 0.5);
        self.pointer = Some((x, y));
        if moved {
            self.last_activity = now_us;
            if self.keyboard_mode {
                self.keyboard_mode = false;
                self.dirty = true; // the focus ring goes away
            }
        }
        let g = self.lib_geom(model, ctx);
        match self.lib.drag {
            Some(LibDrag::Scroll { grab }) => {
                self.drag_scroll(y, grab, &g);
                self.dirty = true;
                return;
            }
            Some(LibDrag::Seek) => {
                self.scrub = Some(((x - g.seek_track.x) / g.seek_track.w.max(1.0)).clamp(0.0, 1.0));
                self.dirty = true;
                return;
            }
            Some(LibDrag::Volume) => return,
            Some(LibDrag::Reorder { from, to, grab_y, moved }) => {
                let moved = moved || (y - grab_y).abs() > 8.0 * self.scale;
                let mut to = to;
                if moved {
                    self.ensure_rows(model, ctx, &g);
                    if let Some(rows) = &self.lib.rows {
                        let by = y - g.m.body.y + self.lib.scroll;
                        let n = rows.ents.len();
                        // The entity under the pointer, or the end of the list when below it.
                        let target = rows.ent_at(g.m.pad + 1.0, by, &g.m).or_else(|| {
                            if by > rows.total - 40.0 * self.scale { n.checked_sub(1) } else { None }
                        });
                        if let Some(t) = target {
                            to = t;
                        }
                    }
                }
                self.lib.drag = Some(LibDrag::Reorder { from, to, grab_y, moved });
                self.dirty = true;
                return;
            }
            None => {}
        }
        let h = self.lib_hit(x, y, &g, model, ctx);
        self.hover_seek = if h == LibHit::Seek {
            Some(((x - g.seek_track.x) / g.seek_track.w.max(1.0)).clamp(0.0, 1.0))
        } else {
            None
        };
        if let LibHit::Menu(pi, ri) = h {
            if ri != usize::MAX {
                self.menu_hover(pi, ri);
            }
        }
        if h != self.lib.hover {
            self.lib.hover = h;
            self.hover_since = now_us;
            self.dirty = true;
        }
    }

    fn drag_scroll(&mut self, y: f32, grab: f32, g: &Geom) {
        let max = self.max_scroll(g);
        if max <= 0.0 {
            return;
        }
        let track = g.scroll_track;
        let total = self.lib.rows.as_ref().map_or(1.0, |r| r.total.max(1.0));
        let thumb_h = (track.h * g.m.body.h / total).max(36.0 * self.scale).min(track.h);
        let k = ((y - grab - track.y) / (track.h - thumb_h).max(1.0)).clamp(0.0, 1.0);
        self.lib.scroll = k * max;
    }

    #[allow(clippy::too_many_arguments)]
    fn lib_down(
        &mut self,
        x: f32,
        y: f32,
        button: PointerButton,
        now_us: i64,
        model: &UiModel,
        ctx: &LibCtx<'_>,
        out: &mut Vec<Action>,
    ) {
        self.pointer = Some((x, y));
        self.last_activity = now_us;
        self.keyboard_mode = false;
        let g = self.lib_geom(model, ctx);
        let hit = self.lib_hit(x, y, &g, model, ctx);
        match button {
            PointerButton::Secondary => {
                if self.lib.prompt.is_some() {
                    return;
                }
                let items = match hit {
                    LibHit::Ent(i) | LibHit::EntPlay(i) => {
                        self.lib.sel = Some(i);
                        menus::ent_menu(self, i, model, ctx)
                    }
                    LibHit::Rail(v) => menus::rail_menu(v),
                    LibHit::Folder(i) => menus::folder_menu(i),
                    LibHit::BarInfo | LibHit::Bar(_) | LibHit::Seek | LibHit::Volume => {
                        menus::global_menu(self, model, ctx)
                    }
                    _ => menus::global_menu(self, model, ctx),
                };
                self.open_menu_at(items, x, y, None);
                self.focus = None;
            }
            PointerButton::Middle => {
                self.menu.clear();
                if model.has_media() {
                    out.push(Action::PlayPause);
                }
            }
            PointerButton::Back => {
                self.menu.clear();
                self.go_back(model, ctx);
            }
            PointerButton::Forward => {
                self.menu.clear();
                out.push(Action::Next);
            }
            PointerButton::Primary => {
                self.pressed_lib = Some(hit);
                if !self.menu.is_empty() && !matches!(hit, LibHit::Menu(..)) {
                    // A click outside an open menu only closes it.
                    self.menu.clear();
                    self.pressed_lib = None;
                    return;
                }
                match hit {
                    LibHit::Ent(ei) | LibHit::EntPlay(ei)
                        if matches!(hit, LibHit::Ent(_))
                            && self.lib.rows.as_ref().is_some_and(|r| {
                                matches!(r.ents[ei].kind, EntKind::Queue(_) | EntKind::PlEntry { .. })
                            }) =>
                    {
                        self.lib.drag = Some(LibDrag::Reorder { from: ei, to: ei, grab_y: y, moved: false });
                    }
                    LibHit::Scrollbar => {
                        let total = self.lib.rows.as_ref().map_or(1.0, |r| r.total.max(1.0));
                        let track = g.scroll_track;
                        let max = self.max_scroll(&g).max(1.0);
                        let thumb_h = (track.h * g.m.body.h / total).max(36.0 * self.scale).min(track.h);
                        let thumb_y = track.y + (track.h - thumb_h) * (self.lib.scroll / max);
                        let grab =
                            if y >= thumb_y && y <= thumb_y + thumb_h { y - thumb_y } else { thumb_h * 0.5 };
                        self.lib.drag = Some(LibDrag::Scroll { grab });
                        self.drag_scroll(y, grab, &g);
                    }
                    LibHit::Seek => {
                        let f = ((x - g.seek_track.x) / g.seek_track.w.max(1.0)).clamp(0.0, 1.0);
                        self.lib.drag = Some(LibDrag::Seek);
                        self.scrub = Some(f);
                        out.push(Action::SeekFraction(f));
                    }
                    LibHit::Volume => {
                        self.lib.drag = Some(LibDrag::Volume);
                        out.push(Action::SetVolume(
                            ((x - g.vol_track.x) / g.vol_track.w.max(1.0)).clamp(0.0, 1.0),
                        ));
                    }
                    LibHit::Search => {
                        self.lib.zone = Zone::Search;
                    }
                    _ => {
                        if self.lib.zone == Zone::Search && hit != LibHit::SearchClear {
                            self.lib.zone = Zone::Content;
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lib_up(
        &mut self,
        x: f32,
        y: f32,
        button: PointerButton,
        now_us: i64,
        model: &UiModel,
        ctx: &LibCtx<'_>,
        out: &mut Vec<Action>,
    ) {
        self.pointer = Some((x, y));
        if button != PointerButton::Primary {
            return;
        }
        let g = self.lib_geom(model, ctx);
        let pressed = self.pressed_lib.take();
        match self.lib.drag.take() {
            Some(LibDrag::Seek) => {
                self.scrub = None;
                out.push(Action::SeekFraction(
                    ((x - g.seek_track.x) / g.seek_track.w.max(1.0)).clamp(0.0, 1.0),
                ));
                return;
            }
            Some(LibDrag::Volume) => {
                out.push(Action::SetVolume(((x - g.vol_track.x) / g.vol_track.w.max(1.0)).clamp(0.0, 1.0)));
                return;
            }
            Some(LibDrag::Scroll { .. }) => return,
            Some(LibDrag::Reorder { from, to, moved: true, .. }) => {
                // A drop on another row moves the item there.
                if let Some(rows) = &self.lib.rows {
                    if let (Some(a), true) = (rows.ents.get(from).copied(), to != from) {
                        let d = (to as i32 - from as i32).clamp(-100, 100) as i8;
                        match a.kind {
                            EntKind::Queue(id) => out.push(Action::MoveItem(id, d)),
                            EntKind::PlEntry { pl, idx } => {
                                out.push(Action::Lib(LibAction::MovePlaylistEntry(pl, idx as u32, d)))
                            }
                            _ => {}
                        }
                        self.lib.sel = Some(to);
                    }
                }
                return;
            }
            Some(LibDrag::Reorder { .. }) => {} // not moved: it was a click
            None => {}
        }
        let hit = self.lib_hit(x, y, &g, model, ctx);
        if pressed != Some(hit) {
            return;
        }
        match hit {
            LibHit::Menu(pi, ri) if ri != usize::MAX => {
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
            LibHit::PromptOk => self.confirm_prompt(),
            LibHit::PromptCancel => self.lib.prompt = None,
            LibHit::Rail(v) => self.show_view(v),
            LibHit::ModeSwitch(m) => out.push(Action::SetMode(m)),
            LibHit::AddFolder => out.push(Action::Lib(LibAction::AddFolder)),
            LibHit::Settings => out.push(Action::ShowSettings),
            LibHit::About => self.show_view(View::About),
            LibHit::Folder(i) => {
                if let Some(r) = ctx.lib.roots().get(i) {
                    let _ = r;
                    let items = menus::folder_menu(i);
                    self.open_menu_at(items, x, y, None);
                }
            }
            LibHit::Back => self.go_back(model, ctx),
            LibHit::SearchClear => {
                self.lib.query.clear();
                self.query_changed();
                if self.lib.view == View::Search {
                    self.leave_search();
                }
                self.lib.zone = Zone::Search;
            }
            LibHit::Search => self.lib.zone = Zone::Search,
            LibHit::Sort => {
                let items = menus::sort_menu(self);
                if let Some(r) = g.sort {
                    self.open_menu_at(items, r.x, r.bottom() + 4.0 * self.scale, None);
                }
            }
            LibHit::SortCol(c) => self.sort_by_column(c),
            LibHit::Button(id) => self.activate_button(id, model, ctx, out),
            LibHit::Bar(b) => self.activate_bar(b, model, ctx, out),
            LibHit::BarInfo => self.show_view(View::NowPlaying),
            LibHit::Viz(i) => out.push(match i {
                0 => Action::Lib(LibAction::VizStep(-1)),
                1 => Action::Lib(LibAction::VizStep(1)),
                2 => Action::Lib(LibAction::VizPalette),
                3 => Action::Lib(LibAction::VizInfo),
                5 => Action::Lib(LibAction::VizCycle),
                _ => Action::Lib(LibAction::VizToggle),
            }),
            LibHit::UpNext(i) => {
                if let Some(e) = self.up_next_items(model).get(i) {
                    out.push(Action::PlayItem(e.id));
                }
            }
            LibHit::Ent(i) => {
                self.lib.sel = Some(i);
                let double =
                    self.lib.last_click.is_some_and(|(li, t)| li == i && now_us - t <= DOUBLE_CLICK_US);
                self.lib.last_click = Some((i, now_us));
                let single_opens = self.lib.rows.as_ref().is_some_and(|r| {
                    matches!(r.ents[i].kind, EntKind::Album(_) | EntKind::Artist(_) | EntKind::Playlist(_))
                });
                if double || single_opens {
                    self.activate_ent(i, Enqueue::Now, model, ctx, out);
                }
            }
            LibHit::EntPlay(i) => {
                self.lib.sel = Some(i);
                self.play_ent(i, ctx, out);
            }
            LibHit::EntHeart(i) => {
                self.lib.sel = Some(i);
                let kind = self.lib.rows.as_ref().map(|r| r.ents[i].kind);
                if let Some(id) = kind.and_then(|k| self.ent_item(k, ctx, model)) {
                    out.push(Action::Lib(LibAction::ToggleFavorite(id)));
                }
            }
            _ => {}
        }
    }

    fn lib_wheel(&mut self, dy: f32, model: &UiModel, ctx: &LibCtx<'_>, out: &mut Vec<Action>) {
        let g = self.lib_geom(model, ctx);
        let hover = self.lib.hover;
        if matches!(hover, LibHit::Seek) {
            self.wheel_acc += dy;
            while self.wheel_acc.abs() >= 40.0 {
                let up = self.wheel_acc < 0.0;
                self.wheel_acc -= 40.0f32.copysign(self.wheel_acc);
                out.push(Action::SeekBy(if up { 5_000 } else { -5_000 }));
            }
            return;
        }
        let scrolls =
            !matches!(self.lib.view, View::NowPlaying | View::Visualizer) || self.lib.detail.is_some();
        if scrolls && !matches!(hover, LibHit::Volume | LibHit::Bar(_)) {
            self.ensure_rows(model, ctx, &g);
            let max = self.max_scroll(&g);
            self.lib.scroll = (self.lib.scroll + dy).clamp(0.0, max);
            return;
        }
        self.wheel_acc += dy;
        while self.wheel_acc.abs() >= 40.0 {
            let up = self.wheel_acc < 0.0;
            self.wheel_acc -= 40.0f32.copysign(self.wheel_acc);
            out.push(Action::VolumeBy(if up { 5 } else { -5 }));
        }
    }

    // ---- actions of the controls --------------------------------------------------------------------------------------

    fn sort_by_column(&mut self, c: u8) {
        use rvp_library::TrackSort as T;
        let by = match c {
            0 => T::Title,
            1 => T::Artist,
            2 => T::Album,
            _ => T::Duration,
        };
        if self.lib.track_sort == by {
            self.lib.track_asc = !self.lib.track_asc;
        } else {
            self.lib.track_sort = by;
            self.lib.track_asc = true;
        }
        self.lib.scroll = 0.0;
        self.lib.rows = None;
    }

    /// Set the sort of the Tracks view (from the sort menu).
    pub fn set_track_sort(&mut self, by: rvp_library::TrackSort, asc: bool) {
        self.lib.track_sort = by;
        self.lib.track_asc = asc;
        self.lib.scroll = 0.0;
        self.lib.rows = None;
        self.dirty = true;
    }

    fn activate_button(&mut self, id: u8, model: &UiModel, ctx: &LibCtx<'_>, out: &mut Vec<Action>) {
        let _ = ctx;
        match (self.lib.view, self.lib.detail, id) {
            (_, Some(d), id) => {
                let scope = match d {
                    Detail::Album(a) => Scope::Album(a),
                    Detail::Artist(a) => Scope::Artist(a),
                    Detail::Playlist(p) => Scope::Playlist(p),
                };
                match id {
                    0 => out.push(Action::Lib(LibAction::Play(scope, Enqueue::Now))),
                    1 => out.push(Action::Lib(LibAction::Play(scope, Enqueue::ShuffleNow))),
                    2 => out.push(Action::Lib(LibAction::Play(scope, Enqueue::Append))),
                    3 => out.push(Action::Lib(LibAction::FavoriteScope(scope))),
                    4 | 5 => {
                        if let Detail::Playlist(p) = d {
                            out.push(Action::Lib(LibAction::ExportPlaylist(p, id == 5)));
                        }
                    }
                    6 => {
                        if let Detail::Playlist(p) = d {
                            out.push(Action::Lib(LibAction::RenamePlaylist(p)));
                        }
                    }
                    7 => {
                        if let Detail::Playlist(p) = d {
                            out.push(Action::Lib(LibAction::DeletePlaylist(p)));
                            self.go_back(model, ctx);
                        }
                    }
                    _ => {}
                }
            }
            (View::Playlists, None, 0) => out.push(Action::Lib(LibAction::NewPlaylist)),
            (View::Playlists, None, 1) => out.push(Action::Lib(LibAction::ImportPlaylist)),
            (View::Queue, None, 0) => out.push(Action::ClearPlaylist),
            (View::Queue, None, 1) => out.push(Action::Lib(LibAction::NewPlaylistFrom(Scope::Queue))),
            (View::Tracks | View::Albums, None, 0) => {
                out.push(Action::Lib(LibAction::Play(Scope::AllTracks, Enqueue::ShuffleNow)))
            }
            (View::Videos, None, 0) => {
                self.lib.video_list = !self.lib.video_list;
                self.lib.sel = None;
                self.lib.scroll = 0.0;
                self.dirty = true;
            }
            (View::Videos, None, 1) => {
                // Title, then newest first, then longest first.
                let (sort, asc) = match (self.lib.video_sort, self.lib.video_asc) {
                    (VideoSort::Title, _) => (VideoSort::Added, false),
                    (VideoSort::Added, _) => (VideoSort::Length, false),
                    (VideoSort::Length, _) => (VideoSort::Title, true),
                };
                self.lib.video_sort = sort;
                self.lib.video_asc = asc;
                self.lib.scroll = 0.0;
                self.dirty = true;
            }
            (View::Favorites, None, 0) => {
                out.push(Action::Lib(LibAction::Play(Scope::ListFrom(0), Enqueue::Now)))
            }
            (View::Favorites, None, 1) => {
                out.push(Action::Lib(LibAction::Play(Scope::List, Enqueue::ShuffleNow)))
            }
            (View::About, None, id @ 0..=2) => out.push(Action::Lib(LibAction::About(id))),
            (View::NowPlaying, None, 0) => self.show_view(View::Albums),
            (_, None, 10) => out.push(Action::Lib(LibAction::AddFolder)),
            (_, None, 11) => out.push(Action::OpenFile),
            _ => {}
        }
        let _ = model;
    }

    fn activate_bar(&mut self, b: Btn, model: &UiModel, ctx: &LibCtx<'_>, out: &mut Vec<Action>) {
        match b {
            Btn::Play => out.push(Action::PlayPause),
            Btn::Prev => out.push(Action::Prev),
            Btn::Next => out.push(Action::Next),
            Btn::Shuffle => out.push(Action::ToggleShuffle),
            Btn::Repeat => out.push(Action::CycleRepeat),
            Btn::Mute => out.push(Action::ToggleMute),
            Btn::QueueView => self.show_view(View::Queue),
            Btn::VizView => self.toggle_visualizer(model, ctx),
            Btn::ModeSwitch => out.push(Action::SetMode(Mode::Player)),
            Btn::Favorite => out.push(Action::ToggleFavorite),
            _ => {}
        }
    }

    /// What Enter or a double click does to entity `i`: open it, or play it.
    fn activate_ent(
        &mut self,
        i: usize,
        how: Enqueue,
        model: &UiModel,
        ctx: &LibCtx<'_>,
        out: &mut Vec<Action>,
    ) {
        let _ = ctx;
        let Some(rows) = &self.lib.rows else { return };
        let Some(e) = rows.ents.get(i).copied() else { return };
        match e.kind {
            EntKind::Album(ai) => {
                if let Some(a) = ctx.lib.albums().get(ai) {
                    if how == Enqueue::Now {
                        self.open_detail(Detail::Album(a.id));
                    } else {
                        out.push(Action::Lib(LibAction::Play(Scope::Album(a.id), how)));
                    }
                }
            }
            EntKind::Artist(ai) => {
                if let Some(a) = ctx.lib.artists().get(ai) {
                    if how == Enqueue::Now {
                        self.open_detail(Detail::Artist(a.id));
                    } else {
                        out.push(Action::Lib(LibAction::Play(Scope::Artist(a.id), how)));
                    }
                }
            }
            EntKind::Playlist(id) => {
                if how == Enqueue::Now {
                    self.open_detail(Detail::Playlist(id));
                } else {
                    out.push(Action::Lib(LibAction::Play(Scope::Playlist(id), how)));
                }
            }
            EntKind::Track { id, pos } => match how {
                Enqueue::Now => {
                    out.push(Action::Lib(LibAction::Play(Scope::ListFrom(pos as u32), Enqueue::Now)))
                }
                h => out.push(Action::Lib(LibAction::Play(Scope::Track(id), h))),
            },
            EntKind::PlEntry { pl, idx } => match how {
                Enqueue::Now => {
                    out.push(Action::Lib(LibAction::Play(Scope::PlaylistFrom(pl, idx as u32), Enqueue::Now)))
                }
                h => {
                    if let Some(t) =
                        ctx.lib.playlist(pl).and_then(|p| p.entries.get(idx)).and_then(|e| e.track)
                    {
                        out.push(Action::Lib(LibAction::Play(Scope::Track(t), h)));
                    }
                }
            },
            EntKind::Queue(id) => match how {
                Enqueue::Now => out.push(Action::PlayItem(id)),
                _ => out.push(Action::Lib(LibAction::QueueToNext(id))),
            },
            EntKind::Video { id, pos } => match how {
                Enqueue::Now => {
                    out.push(Action::Lib(LibAction::Play(Scope::ListFrom(pos as u32), Enqueue::Now)))
                }
                h => out.push(Action::Lib(LibAction::Play(Scope::Video(id), h))),
            },
        }
        let _ = model;
    }

    /// The play button of an entity: play it right away.
    fn play_ent(&mut self, i: usize, ctx: &LibCtx<'_>, out: &mut Vec<Action>) {
        let Some(rows) = &self.lib.rows else { return };
        let Some(e) = rows.ents.get(i).copied() else { return };
        match e.kind {
            EntKind::Album(ai) => {
                if let Some(a) = ctx.lib.albums().get(ai) {
                    out.push(Action::Lib(LibAction::Play(Scope::Album(a.id), Enqueue::Now)));
                }
            }
            EntKind::Track { pos, .. } | EntKind::Video { pos, .. } => {
                out.push(Action::Lib(LibAction::Play(Scope::ListFrom(pos as u32), Enqueue::Now)))
            }
            EntKind::PlEntry { pl, idx } => {
                out.push(Action::Lib(LibAction::Play(Scope::PlaylistFrom(pl, idx as u32), Enqueue::Now)))
            }
            EntKind::Queue(id) => out.push(Action::PlayItem(id)),
            _ => {}
        }
    }

    pub(crate) fn up_next_items<'a>(&self, model: &'a UiModel) -> Vec<&'a crate::model::PlaylistEntry> {
        let cur = model.playlist.iter().position(|e| e.current);
        let start = cur.map_or(0, |c| c + 1);
        model.playlist.iter().skip(start).take(5).collect()
    }

    fn confirm_prompt(&mut self) {
        let Some(p) = self.lib.prompt.take() else { return };
        let name = String::from(p.text.trim());
        match p.kind {
            PromptKind::NewPlaylist(from) => self.lib.commands.push(UiCommand::CreatePlaylist { name, from }),
            PromptKind::Rename(id) => self.lib.commands.push(UiCommand::RenamePlaylist { id, name }),
        }
    }

    pub(crate) fn prompt_buttons(&self, g: &Geom) -> (RectF, RectF) {
        let s = self.scale;
        let card = self.prompt_card(g);
        let by = card.bottom() - 62.0 * s;
        (
            RectF::new(card.right() - 24.0 * s - 120.0 * s, by, 120.0 * s, 40.0 * s),
            RectF::new(card.right() - 24.0 * s - 120.0 * s - 12.0 * s - 100.0 * s, by, 100.0 * s, 40.0 * s),
        )
    }

    pub(crate) fn prompt_card(&self, g: &Geom) -> RectF {
        let s = self.scale;
        let (w, h) = (460.0 * s, 210.0 * s);
        RectF::new((g.m.w - w) * 0.5, (g.m.h - h) * 0.4, w, h)
    }

    // ---- keyboard ---------------------------------------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn lib_key(
        &mut self,
        key: &Key,
        mods: &Modifiers,
        repeat: bool,
        now_us: i64,
        model: &UiModel,
        ctx: &LibCtx<'_>,
        out: &mut Vec<Action>,
    ) {
        let _ = now_us;
        let g = self.lib_geom(model, ctx);
        if self.lib.prompt.is_some() {
            self.prompt_key(key, mods);
            return;
        }
        if !self.menu.is_empty() {
            self.menu_key(key, out);
            return;
        }
        if self.lib.zone == Zone::Search {
            self.search_key(key, mods);
            return;
        }
        let view = self.lib.view;
        let nav_view = !matches!(view, View::NowPlaying | View::Visualizer);
        let plain = !mods.ctrl && !mods.alt && !mods.logo;
        // Shift+V on the visualizer: cycle through the effects by itself, or stop.
        if view == View::Visualizer && plain && mods.shift && matches!(key, Key::Char('v' | 'V')) {
            return out.push(Action::Lib(LibAction::VizCycle));
        }
        // Keys that mean the same everywhere in the library.
        match key {
            Key::Escape => {
                if view == View::Visualizer
                    || self.lib.detail.is_some()
                    || !self.lib.history.is_empty()
                    || view == View::Search
                {
                    self.go_back(model, ctx);
                } else if self.lib.zone != Zone::Content {
                    self.lib.zone = Zone::Content;
                } else if model.fullscreen {
                    out.push(Action::ToggleFullscreen);
                }
                return;
            }
            Key::Other(n) if n == "F1" => {
                self.show_view(View::About);
                return;
            }
            Key::Char('/') if plain => {
                self.show_view(View::Search);
                return;
            }
            Key::Char('f') | Key::Char('F') if mods.ctrl => {
                self.show_view(View::Search);
                return;
            }
            Key::Char(c @ '1'..='9') if plain && !mods.shift => {
                self.show_view(DIGITS[(*c as u8 - b'1') as usize]);
                return;
            }
            Key::Other(n) if n == "Backspace" => {
                self.go_back(model, ctx);
                return;
            }
            Key::Other(n) if n == "Tab" => {
                self.cycle_zone(mods.shift);
                return;
            }
            Key::Other(n) if n == "ContextMenu" || (n == "F10" && mods.shift) => {
                self.open_selection_menu(&g, model, ctx);
                return;
            }
            _ => {}
        }
        match self.lib.zone {
            Zone::Rail => {
                self.rail_key(key, &g);
                return;
            }
            Zone::Bar => {
                self.bar_key(key, &g, model, ctx, out);
                return;
            }
            _ => {}
        }
        // Content. H hearts the selected song or video (what is playing when nothing is selected: the shortcut table's own action).
        if plain && !mods.shift && matches!(key, Key::Char('h' | 'H')) {
            self.ensure_rows(model, ctx, &g);
            let item = self
                .lib
                .sel
                .and_then(|i| self.lib.rows.as_ref().and_then(|r| r.ents.get(i)))
                .and_then(|e| self.ent_item(e.kind, ctx, model));
            if let Some(id) = item {
                return out.push(Action::Lib(LibAction::ToggleFavorite(id)));
            }
        }
        if view == View::Visualizer && plain && !mods.shift {
            match key {
                Key::Left => return out.push(Action::Lib(LibAction::VizStep(-1))),
                Key::Right => return out.push(Action::Lib(LibAction::VizStep(1))),
                Key::Enter => return out.push(Action::Lib(LibAction::VizToggle)),
                Key::Char('c') | Key::Char('C') => return out.push(Action::Lib(LibAction::VizPalette)),
                Key::Char('t') | Key::Char('T') => return out.push(Action::Lib(LibAction::VizInfo)),
                _ => {}
            }
        }
        if nav_view
            && (plain || (mods.alt && !mods.ctrl) || (mods.ctrl && matches!(key, Key::Enter)))
            && !(mods.shift && matches!(key, Key::Left | Key::Right))
            && self.list_key(key, mods, &g, model, ctx, out)
        {
            return;
        }
        if view == View::NowPlaying && matches!(key, Key::Enter) {
            out.push(Action::PlayPause);
            return;
        }
        if let Some(a) = shortcut_for(key, mods) {
            let repeatable = matches!(
                a,
                Action::SeekBy(_) | Action::VolumeBy(_) | Action::SpeedStep(_) | Action::FrameStep(_)
            );
            if !repeat || repeatable {
                out.push(a);
            }
        }
    }

    /// Keys that move around a list or grid and act on its selection. True if the key was used.
    #[allow(clippy::too_many_arguments)]
    fn list_key(
        &mut self,
        key: &Key,
        mods: &Modifiers,
        g: &Geom,
        model: &UiModel,
        ctx: &LibCtx<'_>,
        out: &mut Vec<Action>,
    ) -> bool {
        self.ensure_rows(model, ctx, g);
        let page = ((g.m.body.h / (56.0 * self.scale)) as i32).max(1);
        let cur = self.lib.sel;
        let Some(rows) = &self.lib.rows else { return false };
        let n = rows.ents.len();
        let next = match key {
            Key::Down => rows.step(cur, 0, 1),
            Key::Up => rows.step(cur, 0, -1),
            Key::Right => rows.step(cur, 1, 0),
            Key::Left => rows.step(cur, -1, 0),
            Key::Home => (n > 0).then_some(0),
            Key::End => n.checked_sub(1),
            Key::Other(k) if k == "PageDown" => {
                let mut c = cur;
                for _ in 0..page {
                    c = rows.step(c, 0, 1);
                }
                c
            }
            Key::Other(k) if k == "PageUp" => {
                let mut c = cur;
                for _ in 0..page {
                    c = rows.step(c, 0, -1);
                }
                c
            }
            _ => None,
        };
        let is_nav = matches!(key, Key::Down | Key::Up | Key::Left | Key::Right | Key::Home | Key::End)
            || matches!(key, Key::Other(k) if k == "PageDown" || k == "PageUp");
        if is_nav {
            if mods.alt {
                // Alt+Up/Down moves an item of the queue or a playlist.
                if let (Some(i), Key::Up | Key::Down) = (cur, key) {
                    let d = if matches!(key, Key::Up) { -1 } else { 1 };
                    if let Some(e) = rows.ents.get(i) {
                        match e.kind {
                            EntKind::Queue(id) => {
                                out.push(Action::MoveItem(id, d));
                                self.lib.sel = Some((i as i32 + d as i32).clamp(0, n as i32 - 1) as usize);
                            }
                            EntKind::PlEntry { pl, idx } => {
                                out.push(Action::Lib(LibAction::MovePlaylistEntry(pl, idx as u32, d)));
                                self.lib.sel = Some((i as i32 + d as i32).clamp(0, n as i32 - 1) as usize);
                            }
                            _ => {}
                        }
                    }
                }
                return true;
            }
            if let Some(i) = next {
                self.lib.sel = Some(i);
                self.reveal(i, g);
            }
            return true;
        }
        let Some(i) = cur else {
            return matches!(key, Key::Enter);
        };
        match key {
            Key::Enter => {
                let how = if mods.shift {
                    Enqueue::Append
                } else if mods.ctrl {
                    Enqueue::Next
                } else {
                    Enqueue::Now
                };
                self.activate_ent(i, how, model, ctx, out);
                true
            }
            Key::Other(k) if k == "Delete" => {
                if let Some(e) = self.lib.rows.as_ref().and_then(|r| r.ents.get(i)).copied() {
                    match e.kind {
                        EntKind::Queue(id) => out.push(Action::RemoveItem(id)),
                        EntKind::PlEntry { pl, idx } => {
                            out.push(Action::Lib(LibAction::RemoveFromPlaylist(pl, idx as u32)))
                        }
                        _ => {}
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn search_key(&mut self, key: &Key, mods: &Modifiers) {
        match key {
            Key::Escape => {
                if !self.lib.query.is_empty() {
                    self.lib.query.clear();
                    self.query_changed();
                    if self.lib.view == View::Search {
                        self.leave_search();
                    }
                } else if self.lib.view == View::Search {
                    self.leave_search();
                }
                self.lib.zone = Zone::Content;
            }
            Key::Enter | Key::Down => {
                self.lib.zone = Zone::Content;
                self.lib.sel = None;
            }
            Key::Space => {
                self.lib.query.push(' ');
                self.query_changed();
            }
            Key::Char(c) if !mods.ctrl && !mods.alt => {
                self.lib.query.push(*c);
                self.query_changed();
            }
            Key::Other(n) if n == "Backspace" => {
                self.lib.query.pop();
                self.query_changed();
                if self.lib.query.is_empty() && self.lib.view == View::Search {
                    // Keep the Search view open with its hint; Escape goes back.
                }
            }
            Key::Other(n) if n == "Tab" => {
                self.lib.zone = Zone::Content;
            }
            _ => {}
        }
    }

    fn prompt_key(&mut self, key: &Key, mods: &Modifiers) {
        let Some(p) = &mut self.lib.prompt else { return };
        match key {
            Key::Escape => self.lib.prompt = None,
            Key::Enter => self.confirm_prompt(),
            Key::Space => p.text.push(' '),
            Key::Char(c) if !mods.ctrl && !mods.alt => {
                if p.text.chars().count() < 80 {
                    p.text.push(*c);
                }
            }
            Key::Other(n) if n == "Backspace" => {
                p.text.pop();
            }
            _ => {}
        }
    }

    fn cycle_zone(&mut self, back: bool) {
        let order = [Zone::Content, Zone::Rail, Zone::Bar];
        let cur = order.iter().position(|z| *z == self.lib.zone).unwrap_or(0);
        let n = order.len();
        let next = if back { (cur + n - 1) % n } else { (cur + 1) % n };
        self.lib.zone = order[next];
        if self.lib.zone == Zone::Rail {
            self.lib.rail_focus =
                super::geom::NAV.iter().flatten().position(|(v, ..)| *v == self.lib.view).unwrap_or(0);
        }
    }

    fn rail_key(&mut self, key: &Key, g: &Geom) {
        let n = g.nav.len().max(1);
        match key {
            Key::Down => self.lib.rail_focus = (self.lib.rail_focus + 1) % n,
            Key::Up => self.lib.rail_focus = (self.lib.rail_focus + n - 1) % n,
            Key::Home => self.lib.rail_focus = 0,
            Key::End => self.lib.rail_focus = n - 1,
            Key::Enter | Key::Space | Key::Right => {
                if let Some((v, _)) = g.nav.get(self.lib.rail_focus) {
                    let v = *v;
                    self.show_view(v);
                    if v != View::Search {
                        self.lib.zone = Zone::Content;
                    }
                }
            }
            Key::Left => self.lib.zone = Zone::Content,
            _ => {}
        }
    }

    fn bar_key(&mut self, key: &Key, g: &Geom, model: &UiModel, ctx: &LibCtx<'_>, out: &mut Vec<Action>) {
        let n = g.bar_btns.len().max(1);
        match key {
            Key::Right | Key::Down => self.lib.bar_focus = (self.lib.bar_focus + 1) % n,
            Key::Left | Key::Up => self.lib.bar_focus = (self.lib.bar_focus + n - 1) % n,
            Key::Home => self.lib.bar_focus = 0,
            Key::End => self.lib.bar_focus = n - 1,
            Key::Enter | Key::Space => {
                if let Some((b, _)) = g.bar_btns.get(self.lib.bar_focus) {
                    let b = *b;
                    self.activate_bar(b, model, ctx, out);
                }
            }
            _ => {}
        }
    }

    fn open_selection_menu(&mut self, g: &Geom, model: &UiModel, ctx: &LibCtx<'_>) {
        self.ensure_rows(model, ctx, g);
        let items;
        let (x, y);
        if let (Some(i), Some(rows)) = (self.lib.sel, &self.lib.rows) {
            if i < rows.ents.len() {
                let r = rows.ent_rect(i, &g.m);
                x = g.m.body.x + r.x + 40.0 * self.scale;
                y = g.m.body.y + r.y - self.lib.scroll + r.h * 0.6;
                items = menus::ent_menu(self, i, model, ctx);
                let first = items.iter().position(|m: &MenuItem| m.enabled);
                self.open_menu_at(items, x, y, first);
                return;
            }
        }
        let (px, py) = self.pointer.unwrap_or((self.w as f32 * 0.5, self.h as f32 * 0.5));
        let items = menus::global_menu(self, model, ctx);
        let first = items.iter().position(|m| m.enabled);
        self.open_menu_at(items, px, py, first);
    }
}
