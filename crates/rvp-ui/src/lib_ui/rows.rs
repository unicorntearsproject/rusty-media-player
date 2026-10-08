//! Laying out a view as rows: which rows exist, how tall they are, and which entities (cards, rows) sit in them. Built once per
//! change of the library, the view, the search text, the sort or the window width; drawing and hit testing only look at it.
use super::{Detail, LibCtx, LibUi, Metrics, View};
use crate::model::UiModel;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;
use rvp_library::{TrackSort, VideoSort};

/// What an entity is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntKind {
    /// An artist (index into the artists).
    Artist(usize),
    /// An album (index into the albums).
    Album(usize),
    /// A track in a list; `pos` counts the playable tracks of the list.
    Track {
        /// Library id.
        id: u32,
        /// Position among the list's tracks.
        pos: usize,
    },
    /// A video (library id); `pos` counts the videos of the list.
    Video {
        /// Library id.
        id: u32,
        /// Position among the list's videos.
        pos: usize,
    },
    /// An item of the queue (playlist item id).
    Queue(u32),
    /// A saved playlist (id).
    Playlist(u32),
    /// An entry of a saved playlist.
    PlEntry {
        /// The playlist.
        pl: u32,
        /// Index of the entry.
        idx: usize,
    },
}

/// A selectable thing and where it sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ent {
    /// What it is.
    pub kind: EntKind,
    /// Row it is in.
    pub row: usize,
    /// Column in the row (0 for lists).
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RowKind {
    /// The big block at the top of an album, artist or playlist.
    Hero,
    /// A section title.
    Header(String),
    /// Empty space.
    Gap,
    /// Entities (a grid row has several, a list row one).
    Items(Range<usize>),
    /// A full-width message (an empty view).
    Message(String, String),
    /// The About page (one tall block; `draw_about` lays it out).
    About,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub y: f32,
    pub h: f32,
    pub kind: RowKind,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowsKey {
    pub rev: u64,
    pub view: View,
    pub detail: Option<Detail>,
    pub query: String,
    pub sort: (TrackSort, bool),
    pub video: (bool, VideoSort, bool),
    pub player_empty: bool,
    pub width: u32,
    pub queue: u64,
    pub scale: u32,
}

/// The rows of one view.
#[derive(Debug, Clone)]
pub(crate) struct Rows {
    pub key: RowsKey,
    pub rows: Vec<Row>,
    pub ents: Vec<Ent>,
    pub total: f32,
    /// Library ids of the playable tracks (or videos) of the list, in order (what a double click on a row queues from).
    pub list: Vec<u32>,
    /// The Videos view is laid out as a list.
    pub video_list: bool,
    /// In the History view: what each entity is a play of (by entity index).
    pub hist: BTreeMap<usize, HistInfo>,
}

/// The play behind a row of the History view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HistInfo {
    /// The play, for removing it.
    pub seq: u64,
    /// When it started to count (seconds since 1970).
    pub at: i64,
    /// How many plays of the same file the history holds.
    pub count: u32,
}

struct Builder<'a> {
    s: f32,
    cols: usize,
    rows: Vec<Row>,
    ents: Vec<Ent>,
    y: f32,
    list: Vec<u32>,
    card_h: f32,
    /// The Videos view is a list (else posters).
    video_list: bool,
    video_cols: usize,
    vcard_h: f32,
    hist: BTreeMap<usize, HistInfo>,
    _p: core::marker::PhantomData<&'a ()>,
}

