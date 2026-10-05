//! The audio-first views: the library (albums, artists, tracks, playlists, search), the queue, the now-playing screen and the
//! visualizer, as a second mode of the same UI ("Library" next to "Player").
//!
//! The state lives in [`LibUi`] inside [`crate::Ui`]; the data it draws comes in with every call as a [`LibCtx`] (the library
//! index, the decoded cover of what is playing, scan progress, the visualizer's picture), so nothing here owns or copies the
//! library. Lists are virtual: only the rows on screen are laid out in pixels and drawn.
use crate::gfx::RectF;
use crate::model::UiModel;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;
pub use rvp_library::{Image, Library, ListFormat, ScanStatus, TrackSort};
use rvp_viz::Viz;

mod draw;
mod geom;
mod input;
mod menus;
pub(crate) mod rows;
mod snapshot;

pub use rows::{Ent, EntKind};

/// Which of the two faces of the app is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// The video player: the picture is the hero.
    #[default]
    Player,
    /// The audio-first library.
    Library,
}

/// The views of the library mode (the rail's entries).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// The cover, the title and what is next.
    NowPlaying,
    /// Albums as a grid of covers.
    #[default]
    Albums,
    /// Artists.
    Artists,
    /// Every track, sortable.
    Tracks,
    /// Saved playlists.
    Playlists,
    /// What plays next.
    Queue,
    /// Results for what was typed in the search box.
    Search,
    /// The full-window visualizer.
    Visualizer,
}

/// What is opened inside a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// An album and its tracks.
    Album(u32),
    /// An artist and their albums.
    Artist(u32),
    /// A saved playlist.
    Playlist(u32),
}

/// A set of tracks an action applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// One track (library id).
    Track(u32),
    /// An album (id).
    Album(u32),
    /// All of an artist's albums.
    Artist(u32),
    /// A whole saved playlist.
    Playlist(u32),
    /// A saved playlist, from the entry at this index on.
    PlaylistFrom(u32, u32),
    /// Every track in the library, in the order of the Tracks view.
    AllTracks,
    /// The list on screen, from this position on (what a double click on a row means).
    ListFrom(u32),
    /// The whole list on screen.
    List,
    /// The queue (to save it as a playlist).
    Queue,
}

/// What to do with the tracks of a [`Scope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueue {
    /// Replace the queue and start playing the first one.
    Now,
    /// Replace the queue, shuffled, and start.
    ShuffleNow,
    /// Insert right after what is playing.
    Next,
    /// Add to the end of the queue.
    Append,
}

/// An action of the library mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibAction {
    /// Play, queue or insert tracks.
    Play(Scope, Enqueue),
    /// Append tracks to a saved playlist (id).
    AddToPlaylist(u32, Scope),
    /// Ask for a name, then make a new playlist from the tracks.
    NewPlaylistFrom(Scope),
    /// Ask for a name, then make a new empty playlist.
    NewPlaylist,
    /// Pick a folder to add to the library.
    AddFolder,
    /// Scan a folder again (root index; `u32::MAX` for all).
    Rescan(u32),
    /// Take a folder out of the library (root index).
    ForgetFolder(u32),
    /// Pick a playlist file to import.
    ImportPlaylist,
    /// Save a playlist as a file (id, PLS instead of M3U8).
    ExportPlaylist(u32, bool),
    /// Delete a saved playlist.
    DeletePlaylist(u32),
    /// Ask for a new name for a playlist.
    RenamePlaylist(u32),
    /// Remove entry `index` of a playlist.
    RemoveFromPlaylist(u32, u32),
    /// Move entry `index` of a playlist by a place.
    MovePlaylistEntry(u32, u32, i8),
    /// Make a queue item play next.
    QueueToNext(u32),
    /// Sort the track list.
    SortTracks(TrackSort, bool),
    /// Visualizer: the next (+1) or previous (-1) effect.
    VizStep(i8),
    /// Visualizer: the next colour scheme.
    VizPalette,
    /// Visualizer: show or hide the title overlay.
    VizInfo,
    /// Visualizer: turn the animation on or off (it starts off with reduced motion).
    VizToggle,
}

/// Something the UI collected that carries text, which an [`crate::Action`] cannot.
#[derive(Debug, Clone, PartialEq)]
pub enum UiCommand {
    /// The name prompt for a new playlist was confirmed.
    CreatePlaylist {
        /// The name.
        name: String,
        /// The tracks to start it with (`None`: empty).
        from: Option<Scope>,
    },
    /// The name prompt for renaming was confirmed.
    RenamePlaylist {
        /// The playlist.
        id: u32,
        /// The new name.
        name: String,
    },
}

