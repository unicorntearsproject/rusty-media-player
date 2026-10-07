//! Where everything of the library screen is, in pixels: the rail, the header, the bar, the columns of a track row, the hero
//! block's buttons. Computed from the window size, the view and the data; shared by drawing and hit testing.
use super::{Detail, LibCtx, Metrics, VideoSort, View};
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

pub(crate) fn track_cols(w: f32, s: f32, thumbs: bool, want_artist: bool, want_album: bool) -> TrackCols {
    let pad = 8.0 * s;
    let num = (pad, 36.0 * s);
    let mut x = pad + num.1 + 4.0 * s;
    let thumb = (x, if thumbs { 36.0 * s } else { 0.0 });
    if thumbs {
        x += thumb.1 + 12.0 * s;
    }
    let time = (w - pad - 56.0 * s, 56.0 * s);
    // The heart sits left of the time (see `rows::heart_rect`).
    let avail = time.0 - 34.0 * s - x - 12.0 * s;
    let (artist, album, title);
    if !want_artist && !want_album {
        title = (x, avail);
        artist = None;
        album = None;
    } else if !want_album && w > 640.0 * s {
        let a = avail * 0.34;
        title = (x, avail - a - 12.0 * s);
        artist = Some((x + title.1 + 12.0 * s, a));
        album = None;
    } else if w > 980.0 * s {
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
    /// The Settings button under it.
    pub settings: RectF,
    /// The About button, last.
    pub about: RectF,
    pub folders: Vec<(usize, RectF)>,
    /// Folders that do not fit under the others: a "+N more" line after the last row.
    pub folders_more: usize,
    pub back: Option<RectF>,
    /// The menu button that opens the rail as a drawer (phone width only).
    pub menu_btn: Option<RectF>,
    pub search: RectF,
    pub search_clear: RectF,
    /// The search box is only its icon (a narrow window, not focused).
    pub search_collapsed: bool,
    pub sort: Option<RectF>,
    pub header_btns: Vec<PillBtn>,
    pub title_x: f32,
    pub table_cols: Vec<(u8, RectF)>,
    pub bar_btns: Vec<(Btn, RectF)>,
    pub bar_art: RectF,
    /// Where the bar's title, artist and idle text sit (their centres; a phone's bar has its own rows).
    pub bar_text_y: [f32; 3],
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
pub(crate) const NAV: [Option<(View, &str, Icon, &str)>; 12] = [
    Some((View::Search, "Search", Icon::Search, "/")),
    Some((View::NowPlaying, "Now playing", Icon::AudioLines, "6")),
    Some((View::Albums, "Albums", Icon::Disc3, "1")),
    Some((View::Artists, "Artists", Icon::MicVocal, "2")),
    Some((View::Tracks, "Tracks", Icon::Music, "3")),
    Some((View::Videos, "Videos", Icon::Film, "8")),
    Some((View::Favorites, "Favorites", Icon::Heart, "9")),
    Some((View::History, "History", Icon::History, "0")),
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
        let m = Metrics::new(w, h, s, self.lib.view, self.lib.detail, self.lib.drawer);
        if !m.phone {
            self.lib.drawer = false;
        }
        let mut g = Geom {
            m,
            nav: Vec::new(),
            mode: [RectF::default(); 2],
            add_folder: RectF::default(),
            settings: RectF::default(),
            about: RectF::default(),
            folders: Vec::new(),
            folders_more: 0,
            back: None,
            menu_btn: None,
            search: RectF::default(),
            search_clear: RectF::default(),
            search_collapsed: false,
            sort: None,
            header_btns: Vec::new(),
            title_x: 0.0,
            table_cols: Vec::new(),
            bar_btns: Vec::new(),
            bar_art: RectF::default(),
            bar_text_y: [0.0; 3],
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
            if !g.m.phone || g.m.rail.w > 0.0 {
                self.geom_rail(&mut g, ctx);
            }
            self.geom_header(&mut g, model, ctx);
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
        let mut y = top + if compact { 92.0 * s } else { 52.0 * s };
        let bottom = r.bottom() - 16.0 * s;
        // From the bottom up: About RW (last), Settings, Add folder.
        g.about = RectF::new(px, bottom - 28.0 * s, iw, 28.0 * s);
        g.settings = RectF::new(px, g.about.y - 4.0 * s - 32.0 * s, iw, 32.0 * s);
        g.add_folder = RectF::new(px, g.settings.y - 6.0 * s - 40.0 * s, iw, 40.0 * s);
        // The folders live in the space between the entries and the add button (and the scan's progress line when it shows). The
        // entries shrink first (down to a compact row), so that up to three folders always have their rows; on a window too short
        // even for one, the folders are left out.
        let n_roots = if compact { 0 } else { ctx.lib.roots().len() };
        let (head, step, row_h, more_h) = (26.0 * s, 28.0 * s, 26.0 * s, 16.0 * s);
        let items = NAV.iter().flatten().count() as f32;
        let dividers = NAV.iter().filter(|n| n.is_none()).count() as f32;
        // A window too short for the entries above the three buttons drops Add folder first (the empty views and the menus still
        // add folders), then lets the entries shrink to a row of 16 px.
        let fits = |stack_top: f32, min_item: f32| {
            y + items * (min_item + 2.0 * s) + dividers * 14.0 * s + 12.0 * s <= stack_top
        };
        let mut min_item = 22.0 * s;
        if !fits(g.add_folder.y, min_item) {
            g.add_folder = RectF::default();
            if !fits(g.settings.y, min_item) {
                min_item = 16.0 * s;
            }
        }
        let stack_top = if g.add_folder.w > 0.0 { g.add_folder.y } else { g.settings.y };
        let area_bottom = stack_top - 12.0 * s - if ctx.scan.is_some() { 34.0 * s } else { 0.0 };
        let nav_min = items * (min_item + 2.0 * s) + dividers * 14.0 * s;
        let space = (area_bottom - y - nav_min - 12.0 * s - head).max(0.0);
        let mut reserved_rows = n_roots.min(3).min((space / step) as usize);
        // Folders beyond the rows need room for the "+N more" line too.
        while reserved_rows > 0 && reserved_rows < n_roots && reserved_rows as f32 * step + more_h > space {
            reserved_rows -= 1;
        }
        let reserve = if reserved_rows == 0 {
            0.0
        } else {
            head + reserved_rows as f32 * step + if n_roots > reserved_rows { more_h } else { 0.0 } + 12.0 * s
        };
        let room = (area_bottom - reserve) - y - dividers * 14.0 * s;
        let item_h = ((room / items) - 2.0 * s).clamp(min_item, if compact { 44.0 * s } else { 42.0 * s });
        for it in NAV.iter() {
            match it {
                Some((v, ..)) => {
                    g.nav.push((*v, RectF::new(px, y, iw, item_h)));
                    y += item_h + 2.0 * s;
                }
                None => y += 14.0 * s,
            }
        }
        if reserved_rows > 0 {
            // Whatever the entries left over shows more folders; the rest are counted in a "+N more" line.
            let first = y + 12.0 * s + head;
            let avail = area_bottom - first;
            let mut k = (avail / step).max(0.0) as usize;
            if k < n_roots {
                k = ((avail - more_h) / step).max(0.0) as usize;
            }
            let k = k.clamp(reserved_rows, n_roots);
            let mut fy = first;
            for i in 0..k {
                g.folders.push((i, RectF::new(px, fy, iw, row_h)));
                fy += step;
            }
            g.folders_more = n_roots - k;
        }
    }

    fn geom_header(&mut self, g: &mut Geom, model: &UiModel, ctx: &LibCtx<'_>) {
        let s = self.scale;
        let hd = g.m.header;
        let pad = g.m.pad;
        let (hy, bh) =
            if g.m.phone { (hd.y + (hd.h - 44.0 * s) * 0.5, 44.0 * s) } else { (hd.y + 26.0 * s, 40.0 * s) };
        let mut x = hd.x + pad;
        if g.m.phone {
            g.menu_btn = Some(RectF::new(hd.x + 8.0 * s, hd.y + (hd.h - 44.0 * s) * 0.5, 44.0 * s, 44.0 * s));
            x = hd.x + 8.0 * s + 48.0 * s;
        }
        if self.lib.detail.is_some() || !self.lib.history.is_empty() {
            g.back = Some(RectF::new(x - 4.0 * s, hy - 2.0 * s, bh, bh));
            x += 44.0 * s;
        }
        g.title_x = x;
        // Search box on the right, then the sort button and the view's actions to its left.
        let narrow = hd.w < 620.0 * s;
        let focused = self.lib.zone == super::Zone::Search;
        let sw = if narrow && !focused {
            bh
        } else if narrow {
            hd.w - 2.0 * pad
        } else {
            (hd.w * 0.30).clamp(190.0 * s, 330.0 * s)
        };
        g.search = RectF::new(hd.right() - pad - sw, hy, sw, bh);
        g.search_clear = RectF::new(g.search.right() - 34.0 * s, g.search.y + 6.0 * s, 28.0 * s, 28.0 * s);
        g.search_collapsed = narrow && !focused;
        let mut rx = g.search.x - 12.0 * s;
        let btns: Vec<(u8, &str, Icon, bool)> = match (self.lib.view, self.lib.detail) {
            (_, Some(_)) => Vec::new(),
            _ if narrow && focused => Vec::new(),
            (View::Playlists, _) => {
                alloc::vec![(1, "Import", Icon::Upload, false), (0, "New playlist", Icon::Plus, true)]
            }
            (View::Queue, _) if !model.playlist.is_empty() => {
                alloc::vec![(1, "Save as playlist", Icon::ListPlus, false), (0, "Clear", Icon::Trash2, false)]
            }
            (View::Tracks | View::Albums, _) if ctx.lib.track_count() > 0 => {
                alloc::vec![(0, "Shuffle all", Icon::Shuffle, true)]
            }
            (View::Favorites, _) if ctx.lib.favorite_count() > 0 => {
                alloc::vec![(1, "Shuffle", Icon::Shuffle, false), (0, "Play all", Icon::Play, true)]
            }
            (View::History, _) if ctx.lib.history_len() > 0 => {
                alloc::vec![(0, "Clear history", Icon::Trash2, false)]
            }
            (View::Videos, _) if ctx.lib.video_count() > 0 => {
                let sort = match self.lib.video_sort {
                    VideoSort::Title => "Sort: Title",
                    VideoSort::Added => "Sort: Added",
                    VideoSort::Length => "Sort: Length",
                    VideoSort::Plays => "Sort: Plays",
                    VideoSort::LastPlayed => "Sort: Last played",
                };
                alloc::vec![
                    (1, sort, Icon::ChevronDown, false),
                    (
                        0,
                        if self.lib.video_list { "Posters" } else { "List" },
                        if self.lib.video_list { Icon::LayoutGrid } else { Icon::List },
                        false
                    )
                ]
            }
            _ => Vec::new(),
        };
        let compact = hd.w < 900.0 * s;
        for (id, label, icon, primary) in btns {
            let tw = if compact { 0.0 } else { self.text_w(Face::SansMedium, 13.0, label, 0.0) + 8.0 * s };
            let bw = if compact { bh } else { tw + 50.0 * s };
            let r = RectF::new(rx - bw, hy, bw, bh);
            g.header_btns.push(PillBtn {
                id,
                rect: r,
                label: if compact { String::new() } else { label.into() },
                icon,
                primary,
            });
            rx = r.x - 10.0 * s;
        }
        if self.lib.view == View::Tracks && self.lib.detail.is_none() && !(narrow && focused) {
            let label = self.sort_label();
            let tw = self.text_w(Face::SansMedium, 13.0, &label, 0.0);
            let bw = tw + 48.0 * s;
            if rx - bw > x + 160.0 * s {
                g.sort = Some(RectF::new(rx - bw, hy, bw, bh));
            }
        }
        // The table head of the Tracks view.
        if let Some(th) = g.m.table_head {
            let cols = track_cols(th.w - 2.0 * (g.m.pad - 8.0 * s), s, true, true, true);
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
    }

    fn geom_bar(&mut self, g: &mut Geom, model: &UiModel) {
        let s = self.scale;
        let b = g.m.bar;
        let (w, y0) = (b.w, b.y);
        let compact = g.m.compact;
        let tiny = w < 760.0 * s;
        if g.m.phone {
            return self.geom_bar_phone(g, model);
        }
        g.bar_text_y = [y0 + 36.0 * s, y0 + 58.0 * s, y0 + 48.0 * s];
        let pad = 20.0 * s;
        // Left: cover and text.
        g.bar_art = RectF::new(pad, y0 + 16.0 * s, 64.0 * s, 64.0 * s);
        let small = 36.0 * s;
        let big = 46.0 * s;
        let gap = 10.0 * s;
        let (cx0, cw) = if tiny {
            let x0 = g.bar_art.right() + 12.0 * s;
            (x0, (w - x0 - pad - small - 12.0 * s).max(60.0 * s))
        } else {
            let cw = (w * 0.38).clamp(360.0 * s, 660.0 * s).min(w - 2.0 * pad);
            ((w - cw) * 0.5, cw)
        };
        g.bar_info = RectF::new(
            pad,
            y0 + 12.0 * s,
            if compact { 64.0 * s } else { (cx0 - pad - 16.0 * s).max(64.0 * s) },
            72.0 * s,
        );
        // Centre: transport.
        let cy = y0 + 34.0 * s;
        let row: Vec<(Btn, f32)> = if tiny {
            alloc::vec![(Btn::Prev, small), (Btn::Play, big), (Btn::Next, small)]
        } else {
            alloc::vec![
                (Btn::Shuffle, small),
                (Btn::Prev, small),
                (Btn::Play, big),
                (Btn::Next, small),
                (Btn::Repeat, small)
            ]
        };
        let total: f32 = row.iter().map(|(_, sz)| sz).sum::<f32>() + gap * (row.len() as f32 - 1.0);
        let mut x = cx0 + cw * 0.5 - total * 0.5;
        for (btn, size) in row {
            g.bar_btns.push((btn, RectF::new(x, cy - size * 0.5, size, size)));
            x += size + gap;
        }
        // Seek bar with the times at its ends.
        g.time_y = y0 + 74.0 * s;
        let tw = 46.0 * s;
        g.time_l = cx0;
        g.time_r = cx0 + cw - tw;
        g.seek_track =
            RectF::new(cx0 + tw + 8.0 * s, g.time_y - 2.0 * s, (cw - 2.0 * tw - 16.0 * s).max(10.0), 4.0 * s);
        g.seek_hit = RectF::new(g.seek_track.x, g.time_y - 12.0 * s, g.seek_track.w, 24.0 * s);
        // Right: volume, queue, visualizer, mode (a narrow bar keeps only the switch to the player).
        let mut rx = w - pad;
        let cy2 = y0 + 48.0 * s;
        let right: &[Btn] =
            if tiny { &[Btn::ModeSwitch] } else { &[Btn::ModeSwitch, Btn::VizView, Btn::QueueView] };
        for btn in right {
            g.bar_btns.push((*btn, RectF::new(rx - small, cy2 - small * 0.5, small, small)));
            rx -= small + 4.0 * s;
        }
        if !tiny {
            rx -= 10.0 * s;
            if !compact && w > 1000.0 * s {
                let vw = 92.0 * s;
                g.vol_hit = Some(RectF::new(rx - vw, cy2 - 14.0 * s, vw, 28.0 * s));
                g.vol_track = RectF::new(rx - vw + 6.0 * s, cy2 - 2.0 * s, vw - 12.0 * s, 4.0 * s);
                rx -= vw + 6.0 * s;
            }
            g.bar_btns.push((Btn::Mute, RectF::new(rx - small, cy2 - small * 0.5, small, small)));
            // The heart of what is playing sits at the right end of its title.
            if !compact && model.has_media() {
                g.bar_btns.push((
                    Btn::Favorite,
                    RectF::new(g.bar_info.right() - small, y0 + 14.0 * s, small, small),
                ));
            }
        }
    }

    /// The bar of a phone-width window, in three rows: the seek bar with its times, what is playing, and the transport (queue, previous,
    /// play, next, player), every target at least 44 px.
    fn geom_bar_phone(&mut self, g: &mut Geom, model: &UiModel) {
        let s = self.scale;
        let b = g.m.bar;
        let (w, y0) = (b.w, b.y);
        let pad = 16.0 * s;
        // Row 1: the seek bar, times at its ends.
        g.time_y = y0 + 18.0 * s;
        let tw = 40.0 * s;
        g.time_l = pad;
        g.time_r = w - pad - tw;
        g.seek_track = RectF::new(
            pad + tw + 8.0 * s,
            g.time_y - 2.0 * s,
            (w - 2.0 * pad - 2.0 * tw - 16.0 * s).max(10.0),
            4.0 * s,
        );
        g.seek_hit = RectF::new(g.seek_track.x, g.time_y - 16.0 * s, g.seek_track.w, 32.0 * s);
        // Row 2: the cover and the text (the heart at the right end).
        g.bar_art = RectF::new(pad, y0 + 40.0 * s, 48.0 * s, 48.0 * s);
        g.bar_info = RectF::new(pad, y0 + 38.0 * s, w - 2.0 * pad, 52.0 * s);
        g.bar_text_y = [y0 + 55.0 * s, y0 + 75.0 * s, y0 + 64.0 * s];
        if model.has_media() {
            g.bar_btns.push((
                Btn::Favorite,
                RectF::new(g.bar_info.right() - 44.0 * s, y0 + 42.0 * s, 44.0 * s, 44.0 * s),
            ));
        }
        // Row 3: the transport.
        let cy = y0 + 122.0 * s;
        let row: [(Btn, f32); 5] = [
            (Btn::QueueView, 44.0),
            (Btn::Prev, 44.0),
            (Btn::Play, 56.0),
            (Btn::Next, 44.0),
            (Btn::ModeSwitch, 44.0),
        ];
        let total: f32 = row.iter().map(|(_, z)| z * s).sum::<f32>() + 10.0 * s * 4.0;
        let mut x = (w - total) * 0.5;
        for (btn, z) in row {
            let z = z * s;
            g.bar_btns.push((btn, RectF::new(x, cy - z * 0.5, z, z)));
            x += z + 10.0 * s;
        }
    }

    fn geom_viz(&mut self, g: &mut Geom) {
        let s = self.scale;
        // The switcher: previous, next, palette, info, animation, cycle, centred above the bar.
        let w = g.m.w;
        let bw = 44.0 * s;
        let name_gap = 110.0 * s;
        let total = bw * 6.0 + 8.0 * s * 5.0 + name_gap;
        let mut x = (w - total) * 0.5;
        let y = g.m.bar.y - 70.0 * s;
        for (i, wid) in [(0u8, bw), (1, bw), (2, bw), (3, bw), (4, bw), (5, bw)] {
            g.viz_btns.push((i, RectF::new(x, y, wid, 40.0 * s)));
            x += wid + 8.0 * s + if i == 0 { name_gap } else { 0.0 };
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
                    (2, "Add to queue", Icon::ListEnd, false),
                    (3, "Favorite", Icon::Heart, false)
                ]
            }
            Detail::Playlist(_) => alloc::vec![
                (0, "Play", Icon::Play, true),
                (1, "Shuffle", Icon::Shuffle, false),
                (2, "Add to queue", Icon::ListEnd, false),
                (4, "M3U8", Icon::Download, false),
                (5, "PLS", Icon::Download, false),
                (6, "Rename", Icon::Pencil, false),
                (7, "Delete", Icon::Trash2, false),
            ],
        };
        let mut x = x0;
        let mut out = Vec::new();
        for (id, label, icon, primary) in defs {
            let tw = self.text_w(Face::SansMedium, 13.0, label, 0.0) + 8.0 * s;
            let bw = tw + 46.0 * s;
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
            x += bw + 8.0 * s;
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
            T::Plays => "Plays",
            T::LastPlayed => "Last played",
        };
        alloc::format!("Sort: {name} {}", if self.lib.track_asc { "(A-Z)" } else { "(Z-A)" })
    }
}