impl Builder<'_> {
    fn push(&mut self, h: f32, kind: RowKind) {
        self.rows.push(Row { y: self.y, h, kind });
        self.y += h;
    }

    fn list_row(&mut self, h: f32, kind: EntKind) {
        let start = self.ents.len();
        self.ents.push(Ent { kind, row: self.rows.len(), col: 0 });
        self.push(h, RowKind::Items(start..start + 1));
    }

    fn grid<I: Iterator<Item = EntKind>>(&mut self, items: I) {
        let items: Vec<EntKind> = items.collect();
        for chunk in items.chunks(self.cols.max(1)) {
            let start = self.ents.len();
            for (c, k) in chunk.iter().enumerate() {
                self.ents.push(Ent { kind: *k, row: self.rows.len(), col: c });
            }
            self.push(self.card_h + 20.0 * self.s, RowKind::Items(start..start + chunk.len()));
        }
    }

    /// A video: a poster card in a grid, or a row in a list.
    fn videos<I: Iterator<Item = u32>>(&mut self, ids: I) {
        let ids: Vec<u32> = ids.collect();
        if self.video_list {
            for id in ids {
                let pos = self.list.len();
                self.list.push(id);
                self.list_row(64.0 * self.s, EntKind::Video { id, pos });
            }
            return;
        }
        for chunk in ids.chunks(self.video_cols.max(1)) {
            let start = self.ents.len();
            for (c, id) in chunk.iter().enumerate() {
                let pos = self.list.len();
                self.list.push(*id);
                self.ents.push(Ent { kind: EntKind::Video { id: *id, pos }, row: self.rows.len(), col: c });
            }
            self.push(self.vcard_h + 20.0 * self.s, RowKind::Items(start..start + chunk.len()));
        }
    }

    fn track(&mut self, id: u32) {
        let pos = self.list.len();
        self.list.push(id);
        self.list_row(48.0 * self.s, EntKind::Track { id, pos });
    }
}

