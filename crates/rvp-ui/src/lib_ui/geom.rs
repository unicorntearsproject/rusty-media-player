//! Where everything of the library screen is, in pixels: the rail, the header, the bar, the columns of a track row, the hero
//! block's buttons. Computed from the window size, the view and the data; shared by drawing and hit testing.
use super::{Detail, LibCtx, Metrics, View};
use crate::font::Face;
use crate::gfx::RectF;
use crate::icon::Icon;
use crate::model::UiModel;
use crate::ui::{Btn, Ui};
use alloc::string::String;
use alloc::vec::Vec;

/// A button drawn as a pill with a label and an icon.
#[derive(Debug, Clone)]
pub(crate) struct PillBtn {
    pub id: u8,
    pub rect: RectF,
    pub label: String,
    pub icon: Icon,
    pub primary: bool,
}

/// Columns of a track row, as x offsets from the row's left edge and widths.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TrackCols {
    pub num: (f32, f32),
    pub thumb: (f32, f32),
    pub title: (f32, f32),
    pub artist: Option<(f32, f32)>,
    pub album: Option<(f32, f32)>,
    pub time: (f32, f32),
}

pub(crate) fn track_cols(w: f32, s: f32, thumbs: bool) -> TrackCols {
    let pad = 8.0 * s;
    let num = (pad, 36.0 * s);
    let mut x = pad + num.1 + 4.0 * s;
    let thumb = (x, if thumbs { 36.0 * s } else { 0.0 });
    if thumbs {
        x += thumb.1 + 12.0 * s;
    }
    let time = (w - pad - 56.0 * s, 56.0 * s);
    let avail = time.0 - x - 12.0 * s;
    let (artist, album, title);
    if w > 980.0 * s {
        let a = avail * 0.24;
        let b = avail * 0.28;
        title = (x, avail - a - b - 24.0 * s);
        artist = Some((x + title.1 + 12.0 * s, a));
        album = Some((x + title.1 + a + 24.0 * s, b));
    } else if w > 640.0 * s {
        let a = avail * 0.32;
        title = (x, avail - a - 12.0 * s);
        artist = Some((x + title.1 + 12.0 * s, a));
        album = None;
    } else {
        title = (x, avail);
        artist = None;
        album = None;
    }
    TrackCols { num, thumb, title, artist, album, time }
}

/// All the rectangles of the screen.
#[derive(Debug, Clone)]
pub(crate) struct Geom {
    pub m: Metrics,
    pub nav: Vec<(View, RectF)>,
    pub mode: [RectF; 2],
    pub add_folder: RectF,
    pub folders: Vec<(usize, RectF)>,
    pub back: Option<RectF>,
    pub search: RectF,
    pub search_clear: RectF,
    pub sort: Option<RectF>,
    pub header_btns: Vec<PillBtn>,
    pub title_x: f32,
    pub table_cols: Vec<(u8, RectF)>,
    pub bar_btns: Vec<(Btn, RectF)>,
    pub bar_art: RectF,
    pub bar_info: RectF,
    pub seek_hit: RectF,
    pub seek_track: RectF,
    pub vol_hit: Option<RectF>,
    pub vol_track: RectF,
    pub time_l: f32,
    pub time_r: f32,
    pub time_y: f32,
    pub viz_btns: Vec<(u8, RectF)>,
    pub scroll_track: RectF,
}

/// The rail's entries, top to bottom (a `None` is a divider).
pub(crate) const NAV: [Option<(View, &str, Icon, &str)>; 9] = [
    Some((View::Search, "Search", Icon::Search, "/")),
    Some((View::NowPlaying, "Now playing", Icon::AudioLines, "6")),
    Some((View::Albums, "Albums", Icon::Disc3, "1")),
    Some((View::Artists, "Artists", Icon::MicVocal, "2")),
    Some((View::Tracks, "Tracks", Icon::Music, "3")),
    Some((View::Playlists, "Playlists", Icon::ListMusic, "4")),
    Some((View::Queue, "Queue", Icon::List, "5")),
    None,
    Some((View::Visualizer, "Visualizer", Icon::Sparkles, "V")),
];

