//! Laying out a view as rows: which rows exist, how tall they are, and which entities (cards, rows) sit in them. Built once per
//! change of the library, the view, the search text, the sort or the window width; drawing and hit testing only look at it.
use super::{Detail, LibCtx, LibUi, Metrics, View};
use crate::model::UiModel;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;
use rvp_library::TrackSort;

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
    /// Library ids of the playable tracks of the list, in order (what a double click on a row queues from).
    pub list: Vec<u32>,
}

struct Builder<'a> {
    s: f32,
    cols: usize,
    rows: Vec<Row>,
    ents: Vec<Ent>,
    y: f32,
    list: Vec<u32>,
    card_h: f32,
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
        _p: Default::default(),
    };
    let empty_lib = lib.track_count() == 0;
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
                if r.tracks.is_empty() && r.albums.is_empty() && r.artists.is_empty() {
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
                if !r.tracks.is_empty() {
                    b.push(44.0 * s, RowKind::Header(alloc::format!("Tracks ({})", r.tracks.len())));
                    for &t in r.tracks.iter().take(200) {
                        b.track(t);
                    }
                }
            }
        }
        (View::NowPlaying | View::Visualizer, None) => {}
    }
    b.push(24.0 * s, RowKind::Gap);
    Rows { key, rows: b.rows, ents: b.ents, total: b.y, list: b.list }
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
        // A grid row: cards of a fixed width with gaps, starting at the padding.
        let (cw, gap) = (m.card_w(), 20.0 * m.s);
        let rx = x - m.pad;
        if rx < 0.0 {
            return None;
        }
        let col = (rx / (cw + gap)) as usize;
        if rx - col as f32 * (cw + gap) > cw {
            return None;
        }
        if y - row.y > m.card_h() {
            return None;
        }
        (col < range.len()).then_some(range.start + col)
    }

    /// True for entities that fill a row (everything but album cards).
    pub(crate) fn is_list(&self, ent: usize) -> bool {
        !matches!(self.ents[ent].kind, EntKind::Album(_))
    }

    /// The rectangle of an entity in body coordinates (scroll not applied).
    pub(crate) fn ent_rect(&self, ent: usize, m: &Metrics) -> crate::gfx::RectF {
        let e = &self.ents[ent];
        let row = &self.rows[e.row];
        if self.is_list(ent) {
            crate::gfx::RectF::new(m.pad - 8.0 * m.s, row.y, m.body.w - 2.0 * m.pad + 16.0 * m.s, row.h)
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