/// Lay out the current view.
pub(crate) fn build(ui: &LibUi, model: &UiModel, ctx: &LibCtx<'_>, m: &Metrics, key: RowsKey) -> Rows {
    let s = m.s;
    let lib = ctx.lib;
    let mut b = Builder {
        s,
        cols: m.grid_cols(),
        rows: Vec::new(),
        ents: Vec::new(),
        y: 0.0,
        list: Vec::new(),
        card_h: m.card_h(),
        video_list: ui.video_list,
        video_cols: m.video_cols(),
        vcard_h: m.vcard_h(),
        hist: BTreeMap::new(),
        _p: Default::default(),
    };
    let empty_lib = lib.track_count() == 0;
    // The Player face with nothing loaded: the open-a-video card, inside the same frame.
    if ui.player_empty && ui.mode == super::Mode::Player {
        b.push(
            340.0 * s,
            RowKind::Message(
                "Drop a video here".into(),
                "MP4, MKV and WebM. Everything plays right here on your device.".into(),
            ),
        );
        b.push(24.0 * s, RowKind::Gap);
        return Rows {
            key,
            rows: b.rows,
            ents: b.ents,
            total: b.y,
            list: b.list,
            video_list: ui.video_list,
            hist: b.hist,
        };
    }
    match (ui.view, ui.detail) {
        (_, Some(Detail::Album(id))) => {
            if let Some(a) = lib.album(id) {
                b.push(252.0 * s, RowKind::Hero);
                let mut disc = 0;
                for &t in &a.tracks {
                    if a.discs > 1 {
                        let d = lib.track(t).map_or(1, |t| t.disc_no.max(1));
                        if d != disc {
                            disc = d;
                            b.push(40.0 * s, RowKind::Header(alloc::format!("Disc {d}")));
                        }
                    }
                    b.track(t);
                }
            } else {
                b.push(
                    260.0 * s,
                    RowKind::Message("That album is gone.".into(), "Rescan the folder, or go back.".into()),
                );
            }
        }
        (_, Some(Detail::Artist(id))) => {
            if let Some(a) = lib.artist(id) {
                b.push(252.0 * s, RowKind::Hero);
                b.push(44.0 * s, RowKind::Header(alloc::format!("Albums ({})", a.albums.len())));
                b.grid(a.albums.iter().map(|&i| EntKind::Album(i)));
            } else {
                b.push(
                    260.0 * s,
                    RowKind::Message("That artist is gone.".into(), "Rescan the folder, or go back.".into()),
                );
            }
        }
        (_, Some(Detail::Playlist(id))) => {
            if let Some(p) = lib.playlist(id) {
                b.push(252.0 * s, RowKind::Hero);
                if p.entries.is_empty() {
                    b.push(
                        180.0 * s,
                        RowKind::Message(
                            "This playlist is empty.".into(),
                            "Right-click an album or a track and add it here.".into(),
                        ),
                    );
                }
                for (i, e) in p.entries.iter().enumerate() {
                    match e.track {
                        Some(t) => {
                            let pos = b.list.len();
                            b.list.push(t);
                            let _ = pos;
                            b.list_row(48.0 * s, EntKind::PlEntry { pl: id, idx: i });
                        }
                        None => b.list_row(48.0 * s, EntKind::PlEntry { pl: id, idx: i }),
                    }
                }
            } else {
                b.push(260.0 * s, RowKind::Message("That playlist is gone.".into(), "Go back.".into()));
            }
        }
        (View::Albums, None) => {
            if lib.albums().is_empty() {
                b.push(
                    300.0 * s,
                    RowKind::Message(
                        if empty_lib { "Your music goes here.".into() } else { "Nothing to show.".into() },
                        "Add a folder to build your library. Everything stays on this device.".into(),
                    ),
                );
            } else {
                b.push(8.0 * s, RowKind::Gap);
                b.grid((0..lib.albums().len()).map(EntKind::Album));
            }
        }
        (View::Artists, None) => {
            if lib.artists().is_empty() {
                b.push(
                    300.0 * s,
                    RowKind::Message("No artists yet.".into(), "Add a folder to build your library.".into()),
                );
            }
            for i in 0..lib.artists().len() {
                b.list_row(68.0 * s, EntKind::Artist(i));
            }
        }
        (View::Tracks, None) => {
            if empty_lib {
                b.push(
                    300.0 * s,
                    RowKind::Message("No tracks yet.".into(), "Add a folder to build your library.".into()),
                );
            }
            for id in lib.sorted_tracks(ui.track_sort, ui.track_asc) {
                b.track(id);
            }
        }
        (View::Videos, None) => {
            if lib.video_count() == 0 {
                b.push(
                    300.0 * s,
                    RowKind::Message(
                        "Your videos go here.".into(),
                        "Add a folder with films or clips (MP4, MKV or WebM). Everything stays on this device.".into(),
                    ),
                );
            } else {
                b.push(8.0 * s, RowKind::Gap);
                b.videos(lib.sorted_videos(ui.video_sort, ui.video_asc).into_iter());
            }
        }
        (View::Favorites, None) => {
            let (songs, films) = (lib.favorite_tracks(), lib.favorite_videos());
            if songs.is_empty() && films.is_empty() {
                b.push(
                    300.0 * s,
                    RowKind::Message(
                        "Nothing hearted yet.".into(),
                        "Click the heart on a song or a video, or press Ctrl+F, and it lands here.".into(),
                    ),
                );
            }
            if !songs.is_empty() {
                b.push(44.0 * s, RowKind::Header(alloc::format!("Music ({})", songs.len())));
                for id in songs {
                    b.track(id);
                }
            }
            if !films.is_empty() {
                b.push(44.0 * s, RowKind::Header(alloc::format!("Videos ({})", films.len())));
                b.videos(films.into_iter());
            }
        }
        (View::History, None) => {
            let mut any = false;
            for video in [false, true] {
                let plays: Vec<_> = lib.history_rows(video).into_iter().filter(|r| r.id.is_some()).collect();
                if plays.is_empty() {
                    continue;
                }
                any = true;
                let what = if video { "Videos" } else { "Music" };
                b.push(
                    44.0 * s,
                    RowKind::Header(alloc::format!(
                        "{what} ({})",
                        super::draw::plural(plays.len(), "play", "plays")
                    )),
                );
                let mut day: Option<String> = None;
                for p in plays {
                    let label = day_label(lib, p.play.at);
                    if day.as_ref() != Some(&label) {
                        b.push(32.0 * s, RowKind::Header(label.clone()));
                        day = Some(label);
                    }
                    let Some(id) = p.id else { continue };
                    b.hist.insert(b.ents.len(), HistInfo { seq: p.play.seq, at: p.play.at, count: p.count });
                    if video {
                        let keep = core::mem::replace(&mut b.video_list, true);
                        b.videos(core::iter::once(id));
                        b.video_list = keep;
                    } else {
                        b.track(id);
                    }
                }
            }
            if !any {
                b.push(
                    300.0 * s,
                    RowKind::Message(
                        "Nothing played yet.".into(),
                        "Songs and videos you play land here, newest first, once you have listened for half a minute.".into(),
                    ),
                );
            }
        }
        (View::Playlists, None) => {
            if lib.playlists().is_empty() {
                b.push(
                    300.0 * s,
                    RowKind::Message(
                        "No playlists yet.".into(),
                        "Make one from the queue, an album, or import an M3U, M3U8 or PLS file.".into(),
                    ),
                );
            }
            for p in lib.playlists() {
                b.list_row(68.0 * s, EntKind::Playlist(p.id));
            }
        }
        (View::Queue, None) => {
            if model.playlist.is_empty() {
                b.push(
                    300.0 * s,
                    RowKind::Message(
                        "The queue is empty.".into(),
                        "Play an album, or add tracks from the context menu.".into(),
                    ),
                );
            }
            for e in model.playlist.iter() {
                b.list_row(58.0 * s, EntKind::Queue(e.id));
            }
        }
        (View::Search, None) => {
            let q = ui.query.trim();
            if q.is_empty() {
                b.push(
                    260.0 * s,
                    RowKind::Message(
                        "Search your library.".into(),
                        "Type an artist, an album or a track. Accents and case do not matter.".into(),
                    ),
                );
            } else {
                let r = lib.search(q);
                if r.tracks.is_empty() && r.albums.is_empty() && r.artists.is_empty() && r.videos.is_empty() {
                    b.push(260.0 * s, RowKind::Message("Nothing found.".into(), "Try fewer words.".into()));
                }
                if !r.artists.is_empty() {
                    b.push(44.0 * s, RowKind::Header(alloc::format!("Artists ({})", r.artists.len())));
                    for &i in r.artists.iter().take(6) {
                        b.list_row(68.0 * s, EntKind::Artist(i));
                    }
                }
                if !r.albums.is_empty() {
                    b.push(44.0 * s, RowKind::Header(alloc::format!("Albums ({})", r.albums.len())));
                    b.grid(r.albums.iter().take(b.cols * 2).map(|&i| EntKind::Album(i)));
                }
                if !r.videos.is_empty() {
                    b.push(44.0 * s, RowKind::Header(alloc::format!("Videos ({})", r.videos.len())));
                    b.videos(r.videos.iter().copied().take(if ui.video_list {
                        50
                    } else {
                        b.video_cols * 2
                    }));
                }
                if !r.tracks.is_empty() {
                    b.push(44.0 * s, RowKind::Header(alloc::format!("Tracks ({})", r.tracks.len())));
                    for &t in r.tracks.iter().take(200) {
                        b.track(t);
                    }
                }
            }
        }
        (View::About, None) => {
            // The height is an estimate from the length of the paragraphs (the fonts are not at hand here); drawing lays the block out
            // properly and has room to spare.
            let w = (m.body.w - 2.0 * m.pad).clamp(200.0 * s, 680.0 * s);
            let chars: usize = super::about::PARAGRAPHS.iter().map(|(_, p)| p.len()).sum();
            let lines = libm::ceilf(chars as f32 * 8.2 * s / w) + 6.0;
            b.push(260.0 * s + lines * 24.0 * s + 3.0 * 70.0 * s, RowKind::About);
        }
        (View::NowPlaying | View::Visualizer, None) => {}
    }
    b.push(24.0 * s, RowKind::Gap);
    Rows {
        key,
        rows: b.rows,
        ents: b.ents,
        total: b.y,
        list: b.list,
        video_list: ui.video_list,
        hist: b.hist,
    }
}