impl Ui {
    /// Measure and lay out the whole library screen.
    pub(crate) fn lib_geom(&mut self, model: &UiModel, ctx: &LibCtx<'_>) -> Geom {
        let s = self.scale;
        let (w, h) = (self.w as f32, self.h as f32);
        let m = Metrics::new(w, h, s, self.lib.view, self.lib.detail);
        let mut g = Geom {
            m,
            nav: Vec::new(),
            mode: [RectF::default(); 2],
            add_folder: RectF::default(),
            folders: Vec::new(),
            back: None,
            search: RectF::default(),
            search_clear: RectF::default(),
            sort: None,
            header_btns: Vec::new(),
            title_x: 0.0,
            table_cols: Vec::new(),
            bar_btns: Vec::new(),
            bar_art: RectF::default(),
            bar_info: RectF::default(),
            seek_hit: RectF::default(),
            seek_track: RectF::default(),
            vol_hit: None,
            vol_track: RectF::default(),
            time_l: 0.0,
            time_r: 0.0,
            time_y: 0.0,
            viz_btns: Vec::new(),
            scroll_track: RectF::default(),
        };
        let full = self.lib.view == View::Visualizer;
        if !full {
            self.geom_rail(&mut g, ctx);
            self.geom_header(&mut g, model);
        }
        self.geom_bar(&mut g, model);
        if full {
            self.geom_viz(&mut g);
        }
        let b = g.m.body;
        g.scroll_track = RectF::new(b.right() - 14.0 * s, b.y + 6.0 * s, 8.0 * s, (b.h - 12.0 * s).max(0.0));
        g
    }

    fn geom_rail(&mut self, g: &mut Geom, ctx: &LibCtx<'_>) {
        let s = self.scale;
        let r = g.m.rail;
        let compact = g.m.compact;
        let px = if compact { 10.0 * s } else { 16.0 * s };
        let iw = r.w - 2.0 * px;
        // The Library / Player switch.
        let top = if compact { 16.0 * s } else { 74.0 * s };
        if compact {
            g.mode = [RectF::new(px, top, iw, 36.0 * s), RectF::new(px, top + 40.0 * s, iw, 36.0 * s)];
        } else {
            let sw = RectF::new(px, top, iw, 38.0 * s);
            g.mode = [
                RectF::new(sw.x + 3.0 * s, sw.y + 3.0 * s, sw.w * 0.5 - 3.0 * s, sw.h - 6.0 * s),
                RectF::new(sw.cx(), sw.y + 3.0 * s, sw.w * 0.5 - 3.0 * s, sw.h - 6.0 * s),
            ];
        }
        let mut y = top + if compact { 92.0 * s } else { 58.0 * s };
        let item_h = if compact { 44.0 * s } else { 42.0 * s };
        for it in NAV.iter() {
            match it {
                Some((v, ..)) => {
                    g.nav.push((*v, RectF::new(px, y, iw, item_h)));
                    y += item_h + 2.0 * s;
                }
                None => y += 14.0 * s,
            }
        }
        // Folders (not in the compact rail) and the add button.
        let bottom = r.bottom() - 16.0 * s;
        let btn_h = 40.0 * s;
        g.add_folder = RectF::new(px, bottom - btn_h, iw, btn_h);
        if !compact {
            let mut fy = y + 26.0 * s;
            for (i, _) in ctx.lib.roots().iter().enumerate() {
                if fy + 28.0 * s > g.add_folder.y - 8.0 * s {
                    break;
                }
                g.folders.push((i, RectF::new(px, fy, iw, 28.0 * s)));
                fy += 30.0 * s;
            }
        }
    }