/// What the library views draw from, supplied by the application with every call.
pub struct LibCtx<'a> {
    /// The index.
    pub lib: &'a Library,
    /// The decoded cover of what is playing (from its own tags, so it works for files outside the library too).
    pub now_art: Option<&'a Image>,
    /// A scan in progress.
    pub scan: Option<&'a ScanStatus>,
    /// The visualizer's picture and state.
    pub viz: Option<&'a Viz>,
    /// The latest video frame (RGBA, width, height) when the item playing has a picture.
    pub video: Option<(&'a [u8], u32, u32)>,
}

/// A text prompt over the screen.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Prompt {
    pub title: String,
    pub text: String,
    pub kind: PromptKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PromptKind {
    NewPlaylist(Option<Scope>),
    Rename(u32),
}

/// What the pointer is over in library mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibHit {
    /// Nothing that does anything.
    #[default]
    None,
    /// A rail entry.
    Rail(View),
    /// The Library / Player switch.
    ModeSwitch(Mode),
    /// The add-folder button in the rail.
    AddFolder,
    /// A folder row in the rail (root index).
    Folder(usize),
    /// The search box.
    Search,
    /// The clear button of the search box.
    SearchClear,
    /// The sort button.
    Sort,
    /// A column head of the track table (0 title, 1 artist, 2 album, 3 time).
    SortCol(u8),
    /// The back button.
    Back,
    /// A row or card (index into the entities).
    Ent(usize),
    /// The play button on a row or card.
    EntPlay(usize),
    /// The scroll bar.
    Scrollbar,
    /// A button in a hero block or header (index).
    Button(u8),
    /// The cover or title in the bar (opens now playing).
    BarInfo,
    /// A transport button in the bar.
    Bar(crate::ui::Btn),
    /// The seek bar.
    Seek,
    /// The volume slider.
    Volume,
    /// A visualizer switcher button (0 previous, 1 next, 2 palette, 3 info, 4 animation on/off).
    Viz(u8),
    /// A queued item in the now-playing list (queue position).
    UpNext(usize),
    /// A row of an open menu.
    Menu(usize, usize),
    /// The prompt's OK.
    PromptOk,
    /// The prompt's cancel.
    PromptCancel,
}

/// Which part of the screen the keyboard drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Zone {
    #[default]
    Content,
    Rail,
    Bar,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LibDrag {
    Scroll { grab: f32 },
    Seek,
    Volume,
}

/// Where Back goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NavEntry {
    pub view: View,
    pub detail: Option<Detail>,
    pub scroll: f32,
    pub sel: Option<usize>,
}

/// The library mode's share of the UI state.
pub struct LibUi {
    pub(crate) mode: Mode,
    pub(crate) view: View,
    pub(crate) detail: Option<Detail>,
    pub(crate) history: Vec<NavEntry>,
    pub(crate) query: String,
    pub(crate) track_sort: TrackSort,
    pub(crate) track_asc: bool,
    pub(crate) sel: Option<usize>,
    pub(crate) scroll: f32,
    pub(crate) zone: Zone,
    pub(crate) rail_focus: usize,
    pub(crate) bar_focus: usize,
    pub(crate) hover: LibHit,
    pub(crate) rows: Option<rows::Rows>,
    pub(crate) thumbs: BTreeMap<u64, Rc<(u32, u32, Vec<u8>)>>,
    pub(crate) prompt: Option<Prompt>,
    pub(crate) commands: Vec<UiCommand>,
    pub(crate) viz_info: bool,
    pub(crate) viz_on: bool,
    pub(crate) drag: Option<LibDrag>,
    pub(crate) last_click: Option<(usize, i64)>,
    /// The view before the search was typed into.
    pub(crate) before_search: Option<(View, Option<Detail>)>,
    /// Last time a frame with an animated indicator was drawn.
    pub(crate) anim_at: i64,
    /// Library revision the thumbnails cache was built for.
    pub(crate) thumbs_rev: u64,
    /// Something on screen is animated (equaliser bars).
    pub(crate) animated: bool,
    /// The caret's blink phase as last drawn.
    pub(crate) blink: bool,
    /// The geometry of the last frame drawn and the entities that were on screen (for tooling and tests).
    pub(crate) last_geom: Option<geom::Geom>,
    pub(crate) visible: Vec<(usize, RectF)>,
}

impl Default for LibUi {
    fn default() -> Self {
        Self {
            mode: Mode::Player,
            view: View::Albums,
            detail: None,
            history: Vec::new(),
            query: String::new(),
            track_sort: TrackSort::Title,
            track_asc: true,
            sel: None,
            scroll: 0.0,
            zone: Zone::Content,
            rail_focus: 1,
            bar_focus: 0,
            hover: LibHit::None,
            rows: None,
            thumbs: BTreeMap::new(),
            prompt: None,
            commands: Vec::new(),
            viz_info: true,
            viz_on: true,
            drag: None,
            last_click: None,
            before_search: None,
            anim_at: 0,
            thumbs_rev: 0,
            animated: false,
            blink: false,
            last_geom: None,
            visible: Vec::new(),
        }
    }
}