/// The heart of an entity drawn in `r` (a rectangle of [`Rows::ent_rect`], or the same moved by the scroll), if it has one: songs
/// and queue rows keep it left of the time, video rows at the right end, poster cards in the poster's top right corner.
pub(crate) fn heart_rect(
    kind: EntKind,
    video_list: bool,
    r: crate::gfx::RectF,
    s: f32,
) -> Option<crate::gfx::RectF> {
    use crate::gfx::RectF;
    match kind {
        EntKind::Track { .. } | EntKind::PlEntry { .. } | EntKind::Queue(_) => {
            Some(RectF::new(r.right() - 98.0 * s, r.cy() - 14.0 * s, 28.0 * s, 28.0 * s))
        }
        EntKind::Video { .. } if video_list => {
            Some(RectF::new(r.right() - 44.0 * s, r.cy() - 14.0 * s, 28.0 * s, 28.0 * s))
        }
        EntKind::Video { .. } => Some(RectF::new(r.right() - 36.0 * s, r.y + 8.0 * s, 28.0 * s, 28.0 * s)),
        _ => None,
    }
}

impl Rows {
    /// The entity under (`x`, `y`) given in body coordinates (`y` already includes the scroll), with the card rectangle's
    /// column geometry supplied by the caller.
    pub(crate) fn ent_at(&self, x: f32, y: f32, m: &Metrics) -> Option<usize> {
        let ri = self.rows.partition_point(|r| r.y + r.h <= y);
        let row = self.rows.get(ri)?;
        let RowKind::Items(range) = &row.kind else { return None };
        if range.len() == 1 && self.is_list(range.start) {
            return (y >= row.y && y < row.y + row.h).then_some(range.start);
        }
        // A grid row: cards of a fixed width with gaps, starting at the padding (posters are wider than covers).
        let video = matches!(self.ents[range.start].kind, EntKind::Video { .. });
        let (cw, gap) = (if video { m.vcard_w() } else { m.card_w() }, 20.0 * m.s);
        let rx = x - m.pad;
        if rx < 0.0 {
            return None;
        }
        let col = (rx / (cw + gap)) as usize;
        if rx - col as f32 * (cw + gap) > cw {
            return None;
        }
        if y - row.y > if video { m.vcard_h() } else { m.card_h() } {
            return None;
        }
        (col < range.len()).then_some(range.start + col)
    }