    fn geom_header(&mut self, g: &mut Geom, model: &UiModel) {
        let s = self.scale;
        let hd = g.m.header;
        let pad = g.m.pad;
        let mut x = hd.x + pad;
        if self.lib.detail.is_some() || !self.lib.history.is_empty() {
            g.back = Some(RectF::new(x - 4.0 * s, hd.y + 24.0 * s, 40.0 * s, 40.0 * s));
            x += 44.0 * s;
        }
        g.title_x = x;
        // Search box on the right, then the sort button and the view's actions to its left.
        let sw = (hd.w * 0.30).clamp(190.0 * s, 330.0 * s);
        g.search = RectF::new(hd.right() - pad - sw, hd.y + 26.0 * s, sw, 40.0 * s);
        g.search_clear = RectF::new(g.search.right() - 34.0 * s, g.search.y + 6.0 * s, 28.0 * s, 28.0 * s);
        let mut rx = g.search.x - 12.0 * s;
        let btns: Vec<(u8, &str, Icon, bool)> = match (self.lib.view, self.lib.detail) {
            (_, Some(_)) => Vec::new(),
            (View::Playlists, _) => {
                alloc::vec![(1, "Import", Icon::Upload, false), (0, "New playlist", Icon::Plus, true)]
            }
            (View::Queue, _) => {
                alloc::vec![(1, "Save as playlist", Icon::ListPlus, false), (0, "Clear", Icon::Trash2, false)]
            }
            (View::Tracks | View::Albums, _) => alloc::vec![(0, "Shuffle all", Icon::Shuffle, true)],
            _ => Vec::new(),
        };
        let compact = hd.w < 900.0 * s;
        for (id, label, icon, primary) in btns {
            let tw = if compact { 0.0 } else { self.text_w(Face::SansMedium, 13.0, label, 0.0) + 8.0 * s };
            let bw = if compact { 40.0 * s } else { tw + 50.0 * s };
            let r = RectF::new(rx - bw, hd.y + 26.0 * s, bw, 40.0 * s);
            g.header_btns.push(PillBtn {
                id,
                rect: r,
                label: if compact { String::new() } else { label.into() },
                icon,
                primary,
            });
            rx = r.x - 10.0 * s;
        }
        if self.lib.view == View::Tracks && self.lib.detail.is_none() {
            let label = self.sort_label();
            let tw = self.text_w(Face::SansMedium, 13.0, &label, 0.0);
            let bw = tw + 48.0 * s;
            if rx - bw > x + 160.0 * s {
                g.sort = Some(RectF::new(rx - bw, hd.y + 26.0 * s, bw, 40.0 * s));
            }
        }
        // The table head of the Tracks view.
        if let Some(th) = g.m.table_head {
            let cols = track_cols(th.w - 2.0 * (g.m.pad - 8.0 * s), s, true);
            let x0 = g.m.pad - 8.0 * s + th.x;
            g.table_cols.push((0, RectF::new(x0 + cols.title.0, th.y, cols.title.1, th.h)));
            if let Some(a) = cols.artist {
                g.table_cols.push((1, RectF::new(x0 + a.0, th.y, a.1, th.h)));
            }
            if let Some(a) = cols.album {
                g.table_cols.push((2, RectF::new(x0 + a.0, th.y, a.1, th.h)));
            }
            g.table_cols
                .push((3, RectF::new(x0 + cols.time.0 - 24.0 * s, th.y, cols.time.1 + 24.0 * s, th.h)));
        }
        let _ = model;
    }

    fn geom_bar(&mut self, g: &mut Geom, model: &UiModel) {
        let s = self.scale;
        let b = g.m.bar;
        let (w, y0) = (b.w, b.y);
        let compact = g.m.compact;
        let pad = 20.0 * s;
        // Left: cover and text.
        g.bar_art = RectF::new(pad, y0 + 16.0 * s, 64.0 * s, 64.0 * s);
        let cw = (w * 0.38).clamp(360.0 * s, 660.0 * s).min(w - 2.0 * pad);
        let cx0 = (w - cw) * 0.5;
        g.bar_info = RectF::new(
            pad,
            y0 + 12.0 * s,
            if compact { 64.0 * s } else { (cx0 - pad - 16.0 * s).max(64.0 * s) },
            72.0 * s,
        );
        // Centre: transport.
        let cy = y0 + 34.0 * s;
        let small = 36.0 * s;
        let big = 46.0 * s;
        let gap = 10.0 * s;
        let total = small * 4.0 + big + gap * 4.0;
        let mut x = w * 0.5 - total * 0.5;
        for (btn, size) in [
            (Btn::Shuffle, small),
            (Btn::Prev, small),
            (Btn::Play, big),
            (Btn::Next, small),
            (Btn::Repeat, small),
        ] {
            g.bar_btns.push((btn, RectF::new(x, cy - size * 0.5, size, size)));
            x += size + gap;
        }
        // Seek bar with the times at its ends.
        g.time_y = y0 + 74.0 * s;
        let tw = 46.0 * s;
        g.time_l = cx0;
        g.time_r = cx0 + cw - tw;
        g.seek_track = RectF::new(cx0 + tw + 8.0 * s, g.time_y - 2.0 * s, cw - 2.0 * tw - 16.0 * s, 4.0 * s);
        g.seek_hit = RectF::new(g.seek_track.x, g.time_y - 12.0 * s, g.seek_track.w, 24.0 * s);
        // Right: volume, queue, visualizer, mode.
        let mut rx = w - pad;
        let cy2 = y0 + 48.0 * s;
        for btn in [Btn::ModeSwitch, Btn::VizView, Btn::QueueView] {
            g.bar_btns.push((btn, RectF::new(rx - small, cy2 - small * 0.5, small, small)));
            rx -= small + 4.0 * s;
        }
        rx -= 10.0 * s;
        if !compact && w > 1000.0 * s {
            let vw = 92.0 * s;
            g.vol_hit = Some(RectF::new(rx - vw, cy2 - 14.0 * s, vw, 28.0 * s));
            g.vol_track = RectF::new(rx - vw + 6.0 * s, cy2 - 2.0 * s, vw - 12.0 * s, 4.0 * s);
            rx -= vw + 6.0 * s;
        }
        g.bar_btns.push((Btn::Mute, RectF::new(rx - small, cy2 - small * 0.5, small, small)));
        let _ = model;
    }