impl LibUi {
    /// The current mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The current view.
    pub fn view(&self) -> View {
        self.view
    }

    /// What is open inside the view.
    pub fn detail(&self) -> Option<Detail> {
        self.detail
    }

    /// What is typed in the search box.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Index (into the entities of the view) of the selected row or card.
    pub fn selected(&self) -> Option<usize> {
        self.sel
    }

    /// True while a text field has the keyboard (the page must not use letters as shortcuts then).
    pub fn typing(&self) -> bool {
        self.mode == Mode::Library && (self.zone == Zone::Search || self.prompt.is_some())
    }

    /// Whether the visualizer animation is on.
    pub fn viz_on(&self) -> bool {
        self.viz_on
    }

    /// Whether the visualizer view shows the title overlay.
    pub fn viz_info(&self) -> bool {
        self.viz_info
    }

    /// Entities of the view as laid out last (for tests and tooling).
    pub fn entities(&self) -> &[Ent] {
        self.rows.as_ref().map_or(&[], |r| &r.ents)
    }
}

/// Pixel metrics of the library screen for a window.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Metrics {
    pub s: f32,
    pub w: f32,
    pub h: f32,
    pub compact: bool,
    pub rail: RectF,
    pub bar: RectF,
    pub content: RectF,
    pub header: RectF,
    pub table_head: Option<RectF>,
    pub body: RectF,
    pub pad: f32,
}

impl Metrics {
    pub(crate) fn new(w: f32, h: f32, s: f32, view: View, detail: Option<Detail>) -> Self {
        let compact = w < 860.0 * s;
        let rail_w = if compact { 76.0 * s } else { 236.0 * s };
        let full = view == View::Visualizer;
        let bar_h = 96.0 * s;
        // The visualizer fills the window; its controls float over the bottom.
        let below = if full { 0.0 } else { bar_h };
        let rail =
            if full { RectF::new(0.0, 0.0, 0.0, 0.0) } else { RectF::new(0.0, 0.0, rail_w, h - below) };
        let bar = RectF::new(0.0, h - bar_h, w, bar_h);
        let cx = if full { 0.0 } else { rail_w };
        let content = RectF::new(cx, 0.0, w - cx, h - below);
        let header_h = if full { 0.0 } else { 92.0 * s };
        let header = RectF::new(content.x, 0.0, content.w, header_h);
        let table_head = (view == View::Tracks && detail.is_none())
            .then(|| RectF::new(content.x, header_h, content.w, 34.0 * s));
        let body_top = header_h + table_head.map_or(0.0, |t| t.h);
        let body = RectF::new(content.x, body_top, content.w, (content.h - body_top).max(0.0));
        Self { s, w, h, compact, rail, bar, content, header, table_head, body, pad: 28.0 * s }
    }

    /// Cards per grid row in the body.
    pub(crate) fn grid_cols(&self) -> usize {
        let (cw, gap) = (self.card_w(), 20.0 * self.s);
        (libm::floorf((self.body.w - 2.0 * self.pad + gap) / (cw + gap)) as usize).max(1)
    }

    pub(crate) fn card_w(&self) -> f32 {
        // Wider windows get a bit larger covers, but never fewer than two per row.
        let base = 176.0 * self.s;
        let avail = self.body.w - 2.0 * self.pad;
        let cols = libm::floorf((avail + 20.0 * self.s) / (base + 20.0 * self.s)).max(2.0);
        let w = (avail - (cols - 1.0) * 20.0 * self.s) / cols;
        w.clamp(120.0 * self.s, 232.0 * self.s)
    }

    pub(crate) fn card_h(&self) -> f32 {
        self.card_w() + 54.0 * self.s
    }
}

/// A virtual list's slice that is on screen: the row indexes whose pixels intersect the body.
pub(crate) fn visible_rows(rows: &rows::Rows, scroll: f32, view_h: f32) -> Range<usize> {
    let top = scroll;
    let bottom = scroll + view_h;
    let first = rows.rows.partition_point(|r| r.y + r.h <= top);
    let last = rows.rows.partition_point(|r| r.y < bottom);
    first..last.max(first)
}

/// Whether anything in `model` or `ctx` says the rows must be built again.
pub(crate) fn rows_key(ui: &LibUi, model: &UiModel, ctx: &LibCtx<'_>, m: &Metrics) -> rows::RowsKey {
    rows::RowsKey {
        rev: ctx.lib.revision(),
        view: ui.view,
        detail: ui.detail,
        query: ui.query.clone(),
        sort: (ui.track_sort, ui.track_asc),
        width: m.body.w as u32,
        queue: model.queue_rev,
        scale: (m.s * 100.0) as u32,
    }
}

#[cfg(test)]
mod tests;