    /// True for entities that fill a row (everything but album and poster cards).
    pub(crate) fn is_list(&self, ent: usize) -> bool {
        match self.ents[ent].kind {
            EntKind::Album(_) => false,
            EntKind::Video { .. } => self.video_list,
            _ => true,
        }
    }

    /// The rectangle of an entity in body coordinates (scroll not applied).
    pub(crate) fn ent_rect(&self, ent: usize, m: &Metrics) -> crate::gfx::RectF {
        let e = &self.ents[ent];
        let row = &self.rows[e.row];
        if self.is_list(ent) {
            crate::gfx::RectF::new(m.pad - 8.0 * m.s, row.y, m.body.w - 2.0 * m.pad + 16.0 * m.s, row.h)
        } else if matches!(e.kind, EntKind::Video { .. }) {
            let (cw, gap) = (m.vcard_w(), 20.0 * m.s);
            crate::gfx::RectF::new(m.pad + e.col as f32 * (cw + gap), row.y, cw, m.vcard_h())
        } else {
            let (cw, gap) = (m.card_w(), 20.0 * m.s);
            crate::gfx::RectF::new(m.pad + e.col as f32 * (cw + gap), row.y, cw, m.card_h())
        }
    }

    /// The entity reached from `from` by a key: a step along the row (`dx`) or to the row above or below (`dy`), keeping the column.
    pub(crate) fn step(&self, from: Option<usize>, dx: i32, dy: i32) -> Option<usize> {
        if self.ents.is_empty() {
            return None;
        }
        let Some(cur) = from.filter(|c| *c < self.ents.len()) else { return Some(0) };
        if dx != 0 {
            let n = cur as i32 + dx;
            let n = n.clamp(0, self.ents.len() as i32 - 1) as usize;
            // Horizontal steps stay within the row for grids; for lists they do nothing.
            return Some(if self.ents[n].row == self.ents[cur].row { n } else { cur });
        }
        let e = self.ents[cur];
        let mut best = cur;
        let target_col = e.col;
        let mut row = e.row as i32 + dy.signum();
        while row >= 0 && (row as usize) < self.rows.len() {
            if let RowKind::Items(r) = &self.rows[row as usize].kind {
                let c = target_col.min(r.len() - 1);
                best = r.start + c;
                break;
            }
            row += dy.signum();
        }
        Some(best)
    }
}

const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// The heading of the day a play was on: Today, Yesterday, a weekday-less date, or Earlier when the host has no calendar.
pub(crate) fn day_label(lib: &rvp_library::Library, at: i64) -> String {
    match lib.days_ago(at) {
        Some(0) => "Today".into(),
        Some(1) => "Yesterday".into(),
        Some(_) => {
            let (y, m, d, ..) = lib.local_date(at);
            alloc::format!("{d} {} {y}", MONTHS[(m as usize).clamp(1, 12) - 1])
        }
        None => "Earlier".into(),
    }
}

/// "14:32" in the host's local time (empty without a calendar).
pub(crate) fn clock_text(lib: &rvp_library::Library, at: i64) -> String {
    if lib.days_ago(at).is_none() {
        return String::new();
    }
    let (.., h, mi) = lib.local_date(at);
    alloc::format!("{h:02}:{mi:02}")
}