    fn geom_viz(&mut self, g: &mut Geom) {
        let s = self.scale;
        // The switcher: previous, next, palette, info, animation, centred above the bar.
        let w = g.m.w;
        let bw = 44.0 * s;
        let total = bw * 5.0 + 8.0 * s * 4.0 + 40.0 * s;
        let mut x = (w - total) * 0.5;
        let y = g.m.bar.y - 70.0 * s;
        for (i, wid) in [(0u8, bw), (1, bw), (2, bw), (3, bw), (4, bw)] {
            g.viz_btns.push((i, RectF::new(x, y, wid, 40.0 * s)));
            x += wid + 8.0 * s + if i == 1 { 40.0 * s } else { 0.0 };
        }
    }

    /// The hero block's buttons for a detail view, `rect` being the block on screen.
    pub(crate) fn hero_buttons(&mut self, rect: RectF, detail: Detail, _ctx: &LibCtx<'_>) -> Vec<PillBtn> {
        let s = self.scale;
        let x0 = rect.x + 28.0 * s + 196.0 * s + 28.0 * s;
        let y = rect.bottom() - 64.0 * s;
        let defs: Vec<(u8, &str, Icon, bool)> = match detail {
            Detail::Album(_) | Detail::Artist(_) => {
                alloc::vec![
                    (0, "Play", Icon::Play, true),
                    (1, "Shuffle", Icon::Shuffle, false),
                    (2, "Add to queue", Icon::ListEnd, false)
                ]
            }
            Detail::Playlist(_) => alloc::vec![
                (0, "Play", Icon::Play, true),
                (1, "Shuffle", Icon::Shuffle, false),
                (2, "Add to queue", Icon::ListEnd, false),
                (4, "Export M3U8", Icon::Download, false),
                (5, "PLS", Icon::Download, false),
                (6, "Rename", Icon::Pencil, false),
                (7, "Delete", Icon::Trash2, false),
            ],
        };
        let mut x = x0;
        let mut out = Vec::new();
        for (id, label, icon, primary) in defs {
            let tw = self.text_w(Face::SansMedium, 13.0, label, 0.0) + 8.0 * s;
            let bw = tw + 50.0 * s;
            if x + bw > rect.right() - 20.0 * s && !out.is_empty() {
                break;
            }
            out.push(PillBtn {
                id,
                rect: RectF::new(x, y, bw, 40.0 * s),
                label: label.into(),
                icon,
                primary,
            });
            x += bw + 10.0 * s;
        }
        out
    }

    /// Text on the sort button.
    pub(crate) fn sort_label(&self) -> String {
        use rvp_library::TrackSort as T;
        let name = match self.lib.track_sort {
            T::Title => "Title",
            T::Artist => "Artist",
            T::Album => "Album",
            T::Duration => "Length",
            T::Year => "Year",
            T::Added => "Added",
        };
        alloc::format!("Sort: {name} {}", if self.lib.track_asc { "(A-Z)" } else { "(Z-A)" })
    }
}
