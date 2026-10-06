//! Drawing the library screen with the Unicorn Tears tokens: the rail, header, cards and rows, the hero blocks, the now-playing
//! screen, the visualizer's chrome, the bar and the prompt. Glow is kept for focus and hover; static surfaces are flat.
use super::geom::{Geom, NAV, PillBtn, track_cols};
use super::rows::{EntKind, RowKind};
use super::{Detail, LibCtx, LibHit, Mode, View, Zone, visible_rows};
use crate::font::Face;
use crate::gfx::{FrameBuffer, Paint, RectF, fade};
use crate::icon::Icon;
use crate::model::{MediaState, UiModel, format_time};
use crate::tk as t;
use crate::ui::{Btn, Layout, TOOLTIP_DELAY_US, Ui};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use theme::Rgba;

/// Rectangles of the now-playing screen.
pub(crate) struct NpRects {
    pub cover: RectF,
    pub text_x: f32,
    pub text_w: f32,
    pub up_next: Vec<RectF>,
    pub browse: Option<RectF>,
}

/// The tints of a cover that has no picture (they follow the theme).
fn placeholder() -> [(Rgba, Rgba); 3] {
    [(t::violet_600(), t::magenta_700()), (t::cyan_600(), t::violet_600()), (t::magenta_700(), t::ink_600())]
}

/// `1920x1080 \u{b7} h264` (what is known of the file).
fn video_sub(v: &rvp_library::Video) -> String {
    let mut parts: Vec<String> = Vec::new();
    let size = v.size_text();
    if !size.is_empty() {
        parts.push(size);
    }
    if !v.vcodec.is_empty() {
        parts.push(v.vcodec.clone());
    }
    parts.join(" \u{b7} ")
}

fn dur_text(us: i64) -> String {
    if us <= 0 { String::from("--:--") } else { format_time(us) }
}

/// "3 h 12 min" style length for album and library totals.
pub(crate) fn long_duration(us: i64) -> String {
    let mins = (us.max(0) / 60_000_000) as u64;
    if mins >= 60 {
        alloc::format!("{} h {} min", mins / 60, mins % 60)
    } else {
        alloc::format!("{} min", mins.max(1))
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    alloc::format!("{n} {}", if n == 1 { one } else { many })
}

impl Ui {
    pub(crate) fn lib_layout(&self) -> Layout {
        // Toasts sit just above the bar, out of the way of the header's search box.
        Layout {
            s: self.scale,
            w: self.w as f32,
            h: self.h as f32,
            toast_y: self.h as f32 - 96.0 * self.scale - 52.0 * self.scale,
            ..Layout::default()
        }
    }

    // ---- shared pieces ------------------------------------------------------------------------------------------------

    /// The thumbnail of `art` as RGBA, kept for the next frames.
    fn thumb_rgba(&mut self, ctx: &LibCtx<'_>, art: u64) -> Option<Rc<(u32, u32, Vec<u8>)>> {
        if art == 0 {
            return None;
        }
        if self.lib.thumbs_rev != ctx.lib.revision() {
            if self.lib.thumbs.len() > 160 {
                self.lib.thumbs.clear();
            }
            self.lib.thumbs_rev = ctx.lib.revision();
        }
        if let Some(c) = self.lib.thumbs.get(&art) {
            return Some(c.clone());
        }
        let th = ctx.lib.thumb(art)?;
        let img = th.to_image();
        let rc = Rc::new((img.w, img.h, img.rgba));
        self.lib.thumbs.insert(art, rc.clone());
        Some(rc)
    }

    /// A cover (or a stand-in) in `r`.
    fn draw_cover(
        &mut self,
        fb: &mut FrameBuffer,
        ctx: &LibCtx<'_>,
        r: RectF,
        radius: f32,
        art: u64,
        seed: u32,
        a: f32,
    ) {
        let s = self.scale;
        match self.thumb_rgba(ctx, art) {
            Some(img) => {
                // Keep the picture's shape: fill the square, cropping the long side.
                let (iw, ih) = (img.0 as f32, img.1 as f32);
                if (iw - ih).abs() < 1.5 {
                    fb.blit_scaled_rounded(r, radius, &img.2, img.0, img.1);
                } else {
                    // A non-square picture is drawn letterboxed into the square on a dark tile.
                    fb.fill_rrect(r, radius, Paint::Solid(t::ink_800()), a);
                    let k = (r.w / iw).min(r.h / ih);
                    let (w, h) = (iw * k, ih * k);
                    let dst = RectF::new(r.cx() - w * 0.5, r.cy() - h * 0.5, w, h);
                    fb.blit_scaled_rounded(dst, radius.min(w * 0.5).min(h * 0.5), &img.2, img.0, img.1);
                }
                fb.stroke_rrect(r, radius, 1.0 * s, fade(t::white(), 0.10), a);
            }
            None => {
                let ph = placeholder();
                let (c0, c1) = ph[seed as usize % ph.len()];
                fb.fill_rrect(r, radius, Paint::Vertical(fade(c0, 0.55), fade(c1, 0.55)), a);
                fb.stroke_rrect(r, radius, 1.0 * s, fade(t::white(), 0.10), a);
                let size = (r.w / s * 0.38).clamp(12.0, 64.0);
                self.icon(fb, Icon::Music, r.cx(), r.cy(), size, t::pink_white(), 0.55 * a, false);
            }
        }
    }

    /// A video's poster frame in a 16:9 box (a placeholder with a film icon until it is made).
    fn draw_poster(
        &mut self,
        fb: &mut FrameBuffer,
        ctx: &LibCtx<'_>,
        r: RectF,
        radius: f32,
        art: u64,
        seed: u32,
    ) {
        let s = self.scale;
        match self.thumb_rgba(ctx, art) {
            Some(img) => {
                let (iw, ih) = (img.0 as f32, img.1 as f32);
                if (iw / ih - r.w / r.h).abs() < 0.05 {
                    fb.blit_scaled_rounded(r, radius, &img.2, img.0, img.1);
                } else {
                    // Another shape: letterboxed on a dark tile.
                    fb.fill_rrect(r, radius, Paint::Solid(t::ink_900()), 1.0);
                    let k = (r.w / iw).min(r.h / ih);
                    let (w, h) = (iw * k, ih * k);
                    let dst = RectF::new(r.cx() - w * 0.5, r.cy() - h * 0.5, w, h);
                    fb.blit_scaled_rounded(dst, radius.min(w * 0.5).min(h * 0.5), &img.2, img.0, img.1);
                }
                fb.stroke_rrect(r, radius, 1.0 * s, fade(t::white(), 0.10), 1.0);
            }
            None => {
                let ph = placeholder();
                let (c0, c1) = ph[seed as usize % ph.len()];
                fb.fill_rrect(r, radius, Paint::Vertical(fade(c0, 0.55), fade(c1, 0.55)), 1.0);
                fb.stroke_rrect(r, radius, 1.0 * s, fade(t::white(), 0.10), 1.0);
                let size = (r.h / s * 0.34).clamp(12.0, 56.0);
                self.icon(fb, Icon::Film, r.cx(), r.cy(), size, t::pink_white(), 0.55, false);
            }
        }
    }

    /// A pill button.
    fn draw_pill(&mut self, fb: &mut FrameBuffer, b: &PillBtn, hot: bool, pressed: bool, focused: bool) {
        let s = self.scale;
        let r = if pressed { b.rect.scaled(0.97) } else { b.rect };
        let rad = r.h * 0.5;
        if b.primary {
            if hot || focused {
                fb.glow_rrect(r, rad, 20.0 * s, t::magenta_500(), 0.5);
            }
            fb.fill_rrect(r, rad, Paint::Gradient(t::gradient_tears()), 1.0);
            if hot {
                fb.fill_rrect(r, rad, Paint::Solid(t::white()), 0.12);
            }
        } else {
            if hot {
                fb.fill_rrect(r, rad, Paint::Solid(fade(t::white(), 0.10)), 1.0);
            }
            fb.stroke_rrect(
                r,
                rad,
                1.0 * s,
                if hot { fade(t::cyan_500(), 0.7) } else { fade(t::white(), 0.22) },
                1.0,
            );
        }
        let col = if b.primary || hot { t::white() } else { t::text_body() };
        if b.label.is_empty() {
            self.icon(fb, b.icon, r.cx(), r.cy(), 18.0, col, 1.0, false);
        } else {
            self.icon(fb, b.icon, r.x + 24.0 * s, r.cy(), 17.0, col, 1.0, b.icon == Icon::Play);
            self.text(fb, Face::SansMedium, 13.0, r.x + 40.0 * s, r.cy(), &b.label, col, 1.0, 0.2);
        }
        if focused {
            self.focus_ring(fb, r, rad, 1.0);
        }
    }

    /// The keyboard focus ring for something big (a row, a rail entry): only the cyan outline, no glow tinting the inside.
    fn focus_outline(&mut self, fb: &mut FrameBuffer, r: RectF, radius: f32, a: f32) {
        let s = self.scale;
        fb.stroke_rrect(r.inflate(1.0 * s), radius + 1.0 * s, 2.0 * s, t::focus_ring(), a);
    }

    fn lib_pressed(&self, h: LibHit) -> bool {
        self.pressed_lib == Some(h) && self.lib.hover == h
    }

    // ---- the layers ---------------------------------------------------------------------------------------------------

    /// The layer under the chrome: the night sky, or the visualizer's picture.
    pub fn draw_base_lib(&mut self, fb: &mut FrameBuffer, model: &UiModel, ctx: &LibCtx<'_>) {
        let _ = model;
        let wants_viz =
            matches!(self.lib.view, View::Visualizer | View::NowPlaying) && self.lib.detail.is_none();
        match ctx.viz {
            Some(v) if wants_viz && self.lib.viz_on => {
                let (px, w, h) = v.picture();
                if w > 0 && px.len() >= w * h * 4 {
                    let full = RectF::new(0.0, 0.0, fb.width as f32, fb.height as f32);
                    if self.lib.view == View::NowPlaying {
                        // Behind the cover and the text the picture is dimmed: done on the small picture, not the big screen.
                        self.lib.dim.clear();
                        for p in px.chunks_exact(4) {
                            let k = |c: u8| (c as u32 * 42 / 100) as u8;
                            self.lib.dim.extend_from_slice(&[k(p[0]), k(p[1]), k(p[2]), 255]);
                        }
                        fb.blit_scaled(full, &self.lib.dim, w as u32, h as u32);
                    } else {
                        fb.blit_scaled(full, px, w as u32, h as u32);
                    }
                    return;
                }
                fb.fill_radial(&t::gradient_night());
            }
            _ => fb.fill_radial(&t::gradient_night()),
        }
    }

    /// The chrome and the content.
    pub fn draw_overlay_lib(&mut self, fb: &mut FrameBuffer, model: &UiModel, ctx: &LibCtx<'_>) {
        let g = self.lib_geom(model, ctx);
        self.ensure_rows(model, ctx, &g);
        self.lib.last_geom = Some(g.clone());
        self.lib.visible.clear();
        self.lib.hero.clear();
        let l = self.lib_layout();
        let view = self.lib.view;
        let mut animated = false;
        if view == View::Visualizer {
            self.draw_viz_chrome(fb, &g, model, ctx);
        } else {
            // Content first; the header and the rail are drawn over what scrolls under them.
            if view == View::NowPlaying && self.lib.detail.is_none() {
                animated |= self.draw_now_playing(fb, &g, model, ctx);
            } else {
                animated |= self.draw_body(fb, &g, model, ctx);
                self.draw_scrollbar(fb, &g);
            }
            self.draw_header(fb, &g, model, ctx);
            if g.m.table_head.is_some() {
                self.draw_table_head(fb, &g);
            }
            self.draw_rail(fb, &g, model, ctx);
        }
        if view != View::Visualizer || self.controls_alpha > 0.005 {
            let a = if view == View::Visualizer { self.controls_alpha } else { 1.0 };
            self.draw_lib_bar(fb, &g, model, ctx, a);
        }
        self.draw_toast(fb, &l);
        self.draw_lib_tooltip(fb, &g, model);
        if self.drag_over {
            self.draw_drop_outline(fb, &l);
        }
        let panels = core::mem::take(&mut self.menu);
        for p in &panels {
            self.draw_panel(fb, &l, p);
        }
        self.menu = panels;
        self.draw_audio_panel(fb, model);
        if self.lib.prompt.is_some() {
            self.draw_prompt(fb, &g);
        }
        self.draw_app_dialog(fb, model);
        self.lib.animated = animated;
    }

    // ---- the rail -------------------------------------------------------------------------------------------------------

    fn draw_rail(&mut self, fb: &mut FrameBuffer, g: &Geom, model: &UiModel, ctx: &LibCtx<'_>) {
        let s = self.scale;
        let r = g.m.rail;
        let compact = g.m.compact;
        fb.fill_rect_paint(r, Paint::Solid(t::ink_850()), 0.97);
        fb.fill_rect_paint(
            RectF::new(r.right() - 1.0 * s, r.y, 1.0 * s, r.h),
            Paint::Solid(t::white()),
            0.07,
        );
        let px = if compact { 10.0 * s } else { 16.0 * s };
        if !compact {
            // The brand: the logo and the kicker.
            let mark = RectF::new(px - 2.0 * s, 20.0 * s, 38.0 * s, 38.0 * s);
            crate::logo::draw(fb, mark, 1.0);
            self.text(
                fb,
                Face::SansBold,
                10.5,
                mark.right() + 8.0 * s,
                mark.cy() - 7.0 * s,
                "RUSTY WAVE",
                t::violet_400(),
                1.0,
                1.6,
            );
            self.text(
                fb,
                Face::SansMedium,
                13.0,
                mark.right() + 8.0 * s,
                mark.cy() + 8.0 * s,
                "Media",
                t::text_muted(),
                1.0,
                0.0,
            );
        }
        // The Library / Player switch.
        let seg = if compact {
            RectF::new(g.mode[0].x, g.mode[0].y, g.mode[0].w, g.mode[1].bottom() - g.mode[0].y)
        } else {
            RectF::new(
                g.mode[0].x - 3.0 * s,
                g.mode[0].y - 3.0 * s,
                (g.mode[1].right() - g.mode[0].x) + 6.0 * s,
                g.mode[0].h + 6.0 * s,
            )
        };
        fb.fill_rrect(seg, if compact { 14.0 * s } else { seg.h * 0.5 }, Paint::Solid(t::ink_900()), 1.0);
        fb.stroke_rrect(
            seg,
            if compact { 14.0 * s } else { seg.h * 0.5 },
            1.0 * s,
            fade(t::white(), 0.10),
            1.0,
        );
        for (i, rect) in g.mode.iter().enumerate() {
            let mode = if i == 0 { Mode::Library } else { Mode::Player };
            let on = self.lib.mode == mode;
            let hot = self.lib.hover == LibHit::ModeSwitch(mode);
            let rad = if compact { 12.0 * s } else { rect.h * 0.5 };
            if on {
                fb.fill_rrect(*rect, rad, Paint::Gradient(t::gradient_tears()), 1.0);
            } else if hot {
                fb.fill_rrect(*rect, rad, Paint::Solid(fade(t::white(), 0.08)), 1.0);
            }
            let col = if on {
                t::white()
            } else if hot {
                t::text_body()
            } else {
                t::text_muted()
            };
            let icon = if i == 0 { Icon::Music } else { Icon::Film };
            if compact {
                self.icon(fb, icon, rect.cx(), rect.cy(), 18.0, col, 1.0, false);
            } else {
                let label = if i == 0 { "Library" } else { "Player" };
                let tw = self.text_w(Face::SansMedium, 13.0, label, 0.0);
                let total = 18.0 * s + 8.0 * s + tw;
                let x0 = rect.cx() - total * 0.5;
                self.icon(fb, icon, x0 + 9.0 * s, rect.cy(), 16.0, col, 1.0, false);
                self.text(fb, Face::SansMedium, 13.0, x0 + 26.0 * s, rect.cy(), label, col, 1.0, 0.0);
            }
        }
        // Navigation.
        for (ni, it) in NAV.iter().flatten().enumerate() {
            let (view, label, icon, key) = *it;
            let rect = g.nav[ni].1;
            let on = self.lib.view == view;
            let hot = self.lib.hover == LibHit::Rail(view);
            let kb = self.lib.zone == Zone::Rail && self.lib.rail_focus == ni && self.keyboard_mode;
            if on {
                fb.fill_rrect(rect, 12.0 * s, Paint::Solid(t::ink_700()), 1.0);
                fb.fill_rrect(
                    RectF::new(rect.x + 1.0 * s, rect.cy() - 10.0 * s, 3.0 * s, 20.0 * s),
                    1.5 * s,
                    Paint::Gradient(t::gradient_tears_v()),
                    1.0,
                );
            } else if hot {
                fb.fill_rrect(rect, 12.0 * s, Paint::Solid(t::ink_800()), 1.0);
            }
            let col = if on {
                t::white()
            } else if hot {
                t::text_body()
            } else {
                t::text_muted()
            };
            let icon_col = if on { t::cyan_500() } else { col };
            if compact {
                self.icon(fb, icon, rect.cx(), rect.cy(), 20.0, icon_col, 1.0, false);
            } else {
                self.icon(fb, icon, rect.x + 26.0 * s, rect.cy(), 19.0, icon_col, 1.0, false);
                self.text(
                    fb,
                    if on { Face::SansMedium } else { Face::Sans },
                    14.5,
                    rect.x + 50.0 * s,
                    rect.cy(),
                    label,
                    col,
                    1.0,
                    0.0,
                );
                let kw = self.text_w(Face::Mono, 11.0, key, 0.0);
                self.text(
                    fb,
                    Face::Mono,
                    11.0,
                    rect.right() - 14.0 * s - kw,
                    rect.cy(),
                    key,
                    t::text_disabled(),
                    1.0,
                    0.0,
                );
            }
            if kb {
                self.focus_outline(fb, rect, 12.0 * s, 1.0);
            }
        }
        // Folders and the scan.
        if !compact {
            if let Some((_, first)) = g.folders.first().or(Some(&(0, g.add_folder))) {
                let hy = first.y - 22.0 * s;
                if hy > g.nav.last().map_or(0.0, |(_, r)| r.bottom()) {
                    self.text(fb, Face::SansBold, 10.5, px, hy, "FOLDERS", t::violet_400(), 1.0, 1.6);
                }
            }
            for (i, rect) in &g.folders {
                let hot = self.lib.hover == LibHit::Folder(*i);
                if hot {
                    fb.fill_rrect(*rect, 8.0 * s, Paint::Solid(t::ink_800()), 1.0);
                }
                let root = &ctx.lib.roots()[*i];
                self.icon(
                    fb,
                    Icon::Folder,
                    rect.x + 14.0 * s,
                    rect.cy(),
                    15.0,
                    if root.connected { t::cyan_500() } else { t::text_disabled() },
                    1.0,
                    false,
                );
                let name = self.fonts.fit(Face::Sans, 13.0 * s, &root.name, rect.w - 50.0 * s);
                self.text(
                    fb,
                    Face::Sans,
                    13.0,
                    rect.x + 34.0 * s,
                    rect.cy(),
                    &name,
                    if root.connected { t::text_muted() } else { t::text_disabled() },
                    1.0,
                    0.0,
                );
            }
        }
        if g.folders_more > 0 {
            if let Some((_, last)) = g.folders.last() {
                let label = alloc::format!("+{} more", g.folders_more);
                self.text(
                    fb,
                    Face::Sans,
                    12.0,
                    px + 14.0 * s,
                    last.bottom() + 10.0 * s,
                    &label,
                    t::text_disabled(),
                    1.0,
                    0.0,
                );
            }
        }
        if let Some(sc) = ctx.scan {
            let y = g.add_folder.y - 20.0 * s;
            if !compact {
                let label = alloc::format!(
                    "{} {} / {}",
                    if sc.analysing { "Measuring" } else { "Scanning" },
                    sc.done,
                    sc.total.max(sc.done)
                );
                self.text(fb, Face::Mono, 11.0, px, y - 12.0 * s, &label, t::cyan_400(), 1.0, 0.0);
                let tr = RectF::new(px, y, r.w - 2.0 * px, 4.0 * s);
                fb.fill_rrect(tr, 2.0 * s, Paint::Solid(fade(t::white(), 0.14)), 1.0);
                let f = if sc.total == 0 { 0.0 } else { (sc.done as f32 / sc.total as f32).clamp(0.0, 1.0) };
                if f > 0.0 {
                    fb.fill_rrect(
                        RectF::new(tr.x, tr.y, (tr.w * f).max(4.0 * s), tr.h),
                        2.0 * s,
                        Paint::Horizontal(t::magenta_500(), t::violet_400()),
                        1.0,
                    );
                }
            }
        }
        let hot = self.lib.hover == LibHit::AddFolder;
        let b = PillBtn {
            id: 0,
            rect: g.add_folder,
            label: if compact { String::new() } else { "Add folder".into() },
            icon: Icon::FolderPlus,
            primary: false,
        };
        let pressed = self.lib_pressed(LibHit::AddFolder);
        self.draw_pill(fb, &b, hot, pressed, false);
        let hot = self.lib.hover == LibHit::Settings;
        let b = PillBtn {
            id: 0,
            rect: g.settings,
            label: if compact { String::new() } else { "Settings".into() },
            icon: Icon::Settings,
            primary: false,
        };
        let pressed = self.lib_pressed(LibHit::Settings);
        self.draw_pill(fb, &b, hot, pressed, false);
        let _ = model;
    }

    // ---- the header ------------------------------------------------------------------------------------------------------

    fn title_for(&self, ctx: &LibCtx<'_>, model: &UiModel) -> (String, String) {
        let lib = ctx.lib;
        if let Some(d) = self.lib.detail {
            return match d {
                Detail::Album(id) => lib.album(id).map_or(("Album".into(), String::new()), |a| {
                    (
                        a.title.clone(),
                        alloc::format!("{} \u{b7} {}", a.artist, plural(a.tracks.len(), "track", "tracks")),
                    )
                }),
                Detail::Artist(id) => lib.artist(id).map_or(("Artist".into(), String::new()), |a| {
                    (
                        a.name.clone(),
                        alloc::format!(
                            "{} \u{b7} {}",
                            plural(a.albums.len(), "album", "albums"),
                            plural(a.track_count, "track", "tracks")
                        ),
                    )
                }),
                Detail::Playlist(id) => lib.playlist(id).map_or(("Playlist".into(), String::new()), |p| {
                    (p.name.clone(), plural(p.entries.len(), "track", "tracks"))
                }),
            };
        }
        match self.lib.view {
            View::Albums => ("Albums".into(), plural(lib.albums().len(), "album", "albums")),
            View::Artists => ("Artists".into(), plural(lib.artists().len(), "artist", "artists")),
            View::Tracks => (
                "Tracks".into(),
                alloc::format!(
                    "{} \u{b7} {}",
                    plural(lib.track_count(), "track", "tracks"),
                    long_duration(lib.total_duration_us())
                ),
            ),
            View::Videos => (
                "Videos".into(),
                alloc::format!(
                    "{} \u{b7} {}",
                    plural(lib.video_count(), "video", "videos"),
                    long_duration(lib.total_video_duration_us())
                ),
            ),
            View::Playlists => ("Playlists".into(), plural(lib.playlists().len(), "playlist", "playlists")),
            View::Queue => {
                let total: i64 = model.playlist.iter().map(|e| e.duration_us).sum();
                (
                    "Queue".into(),
                    if model.playlist.is_empty() {
                        "Nothing queued".into()
                    } else {
                        alloc::format!(
                            "{} \u{b7} {}",
                            plural(model.playlist.len(), "track", "tracks"),
                            long_duration(total)
                        )
                    },
                )
            }
            View::Search => {
                let n = self.lib.rows.as_ref().map_or(0, |r| r.ents.len());
                (
                    "Search".into(),
                    if self.lib.query.trim().is_empty() {
                        "Type to search".into()
                    } else {
                        alloc::format!(
                            "{} for \u{201c}{}\u{201d}",
                            plural(n, "result", "results"),
                            self.lib.query.trim()
                        )
                    },
                )
            }
            View::NowPlaying => ("Now playing".into(), String::new()),
            View::Visualizer => ("Visualizer".into(), String::new()),
        }
    }

    fn draw_header(&mut self, fb: &mut FrameBuffer, g: &Geom, model: &UiModel, ctx: &LibCtx<'_>) {
        let s = self.scale;
        let hd = g.m.header;
        // A glassy strip over whatever scrolls under it.
        fb.fill_rect_paint(
            RectF::new(hd.x, hd.y, hd.w, hd.h),
            Paint::Vertical(fade(t::ink_900(), 0.97), fade(t::ink_900(), 0.93)),
            1.0,
        );
        fb.fill_rect_paint(
            RectF::new(hd.x, hd.bottom() - 1.0 * s, hd.w, 1.0 * s),
            Paint::Solid(t::white()),
            0.06,
        );
        if let Some(b) = g.back {
            let hot = self.lib.hover == LibHit::Back;
            if hot {
                fb.fill_rrect(b, 20.0 * s, Paint::Solid(fade(t::white(), 0.10)), 1.0);
            }
            self.icon(
                fb,
                Icon::ChevronLeft,
                b.cx(),
                b.cy(),
                22.0,
                if hot { t::white() } else { t::text_body() },
                1.0,
                false,
            );
        }
        let (title, sub) = self.title_for(ctx, model);
        let max_w =
            (g.search.x - g.title_x - 24.0 * s - g.header_btns.last().map_or(0.0, |b| g.search.x - b.rect.x))
                .max(120.0 * s);
        let max_w = max_w.min(g.search.x - g.title_x - 16.0 * s);
        let room = max_w > 60.0 * s;
        let title = self.fonts.fit(Face::SansBold, 28.0 * s, &title, max_w);
        if room {
            self.text(
                fb,
                Face::SansBold,
                28.0,
                g.title_x,
                hd.y + 40.0 * s,
                &title,
                t::text_strong(),
                1.0,
                -0.3,
            );
        }
        if !sub.is_empty() && room {
            let sub = self.fonts.fit(Face::Sans, 13.0 * s, &sub, max_w);
            self.text(fb, Face::Sans, 13.0, g.title_x, hd.y + 68.0 * s, &sub, t::text_dim(), 1.0, 0.0);
        }
        // Search.
        let focused = self.lib.zone == Zone::Search;
        let hot = self.lib.hover == LibHit::Search;
        let sr = g.search;
        if focused {
            fb.glow_rrect(sr, 20.0 * s, 14.0 * s, t::cyan_500(), 0.35);
        }
        fb.fill_rrect(sr, 20.0 * s, Paint::Solid(t::ink_850()), 1.0);
        fb.stroke_rrect(
            sr,
            20.0 * s,
            if focused { 2.0 * s } else { 1.0 * s },
            if focused {
                t::focus_ring()
            } else if hot {
                fade(t::white(), 0.28)
            } else {
                fade(t::white(), 0.14)
            },
            1.0,
        );
        let icon_x = if g.search_collapsed { sr.cx() } else { sr.x + 22.0 * s };
        self.icon(
            fb,
            Icon::Search,
            icon_x,
            sr.cy(),
            17.0,
            if focused || (g.search_collapsed && !self.lib.query.is_empty()) {
                t::cyan_500()
            } else {
                t::text_dim()
            },
            1.0,
            false,
        );
        let tx = sr.x + 42.0 * s;
        let room = sr.w - 42.0 * s - 40.0 * s;
        if g.search_collapsed {
            // Just the icon.
        } else if self.lib.query.is_empty() {
            self.text(fb, Face::Sans, 14.0, tx, sr.cy(), "Search library", t::text_dim(), 1.0, 0.0);
        } else {
            // Show the tail of what was typed when it is longer than the box.
            let mut shown: String = self.lib.query.clone();
            while self.text_w(Face::Sans, 14.0, &shown, 0.0) > room && shown.chars().count() > 1 {
                shown.remove(0);
            }
            self.text(fb, Face::Sans, 14.0, tx, sr.cy(), &shown, t::text_strong(), 1.0, 0.0);
            let x = tx + self.text_w(Face::Sans, 14.0, &shown, 0.0);
            if focused && (self.now / 530_000) % 2 == 0 {
                fb.fill_rect_paint(
                    RectF::new(x + 1.0 * s, sr.cy() - 9.0 * s, 1.5 * s, 18.0 * s),
                    Paint::Solid(t::cyan_500()),
                    1.0,
                );
            }
            let clear_hot = self.lib.hover == LibHit::SearchClear;
            self.icon(
                fb,
                Icon::X,
                g.search_clear.cx(),
                g.search_clear.cy(),
                15.0,
                if clear_hot { t::white() } else { t::text_dim() },
                1.0,
                false,
            );
        }
        if self.lib.query.is_empty() && focused && !g.search_collapsed && (self.now / 530_000) % 2 == 0 {
            fb.fill_rect_paint(
                RectF::new(tx - 1.0 * s, sr.cy() - 9.0 * s, 1.5 * s, 18.0 * s),
                Paint::Solid(t::cyan_500()),
                1.0,
            );
        }
        // Sort and actions.
        if let Some(sr) = g.sort {
            let hot = self.lib.hover == LibHit::Sort;
            if hot {
                fb.fill_rrect(sr, sr.h * 0.5, Paint::Solid(fade(t::white(), 0.10)), 1.0);
            }
            fb.stroke_rrect(sr, sr.h * 0.5, 1.0 * s, fade(t::white(), 0.22), 1.0);
            let label = self.sort_label();
            self.text(fb, Face::SansMedium, 13.0, sr.x + 18.0 * s, sr.cy(), &label, t::text_body(), 1.0, 0.0);
            self.icon(
                fb,
                Icon::ChevronDown,
                sr.right() - 20.0 * s,
                sr.cy(),
                15.0,
                t::text_muted(),
                1.0,
                false,
            );
        }
        for b in g.header_btns.clone() {
            let h = LibHit::Button(b.id);
            self.draw_pill(fb, &b, self.lib.hover == h, self.lib_pressed(h), false);
        }
    }

    fn draw_table_head(&mut self, fb: &mut FrameBuffer, g: &Geom) {
        let s = self.scale;
        let Some(th) = g.m.table_head else { return };
        fb.fill_rect_paint(th, Paint::Solid(t::ink_900()), 0.97);
        fb.fill_rect_paint(
            RectF::new(th.x, th.bottom() - 1.0 * s, th.w, 1.0 * s),
            Paint::Solid(t::white()),
            0.06,
        );
        let cols = track_cols(th.w - 2.0 * (g.m.pad - 8.0 * s), s, true, true, true);
        let x0 = th.x + g.m.pad - 8.0 * s;
        use rvp_library::TrackSort as T;
        let active = match self.lib.track_sort {
            T::Title => Some(0u8),
            T::Artist => Some(1),
            T::Album => Some(2),
            T::Duration => Some(3),
            _ => None,
        };
        self.text(
            fb,
            Face::SansBold,
            10.5,
            x0 + cols.num.0 + 10.0 * s,
            th.cy(),
            "#",
            t::text_disabled(),
            1.0,
            1.4,
        );
        let head = |ui: &mut Ui, fb: &mut FrameBuffer, id: u8, x: f32, w: f32, label: &str, right: bool| {
            let on = active == Some(id);
            let hot = ui.lib.hover == LibHit::SortCol(id);
            let col = if on {
                t::cyan_400()
            } else if hot {
                t::text_body()
            } else {
                t::violet_400()
            };
            let tw = ui.text_w(Face::SansBold, 10.5, label, 1.4);
            let tx = if right { x + w - tw } else { x };
            ui.text(fb, Face::SansBold, 10.5, tx, th.cy(), label, col, 1.0, 1.4);
            if on {
                let icon = if ui.lib.track_asc { Icon::ArrowUp } else { Icon::ArrowDown };
                ui.icon(
                    fb,
                    icon,
                    if right { tx - 12.0 * s } else { tx + tw + 12.0 * s },
                    th.cy(),
                    12.0,
                    t::cyan_400(),
                    1.0,
                    false,
                );
            }
        };
        head(self, fb, 0, x0 + cols.title.0, cols.title.1, "TITLE", false);
        if let Some(a) = cols.artist {
            head(self, fb, 1, x0 + a.0, a.1, "ARTIST", false);
        }
        if let Some(a) = cols.album {
            head(self, fb, 2, x0 + a.0, a.1, "ALBUM", false);
        }
        head(self, fb, 3, x0 + cols.time.0, cols.time.1, "TIME", true);
    }

    fn draw_scrollbar(&mut self, fb: &mut FrameBuffer, g: &Geom) {
        let s = self.scale;
        let Some(rows) = &self.lib.rows else { return };
        let total = rows.total;
        let h = g.m.body.h;
        if total <= h + 1.0 {
            return;
        }
        let track = g.scroll_track;
        let max = total - h;
        let thumb_h = (track.h * h / total).clamp(36.0 * s, track.h);
        let thumb_y = track.y + (track.h - thumb_h) * (self.lib.scroll / max).clamp(0.0, 1.0);
        let hot = self.lib.hover == LibHit::Scrollbar
            || matches!(self.lib.drag, Some(super::LibDrag::Scroll { .. }));
        let r = RectF::new(
            track.x + if hot { 0.0 } else { 2.0 * s },
            thumb_y,
            if hot { 8.0 * s } else { 4.0 * s },
            thumb_h,
        );
        fb.fill_rrect(r, r.w * 0.5, Paint::Solid(if hot { t::violet_500() } else { t::ink_500() }), 1.0);
    }

    // ---- the body ------------------------------------------------------------------------------------------------------------

    /// Draw the rows that are on screen. True if something on screen is animated.
    fn draw_body(&mut self, fb: &mut FrameBuffer, g: &Geom, model: &UiModel, ctx: &LibCtx<'_>) -> bool {
        let Some(rows) = self.lib.rows.take() else { return false };
        let body = g.m.body;
        let range = visible_rows(&rows, self.lib.scroll, body.h);
        let mut animated = false;
        for ri in range {
            let row = &rows.rows[ri];
            let y = body.y + row.y - self.lib.scroll;
            match &row.kind {
                RowKind::Hero => self.draw_hero(fb, RectF::new(body.x, y, body.w, row.h), model, ctx),
                RowKind::Header(text) => {
                    let label: String = text.chars().flat_map(char::to_uppercase).collect();
                    self.text(
                        fb,
                        Face::SansBold,
                        11.0,
                        body.x + g.m.pad,
                        y + row.h - 14.0 * self.scale,
                        &label,
                        t::violet_400(),
                        1.0,
                        1.6,
                    );
                }
                RowKind::Gap => {}
                RowKind::Message(head, sub) => {
                    let rect = RectF::new(body.x, y, body.w, row.h);
                    self.draw_message(fb, rect, head, sub, ctx);
                }
                RowKind::Items(range) => {
                    for ei in range.clone() {
                        let r = rows.ent_rect(ei, &g.m);
                        let screen = RectF::new(body.x + r.x, y + (r.y - row.y), r.w, r.h);
                        self.lib.visible.push((ei, screen));
                        animated |= self.draw_ent(fb, &rows, ei, screen, g, model, ctx);
                    }
                }
            }
        }
        // The drop line of a row being dragged to a new place.
        if let Some(super::LibDrag::Reorder { from, to, moved: true, .. }) = self.lib.drag {
            if let (Some(&(_, r)), Some(&(_, f))) = (
                self.lib.visible.iter().find(|(i, _)| *i == to),
                self.lib.visible.iter().find(|(i, _)| *i == from),
            ) {
                let y = if to > from { r.bottom() } else { r.y };
                let s = self.scale;
                fb.fill_rrect(
                    RectF::new(r.x, y - 1.5 * s, r.w, 3.0 * s),
                    1.5 * s,
                    Paint::Solid(t::cyan_500()),
                    0.95,
                );
                fb.fill_rrect(
                    RectF::new(r.x - 3.0 * s, y - 4.0 * s, 8.0 * s, 8.0 * s),
                    4.0 * s,
                    Paint::Solid(t::cyan_500()),
                    1.0,
                );
                fb.stroke_rrect(f, 10.0 * s, 1.5 * s, fade(t::cyan_500(), 0.7), 1.0);
            }
        }
        self.lib.rows = Some(rows);
        animated
    }

    fn draw_message(&mut self, fb: &mut FrameBuffer, rect: RectF, head: &str, sub: &str, ctx: &LibCtx<'_>) {
        let s = self.scale;
        let cx = rect.cx();
        let mut y = rect.y + 70.0 * s;
        if self.empty_view(ctx) {
            // The empty library: the logo.
            crate::logo::draw(fb, RectF::new(cx - 46.0 * s, y - 46.0 * s, 92.0 * s, 92.0 * s), 1.0);
        } else {
            let ring = RectF::new(cx - 34.0 * s, y - 34.0 * s, 68.0 * s, 68.0 * s);
            fb.fill_rrect(ring, 34.0 * s, Paint::Solid(t::ink_800()), 1.0);
            fb.stroke_rrect(ring, 34.0 * s, 1.5 * s, fade(t::violet_500(), 0.6), 1.0);
            let icon = match self.lib.view {
                View::Search => Icon::Search,
                View::Queue => Icon::List,
                View::Playlists => Icon::ListMusic,
                View::Artists => Icon::MicVocal,
                _ => Icon::Disc3,
            };
            self.icon(fb, icon, cx, y, 30.0, t::violet_400(), 1.0, false);
        }
        y += 62.0 * s;
        let hw = self.text_w(Face::SansBold, 24.0, head, -0.2);
        self.text(fb, Face::SansBold, 24.0, cx - hw * 0.5, y, head, t::text_strong(), 1.0, -0.2);
        y += 30.0 * s;
        for line in self.wrap(Face::Sans, 14.0, sub, (rect.w - 120.0 * s).min(520.0 * s), 3) {
            let w = self.text_w(Face::Sans, 14.0, &line, 0.0);
            self.text(fb, Face::Sans, 14.0, cx - w * 0.5, y, &line, t::text_muted(), 1.0, 0.0);
            y += 22.0 * s;
        }
        if self.empty_view(ctx) && self.lib.detail.is_none() {
            for b in self.message_buttons(rect) {
                let h = LibHit::Button(b.id);
                self.draw_pill(fb, &b, self.lib.hover == h, self.lib_pressed(h), false);
            }
        }
    }

    /// The buttons of the empty-library message (ids 10 and 11).
    pub(crate) fn message_buttons(&mut self, rect: RectF) -> Vec<PillBtn> {
        let s = self.scale;
        let wa = self.text_w(Face::SansMedium, 13.0, "Add folder", 0.0) + 58.0 * s;
        let wb = self.text_w(Face::SansMedium, 13.0, "Open files", 0.0) + 58.0 * s;
        let total = wa + wb + 12.0 * s;
        let y = rect.y + 70.0 * s + 62.0 * s + 30.0 * s + 22.0 * s * 2.0 + 30.0 * s;
        let x = rect.cx() - total * 0.5;
        alloc::vec![
            PillBtn {
                id: 10,
                rect: RectF::new(x, y, wa, 42.0 * s),
                label: "Add folder".into(),
                icon: Icon::FolderPlus,
                primary: true
            },
            PillBtn {
                id: 11,
                rect: RectF::new(x + wa + 12.0 * s, y, wb, 42.0 * s),
                label: "Open files".into(),
                icon: Icon::FolderOpen,
                primary: false
            },
        ]
    }

    fn eq_bars(&mut self, fb: &mut FrameBuffer, cx: f32, cy: f32, size: f32, color: Rgba, playing: bool) {
        let s = self.scale;
        let n = 3;
        let bw = size * 0.22;
        let gap = size * 0.14;
        let total = n as f32 * bw + (n as f32 - 1.0) * gap;
        let phase = if playing && !self.config.reduce_motion { (self.now / 130_000) as u32 } else { 0 };
        for i in 0..n {
            let step = (phase + i as u32 * 5) % 7;
            let k = if playing {
                [0.35, 0.8, 0.55, 1.0, 0.45, 0.7, 0.9][step as usize]
            } else {
                [0.5, 0.9, 0.65][i as usize]
            };
            let h = size * k;
            let x = cx - total * 0.5 + i as f32 * (bw + gap);
            fb.fill_rrect(RectF::new(x, cy + size * 0.5 - h, bw, h), bw * 0.4, Paint::Solid(color), 1.0);
        }
        let _ = s;
    }

    /// Draw one card or row. True if it shows an animated indicator.
    #[allow(clippy::too_many_arguments)]
    fn draw_ent(
        &mut self,
        fb: &mut FrameBuffer,
        rows: &super::rows::Rows,
        ei: usize,
        r: RectF,
        g: &Geom,
        model: &UiModel,
        ctx: &LibCtx<'_>,
    ) -> bool {
        let s = self.scale;
        let lib = ctx.lib;
        let hover_row = matches!(self.lib.hover, LibHit::Ent(i) | LibHit::EntPlay(i) if i == ei);
        let play_hot = matches!(self.lib.hover, LibHit::EntPlay(i) if i == ei);
        let selected = self.lib.sel == Some(ei);
        let kb = selected && self.keyboard_mode && self.lib.zone == Zone::Content;
        let playing_state = model.state.is_active();
        let mut animated = false;
        match rows.ents[ei].kind {
            EntKind::Album(ai) => {
                let Some(a) = lib.albums().get(ai) else { return false };
                let cw = r.w;
                let cover = RectF::new(r.x, r.y, cw, cw);
                let now_here = model.now_track.is_some_and(|t| a.tracks.contains(&t));
                let lift = if hover_row || selected { 2.0 * s } else { 0.0 };
                let cr =
                    RectF::new(cover.x - lift, cover.y - lift, cover.w + 2.0 * lift, cover.h + 2.0 * lift);
                if hover_row || kb {
                    fb.glow_rrect(cr, 14.0 * s, 18.0 * s, t::violet_500(), 0.45);
                }
                self.draw_cover(fb, ctx, cr, 12.0 * s, a.art, a.id, 1.0);
                if hover_row || kb {
                    fb.stroke_rrect(
                        cr,
                        12.0 * s,
                        2.0 * s,
                        if kb { t::focus_ring() } else { fade(t::violet_400(), 0.9) },
                        1.0,
                    );
                    // The round play button.
                    let pb =
                        RectF::new(cover.right() - 58.0 * s, cover.bottom() - 58.0 * s, 44.0 * s, 44.0 * s);
                    let pb = if play_hot { pb.inflate(2.0 * s) } else { pb };
                    fb.glow_rrect(
                        pb,
                        pb.h * 0.5,
                        16.0 * s,
                        t::magenta_500(),
                        if play_hot { 0.7 } else { 0.4 },
                    );
                    fb.fill_rrect(pb, pb.h * 0.5, Paint::Gradient(t::gradient_tears()), 1.0);
                    self.icon(fb, Icon::Play, pb.cx() + 1.5 * s, pb.cy(), 20.0, t::white(), 1.0, true);
                }
                let ty = cover.bottom() + 20.0 * s;
                let title = self.fonts.fit(
                    Face::SansMedium,
                    14.0 * s,
                    &a.title,
                    cw - if now_here { 22.0 * s } else { 0.0 },
                );
                let tx = self.text(
                    fb,
                    Face::SansMedium,
                    14.0,
                    cover.x,
                    ty,
                    &title,
                    if now_here { t::cyan_400() } else { t::text_strong() },
                    1.0,
                    0.0,
                );
                if now_here {
                    self.eq_bars(fb, tx + 12.0 * s, ty, 12.0 * s, t::cyan_500(), playing_state);
                    animated = playing_state;
                }
                let mut sub = a.artist.clone();
                if a.year > 0 {
                    sub = alloc::format!("{sub} \u{b7} {}", a.year);
                }
                let sub = self.fonts.fit(Face::Sans, 12.5 * s, &sub, cw);
                self.text(fb, Face::Sans, 12.5, cover.x, ty + 20.0 * s, &sub, t::text_dim(), 1.0, 0.0);
            }
            EntKind::Video { id, .. } => {
                let Some(v) = lib.video(id) else { return false };
                let frac = ctx.resume.get(&id).copied().filter(|f| *f > 0.005 && *f < 0.995);
                let now_here = model.now_track == Some(id);
                if rows.is_list(ei) {
                    self.row_background(fb, r, hover_row, selected, kb);
                    let ph = r.h - 16.0 * s;
                    let poster = RectF::new(r.x + 12.0 * s, r.y + 8.0 * s, ph * 16.0 / 9.0, ph);
                    self.draw_poster(fb, ctx, poster, 8.0 * s, v.poster, v.id);
                    let tx = poster.right() + 16.0 * s;
                    let right = r.right() - 20.0 * s;
                    let dur = dur_text(v.duration_us);
                    let dw = self.text_w(Face::Mono, 12.0, &dur, 0.0);
                    self.text(fb, Face::Mono, 12.0, right - dw, r.cy(), &dur, t::text_dim(), 1.0, 0.0);
                    let mut room = right - dw - tx - 24.0 * s;
                    if let Some(f) = frac {
                        let bar = RectF::new(right - dw - 100.0 * s, r.cy() - 2.0 * s, 70.0 * s, 4.0 * s);
                        fb.fill_rrect(bar, 2.0 * s, Paint::Solid(fade(t::white(), 0.18)), 1.0);
                        fb.fill_rrect(
                            RectF::new(bar.x, bar.y, (bar.w * f).max(4.0 * s), bar.h),
                            2.0 * s,
                            Paint::Horizontal(t::magenta_500(), t::violet_400()),
                            1.0,
                        );
                        room -= 90.0 * s;
                    }
                    let title =
                        self.fonts.fit(Face::SansMedium, 15.0 * s, v.display_title(), room.max(40.0 * s));
                    self.text(
                        fb,
                        Face::SansMedium,
                        15.0,
                        tx,
                        r.cy() - 9.0 * s,
                        &title,
                        if now_here { t::cyan_400() } else { t::text_strong() },
                        1.0,
                        0.0,
                    );
                    let sub = self.fonts.fit(Face::Sans, 12.5 * s, &video_sub(v), room.max(40.0 * s));
                    self.text(fb, Face::Sans, 12.5, tx, r.cy() + 11.0 * s, &sub, t::text_dim(), 1.0, 0.0);
                } else {
                    let cw = r.w;
                    let poster = RectF::new(r.x, r.y, cw, cw * 9.0 / 16.0);
                    let lift = if hover_row || selected { 2.0 * s } else { 0.0 };
                    let pr = RectF::new(
                        poster.x - lift,
                        poster.y - lift,
                        poster.w + 2.0 * lift,
                        poster.h + 2.0 * lift,
                    );
                    if hover_row || kb {
                        fb.glow_rrect(pr, 14.0 * s, 18.0 * s, t::violet_500(), 0.45);
                    }
                    self.draw_poster(fb, ctx, pr, 12.0 * s, v.poster, v.id);
                    // The length, bottom right, on a dark chip.
                    let dur = dur_text(v.duration_us);
                    let dw = self.text_w(Face::Mono, 11.0, &dur, 0.0);
                    let chip = RectF::new(
                        poster.right() - dw - 18.0 * s,
                        poster.bottom() - 28.0 * s - if frac.is_some() { 4.0 * s } else { 0.0 },
                        dw + 12.0 * s,
                        20.0 * s,
                    );
                    fb.fill_rrect(chip, 6.0 * s, Paint::Solid(fade(t::ink_900(), 0.78)), 1.0);
                    self.text(
                        fb,
                        Face::Mono,
                        11.0,
                        chip.x + 6.0 * s,
                        chip.cy(),
                        &dur,
                        t::text_strong(),
                        1.0,
                        0.0,
                    );
                    // The resume marker: how far the film was watched, along the foot of the poster.
                    if let Some(f) = frac {
                        let bar = RectF::new(
                            poster.x + 8.0 * s,
                            poster.bottom() - 10.0 * s,
                            poster.w - 16.0 * s,
                            4.0 * s,
                        );
                        fb.fill_rrect(bar, 2.0 * s, Paint::Solid(fade(t::white(), 0.28)), 1.0);
                        fb.fill_rrect(
                            RectF::new(bar.x, bar.y, (bar.w * f).max(4.0 * s), bar.h),
                            2.0 * s,
                            Paint::Horizontal(t::magenta_500(), t::violet_400()),
                            1.0,
                        );
                    }
                    if hover_row || kb {
                        fb.stroke_rrect(
                            pr,
                            12.0 * s,
                            2.0 * s,
                            if kb { t::focus_ring() } else { fade(t::violet_400(), 0.9) },
                            1.0,
                        );
                        let pb =
                            RectF::new(poster.cx() - 24.0 * s, poster.cy() - 24.0 * s, 48.0 * s, 48.0 * s);
                        let pb = if play_hot { pb.inflate(2.0 * s) } else { pb };
                        fb.glow_rrect(
                            pb,
                            pb.h * 0.5,
                            16.0 * s,
                            t::magenta_500(),
                            if play_hot { 0.7 } else { 0.4 },
                        );
                        fb.fill_rrect(pb, pb.h * 0.5, Paint::Gradient(t::gradient_tears()), 1.0);
                        self.icon(fb, Icon::Play, pb.cx() + 1.5 * s, pb.cy(), 22.0, t::white(), 1.0, true);
                    }
                    let ty = poster.bottom() + 20.0 * s;
                    let title = self.fonts.fit(Face::SansMedium, 14.0 * s, v.display_title(), cw);
                    self.text(
                        fb,
                        Face::SansMedium,
                        14.0,
                        poster.x,
                        ty,
                        &title,
                        if now_here { t::cyan_400() } else { t::text_strong() },
                        1.0,
                        0.0,
                    );
                    let sub = self.fonts.fit(Face::Sans, 12.5 * s, &video_sub(v), cw);
                    self.text(fb, Face::Sans, 12.5, poster.x, ty + 20.0 * s, &sub, t::text_dim(), 1.0, 0.0);
                }
            }
            EntKind::Artist(ai) => {
                let Some(a) = lib.artists().get(ai) else { return false };
                self.row_background(fb, r, hover_row, selected, kb);
                let av = RectF::new(r.x + 12.0 * s, r.cy() - 25.0 * s, 50.0 * s, 50.0 * s);
                self.draw_cover(fb, ctx, av, 25.0 * s, a.art, a.id, 1.0);
                let name = self.fonts.fit(Face::SansMedium, 16.0 * s, &a.name, r.w - 120.0 * s);
                self.text(
                    fb,
                    Face::SansMedium,
                    16.0,
                    av.right() + 16.0 * s,
                    r.cy() - 9.0 * s,
                    &name,
                    t::text_strong(),
                    1.0,
                    0.0,
                );
                let sub = alloc::format!(
                    "{} \u{b7} {}",
                    plural(a.albums.len(), "album", "albums"),
                    plural(a.track_count, "track", "tracks")
                );
                self.text(
                    fb,
                    Face::Sans,
                    12.5,
                    av.right() + 16.0 * s,
                    r.cy() + 12.0 * s,
                    &sub,
                    t::text_dim(),
                    1.0,
                    0.0,
                );
                self.icon(
                    fb,
                    Icon::ChevronRight,
                    r.right() - 24.0 * s,
                    r.cy(),
                    18.0,
                    t::text_disabled(),
                    1.0,
                    false,
                );
            }
            EntKind::Track { id, pos } => {
                let Some(tr) = lib.track(id) else { return false };
                let in_album = matches!(self.lib.detail, Some(Detail::Album(_)));
                let thumbs = !in_album;
                let various =
                    in_album && lib.album_of(id).is_some_and(|a| a.artist == rvp_library::VARIOUS_ARTISTS);
                let (show_artist, show_album) = if in_album { (various, false) } else { (true, true) };
                let now_here = model.now_track == Some(id);
                let num = alloc::format!(
                    "{}",
                    if matches!(self.lib.detail, Some(Detail::Album(_))) && tr.track_no > 0 {
                        tr.track_no as usize
                    } else {
                        pos + 1
                    }
                );
                animated |= self.track_row(
                    fb,
                    r,
                    g,
                    ctx,
                    TrackRow {
                        num,
                        art: tr.art,
                        seed: tr.id,
                        title: tr.display_title(),
                        artist: tr.display_artist(),
                        album: tr.display_album(),
                        dur: tr.duration_us,
                        playing: now_here,
                        active: playing_state,
                        missing: false,
                        thumbs,
                        show_artist,
                        show_album,
                    },
                    (hover_row, play_hot, selected, kb),
                );
            }
            EntKind::PlEntry { pl, idx } => {
                let Some(e) = lib.playlist(pl).and_then(|p| p.entries.get(idx)) else { return false };
                match e.track.and_then(|t| lib.track(t)) {
                    Some(tr) => {
                        let now_here = model.now_track == Some(tr.id);
                        animated |= self.track_row(
                            fb,
                            r,
                            g,
                            ctx,
                            TrackRow {
                                num: alloc::format!("{}", idx + 1),
                                art: tr.art,
                                seed: tr.id,
                                title: tr.display_title(),
                                artist: tr.display_artist(),
                                album: tr.display_album(),
                                dur: tr.duration_us,
                                playing: now_here,
                                active: playing_state,
                                missing: false,
                                thumbs: true,
                                show_artist: true,
                                show_album: true,
                            },
                            (hover_row, play_hot, selected, kb),
                        );
                    }
                    None => {
                        let name = e
                            .title
                            .clone()
                            .unwrap_or_else(|| e.path.rsplit('/').next().unwrap_or(&e.path).into());
                        self.track_row(
                            fb,
                            r,
                            g,
                            ctx,
                            TrackRow {
                                num: alloc::format!("{}", idx + 1),
                                art: 0,
                                seed: idx as u32,
                                title: &name,
                                artist: "Missing: not in the library",
                                album: &e.path,
                                dur: (e.seconds.unwrap_or(0.0) * 1e6) as i64,
                                playing: false,
                                active: false,
                                missing: true,
                                thumbs: true,
                                show_artist: true,
                                show_album: true,
                            },
                            (hover_row, false, selected, kb),
                        );
                    }
                }
            }
            EntKind::Queue(id) => {
                let Some(q) = model.playlist.iter().find(|e| e.id == id) else { return false };
                self.row_background(fb, r, hover_row, selected, kb);
                if q.current {
                    fb.fill_rrect(
                        RectF::new(r.x, r.cy() - 14.0 * s, 3.0 * s, 28.0 * s),
                        1.5 * s,
                        Paint::Gradient(t::gradient_tears_v()),
                        1.0,
                    );
                }
                let th = RectF::new(r.x + 50.0 * s, r.cy() - 21.0 * s, 42.0 * s, 42.0 * s);
                self.draw_cover(fb, ctx, th, 8.0 * s, q.art, id, 1.0);
                let n = model.playlist.iter().position(|e| e.id == id).map_or(0, |p| p + 1);
                if q.current {
                    self.eq_bars(fb, r.x + 24.0 * s, r.cy(), 14.0 * s, t::cyan_500(), playing_state);
                    animated = playing_state;
                } else if play_hot {
                    self.icon(fb, Icon::Play, r.x + 24.0 * s, r.cy(), 16.0, t::white(), 1.0, true);
                } else {
                    let l = alloc::format!("{n}");
                    let w = self.text_w(Face::Mono, 12.0, &l, 0.0);
                    self.text(
                        fb,
                        Face::Mono,
                        12.0,
                        r.x + 24.0 * s - w * 0.5,
                        r.cy(),
                        &l,
                        t::text_dim(),
                        1.0,
                        0.0,
                    );
                }
                let tw = r.w - 50.0 * s - 42.0 * s - 12.0 * s - 80.0 * s;
                let title = self.fonts.fit(Face::SansMedium, 14.5 * s, &q.label, tw);
                let has_sub = !q.subtitle.is_empty();
                self.text(
                    fb,
                    Face::SansMedium,
                    14.5,
                    th.right() + 14.0 * s,
                    r.cy() - if has_sub { 8.0 * s } else { 0.0 },
                    &title,
                    if q.current { t::cyan_400() } else { t::text_body() },
                    1.0,
                    0.0,
                );
                if has_sub {
                    let sub = self.fonts.fit(Face::Sans, 12.5 * s, &q.subtitle, tw);
                    self.text(
                        fb,
                        Face::Sans,
                        12.5,
                        th.right() + 14.0 * s,
                        r.cy() + 11.0 * s,
                        &sub,
                        t::text_dim(),
                        1.0,
                        0.0,
                    );
                }
                let d = dur_text(q.duration_us);
                let w = self.text_w(Face::Mono, 12.0, &d, 0.0);
                self.text(
                    fb,
                    Face::Mono,
                    12.0,
                    r.right() - 16.0 * s - w,
                    r.cy(),
                    &d,
                    t::text_dim(),
                    1.0,
                    0.0,
                );
            }
            EntKind::Playlist(id) => {
                let Some(p) = lib.playlist(id) else { return false };
                self.row_background(fb, r, hover_row, selected, kb);
                let art = p
                    .entries
                    .iter()
                    .filter_map(|e| e.track)
                    .filter_map(|t| lib.track(t))
                    .map(|t| t.art)
                    .find(|a| *a != 0)
                    .unwrap_or(0);
                let th = RectF::new(r.x + 12.0 * s, r.cy() - 25.0 * s, 50.0 * s, 50.0 * s);
                self.draw_cover(fb, ctx, th, 10.0 * s, art, id, 1.0);
                let name = self.fonts.fit(Face::SansMedium, 16.0 * s, &p.name, r.w - 120.0 * s);
                self.text(
                    fb,
                    Face::SansMedium,
                    16.0,
                    th.right() + 16.0 * s,
                    r.cy() - 9.0 * s,
                    &name,
                    t::text_strong(),
                    1.0,
                    0.0,
                );
                let dur: i64 = p
                    .entries
                    .iter()
                    .filter_map(|e| e.track)
                    .filter_map(|t| lib.track(t))
                    .map(|t| t.duration_us)
                    .sum();
                let mut sub = alloc::format!(
                    "{} \u{b7} {}",
                    plural(p.entries.len(), "track", "tracks"),
                    long_duration(dur)
                );
                let miss = p.missing();
                let x0 = th.right() + 16.0 * s;
                if miss > 0 {
                    sub = alloc::format!("{sub} \u{b7} ");
                }
                let x1 =
                    self.text(fb, Face::Sans, 12.5, x0, r.cy() + 12.0 * s, &sub, t::text_dim(), 1.0, 0.0);
                if miss > 0 {
                    self.text(
                        fb,
                        Face::Sans,
                        12.5,
                        x1,
                        r.cy() + 12.0 * s,
                        &alloc::format!("{miss} missing"),
                        t::warning(),
                        1.0,
                        0.0,
                    );
                }
                self.icon(
                    fb,
                    Icon::ChevronRight,
                    r.right() - 24.0 * s,
                    r.cy(),
                    18.0,
                    t::text_disabled(),
                    1.0,
                    false,
                );
            }
        }
        animated
    }

    fn row_background(&mut self, fb: &mut FrameBuffer, r: RectF, hover: bool, selected: bool, kb: bool) {
        let s = self.scale;
        if hover || selected {
            fb.fill_rrect(
                r,
                10.0 * s,
                Paint::Solid(if selected && !hover { t::ink_800() } else { t::ink_700() }),
                1.0,
            );
        }
        if kb {
            self.focus_outline(fb, r, 10.0 * s, 1.0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn track_row(
        &mut self,
        fb: &mut FrameBuffer,
        r: RectF,
        g: &Geom,
        ctx: &LibCtx<'_>,
        row: TrackRow<'_>,
        state: (bool, bool, bool, bool),
    ) -> bool {
        let s = self.scale;
        let (hover, play_hot, selected, kb) = state;
        self.row_background(fb, r, hover, selected, kb);
        if row.playing {
            fb.fill_rrect(
                RectF::new(r.x, r.cy() - 14.0 * s, 3.0 * s, 28.0 * s),
                1.5 * s,
                Paint::Gradient(t::gradient_tears_v()),
                1.0,
            );
        }
        let cols = track_cols(r.w, s, row.thumbs, row.show_artist, row.show_album);
        let x0 = r.x;
        let mut animated = false;
        // Number, play button or equaliser.
        let ncx = x0 + cols.num.0 + cols.num.1 * 0.5;
        if row.playing {
            self.eq_bars(fb, ncx, r.cy(), 14.0 * s, t::cyan_500(), row.active);
            animated = row.active;
        } else if hover && !row.missing {
            self.icon(
                fb,
                Icon::Play,
                ncx,
                r.cy(),
                16.0,
                if play_hot { t::magenta_400() } else { t::white() },
                1.0,
                true,
            );
        } else if row.missing {
            self.icon(fb, Icon::TriangleAlert, ncx, r.cy(), 16.0, t::warning(), 1.0, false);
        } else {
            let w = self.text_w(Face::Mono, 12.0, &row.num, 0.0);
            self.text(fb, Face::Mono, 12.0, ncx - w * 0.5, r.cy(), &row.num, t::text_dim(), 1.0, 0.0);
        }
        if row.thumbs && cols.thumb.1 > 0.0 {
            let th = RectF::new(x0 + cols.thumb.0, r.cy() - cols.thumb.1 * 0.5, cols.thumb.1, cols.thumb.1);
            if row.missing {
                fb.fill_rrect(th, 7.0 * s, Paint::Solid(t::ink_800()), 1.0);
                fb.stroke_rrect(th, 7.0 * s, 1.0 * s, fade(t::warning(), 0.5), 1.0);
            } else {
                self.draw_cover(fb, ctx, th, 7.0 * s, row.art, row.seed, 1.0);
            }
        }
        let dim = if row.missing { t::text_disabled() } else { t::text_body() };
        let title_col = if row.playing { t::cyan_400() } else { dim };
        let two_line = cols.artist.is_none();
        let title = self.fonts.fit(Face::SansMedium, 14.5 * s, row.title, cols.title.1);
        self.text(
            fb,
            Face::SansMedium,
            14.5,
            x0 + cols.title.0,
            r.cy() - if two_line { 8.0 } else { 0.0 } * s,
            &title,
            title_col,
            1.0,
            0.0,
        );
        if two_line {
            let a = self.fonts.fit(Face::Sans, 12.5 * s, row.artist, cols.title.1);
            self.text(
                fb,
                Face::Sans,
                12.5,
                x0 + cols.title.0,
                r.cy() + 11.0 * s,
                &a,
                if row.missing { t::warning() } else { t::text_dim() },
                1.0,
                0.0,
            );
        }
        if let Some(a) = cols.artist {
            let txt = self.fonts.fit(Face::Sans, 13.0 * s, row.artist, a.1);
            self.text(
                fb,
                Face::Sans,
                13.0,
                x0 + a.0,
                r.cy(),
                &txt,
                if row.missing { t::warning() } else { t::text_muted() },
                1.0,
                0.0,
            );
        }
        if let Some(a) = cols.album {
            let txt = self.fonts.fit(Face::Sans, 13.0 * s, row.album, a.1);
            self.text(fb, Face::Sans, 13.0, x0 + a.0, r.cy(), &txt, t::text_dim(), 1.0, 0.0);
        }
        let d = dur_text(row.dur);
        let w = self.text_w(Face::Mono, 12.0, &d, 0.0);
        self.text(
            fb,
            Face::Mono,
            12.0,
            x0 + cols.time.0 + cols.time.1 - w,
            r.cy(),
            &d,
            t::text_dim(),
            1.0,
            0.0,
        );
        let _ = g;
        animated
    }

    fn draw_hero(&mut self, fb: &mut FrameBuffer, rect: RectF, model: &UiModel, ctx: &LibCtx<'_>) {
        let s = self.scale;
        let Some(d) = self.lib.detail else { return };
        let lib = ctx.lib;
        let cover = RectF::new(rect.x + 28.0 * s, rect.y + 20.0 * s, 196.0 * s, 196.0 * s);
        let (kicker, title, sub, art, seed): (&str, String, String, u64, u32) = match d {
            Detail::Album(id) => match lib.album(id) {
                Some(a) => {
                    let mut sub = a.artist.clone();
                    if a.year > 0 {
                        sub = alloc::format!("{sub} \u{b7} {}", a.year);
                    }
                    sub = alloc::format!(
                        "{sub} \u{b7} {} \u{b7} {}",
                        plural(a.tracks.len(), "track", "tracks"),
                        long_duration(a.duration_us)
                    );
                    if !a.genre.is_empty() {
                        sub = alloc::format!("{sub} \u{b7} {}", a.genre);
                    }
                    ("ALBUM", a.title.clone(), sub, a.art, a.id)
                }
                None => return,
            },
            Detail::Artist(id) => match lib.artist(id) {
                Some(a) => (
                    "ARTIST",
                    a.name.clone(),
                    alloc::format!(
                        "{} \u{b7} {}",
                        plural(a.albums.len(), "album", "albums"),
                        plural(a.track_count, "track", "tracks")
                    ),
                    a.art,
                    a.id,
                ),
                None => return,
            },
            Detail::Playlist(id) => match lib.playlist(id) {
                Some(p) => {
                    let art = p
                        .entries
                        .iter()
                        .filter_map(|e| e.track)
                        .filter_map(|t| lib.track(t))
                        .map(|t| t.art)
                        .find(|a| *a != 0)
                        .unwrap_or(0);
                    let dur: i64 = p
                        .entries
                        .iter()
                        .filter_map(|e| e.track)
                        .filter_map(|t| lib.track(t))
                        .map(|t| t.duration_us)
                        .sum();
                    let mut sub = alloc::format!(
                        "{} \u{b7} {}",
                        plural(p.entries.len(), "track", "tracks"),
                        long_duration(dur)
                    );
                    if p.missing() > 0 {
                        sub = alloc::format!("{sub} \u{b7} {} missing", p.missing());
                    }
                    ("PLAYLIST", p.name.clone(), sub, art, p.id)
                }
                None => return,
            },
        };
        fb.shadow_rrect(cover, 18.0 * s, 14.0 * s, 32.0 * s, Rgba::new(5, 2, 15, 170), 1.0);
        let round = if matches!(d, Detail::Artist(_)) { cover.w * 0.5 } else { 16.0 * s };
        self.draw_cover(fb, ctx, cover, round, art, seed, 1.0);
        let x = cover.right() + 28.0 * s;
        let w = rect.right() - x - 20.0 * s;
        self.text(fb, Face::SansBold, 11.0, x, rect.y + 52.0 * s, kicker, t::violet_400(), 1.0, 2.0);
        let lines = self.wrap(Face::SansBold, 34.0, &title, w, 2);
        let mut y = rect.y + 92.0 * s;
        for ln in &lines {
            self.text(fb, Face::SansBold, 34.0, x, y, ln, t::text_strong(), 1.0, -0.4);
            y += 40.0 * s;
        }
        let sub = self.fonts.fit(Face::Sans, 14.0 * s, &sub, w);
        self.text(
            fb,
            Face::Sans,
            14.0,
            x,
            (y - 8.0 * s).min(rect.bottom() - 84.0 * s),
            &sub,
            t::text_muted(),
            1.0,
            0.0,
        );
        for b in self.hero_buttons(rect, d, ctx) {
            self.lib.hero.push((b.id, b.rect, b.label.clone()));
            let h = LibHit::Button(b.id);
            self.draw_pill(fb, &b, self.lib.hover == h, self.lib_pressed(h), false);
        }
        let _ = model;
    }

    // ---- now playing ------------------------------------------------------------------------------------------------------------

    pub(crate) fn now_playing_rects(&self, g: &Geom, model: &UiModel) -> NpRects {
        let s = self.scale;
        let b = g.m.body;
        let pad = g.m.pad;
        let wide = b.w > 760.0 * s;
        let size = if wide {
            (b.h - 2.0 * pad).min(b.w * 0.42).clamp(160.0 * s, 560.0 * s)
        } else {
            (b.h * 0.42).min(b.w - 2.0 * pad).clamp(120.0 * s, 360.0 * s)
        };
        let cover = if wide {
            RectF::new(b.x + pad, b.y + (b.h - size) * 0.5, size, size)
        } else {
            RectF::new(b.cx() - size * 0.5, b.y + 12.0 * s, size, size)
        };
        let (text_x, text_w) = if wide {
            (cover.right() + 44.0 * s, b.right() - pad - cover.right() - 44.0 * s)
        } else {
            (b.x + pad, b.w - 2.0 * pad)
        };
        let mut up_next = Vec::new();
        let mut browse = None;
        if !model.has_media() {
            let bw = 190.0 * s;
            browse = Some(RectF::new(b.cx() - bw * 0.5, b.cy() + 40.0 * s, bw, 42.0 * s));
        } else if wide {
            let n = self.up_next_items(model).len();
            let top = b.bottom() - pad - n as f32 * 46.0 * s;
            for i in 0..n {
                up_next.push(RectF::new(
                    text_x - 8.0 * s,
                    top + i as f32 * 46.0 * s,
                    text_w + 16.0 * s,
                    44.0 * s,
                ));
            }
        }
        NpRects { cover, text_x, text_w, up_next, browse }
    }

    /// The now-playing screen. True if it animates.
    fn draw_now_playing(
        &mut self,
        fb: &mut FrameBuffer,
        g: &Geom,
        model: &UiModel,
        ctx: &LibCtx<'_>,
    ) -> bool {
        let s = self.scale;
        let rects = self.now_playing_rects(g, model);
        let b = g.m.body;
        if !model.has_media() {
            let cx = b.cx();
            let cy = b.cy() - 40.0 * s;
            let ring = RectF::new(cx - 40.0 * s, cy - 40.0 * s, 80.0 * s, 80.0 * s);
            fb.fill_rrect(ring, 40.0 * s, Paint::Solid(fade(t::ink_700(), 0.9)), 1.0);
            fb.stroke_rrect(ring, 40.0 * s, 1.5 * s, fade(t::violet_500(), 0.7), 1.0);
            self.icon(fb, Icon::AudioLines, cx, cy, 36.0, t::violet_400(), 1.0, false);
            let head = "Nothing is playing";
            let w = self.text_w(Face::SansBold, 24.0, head, -0.2);
            self.text(
                fb,
                Face::SansBold,
                24.0,
                cx - w * 0.5,
                cy + 74.0 * s,
                head,
                t::text_strong(),
                1.0,
                -0.2,
            );
            let sub = "Pick an album, or drop some music here.";
            let w = self.text_w(Face::Sans, 14.0, sub, 0.0);
            self.text(fb, Face::Sans, 14.0, cx - w * 0.5, cy + 102.0 * s, sub, t::text_muted(), 1.0, 0.0);
            if let Some(r) = rects.browse {
                let pb = PillBtn {
                    id: 0,
                    rect: r,
                    label: "Browse albums".into(),
                    icon: Icon::Disc3,
                    primary: true,
                };
                self.draw_pill(
                    fb,
                    &pb,
                    self.lib.hover == LibHit::Button(0),
                    self.lib_pressed(LibHit::Button(0)),
                    false,
                );
            }
            return false;
        }
        // The cover, or the picture of a video.
        let c = rects.cover;
        // A big soft shadow costs a lot to blend; with the visualizer moving behind (it dims the picture anyway) a rim will do.
        if ctx.viz.is_some() && self.lib.viz_on {
            fb.stroke_rrect(c.inflate(1.0 * s), 20.0 * s, 1.5 * s, fade(t::violet_400(), 0.35), 1.0);
        } else {
            fb.shadow_rrect(c, 20.0 * s, 18.0 * s, 44.0 * s, Rgba::new(5, 2, 15, 190), 1.0);
        }
        match (model.has_video, ctx.video, ctx.now_art) {
            (true, Some((px, w, h)), _) if w > 0 && h > 0 => {
                let k = (c.w / w as f32).min(c.h / h as f32);
                let dst = RectF::new(
                    c.cx() - w as f32 * k * 0.5,
                    c.cy() - h as f32 * k * 0.5,
                    w as f32 * k,
                    h as f32 * k,
                );
                fb.blit_scaled_rounded(dst, 12.0 * s, px, w, h);
            }
            (_, _, Some(img)) if img.w > 0 => {
                let k = (c.w / img.w as f32).min(c.h / img.h as f32);
                let dst = RectF::new(
                    c.cx() - img.w as f32 * k * 0.5,
                    c.cy() - img.h as f32 * k * 0.5,
                    img.w as f32 * k,
                    img.h as f32 * k,
                );
                fb.blit_scaled_rounded(dst, 18.0 * s, &img.rgba, img.w, img.h);
                fb.stroke_rrect(dst, 18.0 * s, 1.0 * s, fade(t::white(), 0.12), 1.0);
            }
            _ => self.draw_cover(fb, ctx, c, 18.0 * s, model.now_art, model.now_track.unwrap_or(7), 1.0),
        }
        // The text.
        let (x, w) = (rects.text_x, rects.text_w);
        let wide = b.w > 760.0 * s;
        let mut y = if wide { c.y + 44.0 * s } else { c.bottom() + 40.0 * s };
        let status = match model.state {
            MediaState::Playing | MediaState::Buffering => "NOW PLAYING",
            MediaState::Paused => "PAUSED",
            MediaState::Ended => "FINISHED",
            _ => "OPENING",
        };
        self.text(fb, Face::SansBold, 11.0, x, y, status, t::violet_400(), 1.0, 2.0);
        y += 44.0 * s;
        let title = if model.title.is_empty() { "Untitled".into() } else { model.title.clone() };
        let lines = self.wrap(Face::SansBold, if wide { 40.0 } else { 28.0 }, &title, w, 2);
        for ln in &lines {
            self.text(
                fb,
                Face::SansBold,
                if wide { 40.0 } else { 28.0 },
                x,
                y,
                ln,
                t::text_strong(),
                1.0,
                -0.5,
            );
            y += if wide { 46.0 * s } else { 34.0 * s };
        }
        if !model.artist.is_empty() {
            let a = self.fonts.fit(Face::SansMedium, 20.0 * s, &model.artist, w);
            self.text(fb, Face::SansMedium, 20.0, x, y + 4.0 * s, &a, t::text_body(), 1.0, 0.0);
            y += 32.0 * s;
        }
        if !model.album.is_empty() {
            let a = self.fonts.fit(Face::Sans, 15.0 * s, &model.album, w);
            self.text(fb, Face::Sans, 15.0, x, y + 4.0 * s, &a, t::text_muted(), 1.0, 0.0);
        }
        // Up next.
        let items = self.up_next_items(model);
        if wide && !items.is_empty() {
            let first = rects.up_next.first().copied().unwrap_or(c);
            self.text(fb, Face::SansBold, 11.0, x, first.y - 18.0 * s, "UP NEXT", t::violet_400(), 1.0, 2.0);
            for (i, (r, e)) in rects.up_next.iter().zip(items.iter()).enumerate() {
                let hot = self.lib.hover == LibHit::UpNext(i);
                if hot {
                    fb.fill_rrect(*r, 10.0 * s, Paint::Solid(fade(t::ink_700(), 0.9)), 1.0);
                }
                let th = RectF::new(r.x + 8.0 * s, r.cy() - 17.0 * s, 34.0 * s, 34.0 * s);
                self.draw_cover(fb, ctx, th, 6.0 * s, e.art, e.id, 1.0);
                let label = self.fonts.fit(Face::SansMedium, 14.0 * s, &e.label, r.w - 150.0 * s);
                self.text(
                    fb,
                    Face::SansMedium,
                    14.0,
                    th.right() + 12.0 * s,
                    r.cy() - if e.subtitle.is_empty() { 0.0 } else { 7.0 * s },
                    &label,
                    t::text_body(),
                    1.0,
                    0.0,
                );
                if !e.subtitle.is_empty() {
                    let sub = self.fonts.fit(Face::Sans, 12.0 * s, &e.subtitle, r.w - 150.0 * s);
                    self.text(
                        fb,
                        Face::Sans,
                        12.0,
                        th.right() + 12.0 * s,
                        r.cy() + 10.0 * s,
                        &sub,
                        t::text_dim(),
                        1.0,
                        0.0,
                    );
                }
                let d = dur_text(e.duration_us);
                let tw = self.text_w(Face::Mono, 12.0, &d, 0.0);
                self.text(
                    fb,
                    Face::Mono,
                    12.0,
                    r.right() - 12.0 * s - tw,
                    r.cy(),
                    &d,
                    t::text_dim(),
                    1.0,
                    0.0,
                );
            }
        }
        false
    }

    // ---- the visualizer ---------------------------------------------------------------------------------------------------------

    fn draw_viz_chrome(&mut self, fb: &mut FrameBuffer, g: &Geom, model: &UiModel, ctx: &LibCtx<'_>) {
        let s = self.scale;
        let (w, h) = (g.m.w, g.m.h);
        let a = self.controls_alpha;
        // Protection gradients under the text.
        fb.fill_rect_paint(
            RectF::new(0.0, 0.0, w, 84.0 * s),
            Paint::Vertical(fade(t::ink_900(), 0.7), Rgba::new(7, 6, 13, 0)),
            a.max(0.0),
        );
        fb.fill_rect_paint(
            RectF::new(0.0, h - 210.0 * s, w, 210.0 * s),
            Paint::Vertical(Rgba::new(7, 6, 13, 0), fade(t::ink_900(), 0.78)),
            1.0,
        );
        let name = ctx.viz.map_or("", |v| v.effect.name());
        let pal = ctx.viz.map_or("", |v| v.palette.name());
        let reduced = self.config.reduce_motion;
        // The overlay: title, artist, album.
        if self.lib.viz_info && model.has_media() {
            let x = 40.0 * s;
            let mut y = h - 190.0 * s;
            if self.controls_alpha > 0.3 {
                y -= 12.0 * s;
            }
            let t1 = self.fonts.fit(Face::SansBold, 34.0 * s, &model.title, w - 2.0 * x);
            self.text(fb, Face::SansBold, 34.0, x, y, &t1, t::text_strong(), 1.0, -0.4);
            if !model.artist.is_empty() {
                let t2 = self.fonts.fit(Face::SansMedium, 20.0 * s, &model.artist, w - 2.0 * x);
                self.text(fb, Face::SansMedium, 20.0, x, y + 34.0 * s, &t2, t::text_body(), 1.0, 0.0);
            }
            let time = alloc::format!(
                "{} / {}",
                format_time(model.position_us),
                model.duration_us.map_or(String::from("--:--"), format_time)
            );
            self.text(fb, Face::Mono, 13.0, x, y + 62.0 * s, &time, t::cyan_400(), 1.0, 0.0);
        }
        // Back and the effect name.
        if a > 0.01 {
            let back = RectF::new(24.0 * s, 20.0 * s, 110.0 * s, 38.0 * s);
            fb.fill_rrect(back, 19.0 * s, Paint::Solid(fade(t::ink_900(), 0.7)), a);
            fb.stroke_rrect(back, 19.0 * s, 1.0 * s, fade(t::white(), 0.2), a);
            self.icon(fb, Icon::ChevronLeft, back.x + 22.0 * s, back.cy(), 18.0, t::text_body(), a, false);
            self.text(
                fb,
                Face::SansMedium,
                13.0,
                back.x + 40.0 * s,
                back.cy(),
                "Esc  Back",
                t::text_body(),
                a,
                0.0,
            );
            let label = alloc::format!("{} \u{b7} {}", name.to_uppercase(), pal.to_uppercase());
            let lw = self.text_w(Face::MonoBold, 11.5, &label, 1.2);
            self.text(fb, Face::MonoBold, 11.5, w - 28.0 * s - lw, 39.0 * s, &label, t::cyan_400(), a, 1.2);
        }
        if !self.lib.viz_on {
            // Animation is off (the default with reduced motion): a calm card says how to turn it on.
            let card = RectF::new(w * 0.5 - 250.0 * s, h * 0.5 - 110.0 * s, 500.0 * s, 190.0 * s);
            fb.fill_rrect(card, 24.0 * s, Paint::Solid(fade(t::ink_800(), 0.94)), 1.0);
            fb.stroke_rrect(card, 24.0 * s, 1.0 * s, t::border_subtle(), 1.0);
            self.icon(fb, Icon::Sparkles, card.cx(), card.y + 52.0 * s, 30.0, t::violet_400(), 1.0, false);
            let head = if reduced { "The visualizer is resting." } else { "The visualizer is off." };
            let hw = self.text_w(Face::SansBold, 22.0, head, -0.2);
            self.text(
                fb,
                Face::SansBold,
                22.0,
                card.cx() - hw * 0.5,
                card.y + 100.0 * s,
                head,
                t::text_strong(),
                1.0,
                -0.2,
            );
            let sub = if reduced {
                "Motion is reduced on this device. Press Enter to start a calm version."
            } else {
                "Press Enter to start it."
            };
            for (i, ln) in self.wrap(Face::Sans, 14.0, sub, card.w - 60.0 * s, 2).iter().enumerate() {
                let lw = self.text_w(Face::Sans, 14.0, ln, 0.0);
                self.text(
                    fb,
                    Face::Sans,
                    14.0,
                    card.cx() - lw * 0.5,
                    card.y + 130.0 * s + i as f32 * 22.0 * s,
                    ln,
                    t::text_muted(),
                    1.0,
                    0.0,
                );
            }
        }
        // The switcher.
        if a > 0.01 {
            let icons = [
                Icon::ChevronLeft,
                Icon::ChevronRight,
                Icon::Sparkles,
                Icon::Info,
                if self.lib.viz_on { Icon::Pause } else { Icon::Play },
            ];
            let (first, last) = (g.viz_btns.first().map(|b| b.1), g.viz_btns.last().map(|b| b.1));
            if let (Some(f), Some(l)) = (first, last) {
                let pill =
                    RectF::new(f.x - 14.0 * s, f.y - 8.0 * s, l.right() - f.x + 28.0 * s, f.h + 16.0 * s);
                fb.shadow_rrect(pill, pill.h * 0.5, 6.0 * s, 20.0 * s, Rgba::new(5, 2, 15, 150), a);
                fb.fill_rrect(pill, pill.h * 0.5, Paint::Solid(fade(t::ink_800(), 0.92)), a);
                fb.stroke_rrect(pill, pill.h * 0.5, 1.0 * s, fade(t::white(), 0.14), a);
            }
            for (i, r) in &g.viz_btns {
                let hot = self.lib.hover == LibHit::Viz(*i);
                if hot {
                    fb.fill_rrect(*r, r.h * 0.5, Paint::Solid(fade(t::white(), 0.12)), a);
                }
                let on = (*i == 3 && self.lib.viz_info) || (*i == 4 && self.lib.viz_on);
                self.icon(
                    fb,
                    icons[*i as usize],
                    r.cx(),
                    r.cy(),
                    18.0,
                    if on || hot { t::cyan_400() } else { t::text_body() },
                    a,
                    false,
                );
            }
            // The effect's name between the arrows.
            if g.viz_btns.len() >= 2 {
                let (p, n) = (g.viz_btns[0].1, g.viz_btns[1].1);
                let gap = n.x - p.right();
                let tw = self.text_w(Face::SansMedium, 13.0, name, 0.0);
                self.text(
                    fb,
                    Face::SansMedium,
                    13.0,
                    p.right() + gap * 0.5 - tw * 0.5,
                    p.cy(),
                    name,
                    t::text_strong(),
                    a,
                    0.0,
                );
            }
        }
    }

    // ---- the bar ------------------------------------------------------------------------------------------------------------------

    fn draw_lib_bar(&mut self, fb: &mut FrameBuffer, g: &Geom, model: &UiModel, ctx: &LibCtx<'_>, a: f32) {
        let s = self.scale;
        let b = g.m.bar;
        if self.lib.view == View::Visualizer {
            fb.fill_rect_paint(b, Paint::Vertical(Rgba::new(7, 6, 13, 0), fade(t::ink_900(), 0.92)), a);
        } else {
            fb.fill_rect_paint(b, Paint::Solid(t::ink_850()), 0.98);
            fb.fill_rect_paint(RectF::new(b.x, b.y, b.w, 1.0 * s), Paint::Solid(t::white()), 0.07);
        }
        // What is playing.
        if model.has_media() {
            self.draw_cover(fb, ctx, g.bar_art, 10.0 * s, model.now_art, model.now_track.unwrap_or(3), a);
            if g.bar_info.w > 120.0 * s {
                let tx = g.bar_art.right() + 14.0 * s;
                let tw = g.bar_info.right() - tx;
                let hot = self.lib.hover == LibHit::BarInfo;
                let title = self.fonts.fit(Face::SansMedium, 14.5 * s, &model.title, tw);
                self.text(
                    fb,
                    Face::SansMedium,
                    14.5,
                    tx,
                    b.y + 36.0 * s,
                    &title,
                    if hot { t::cyan_400() } else { t::text_strong() },
                    a,
                    0.0,
                );
                let sub = if !model.artist.is_empty() { model.artist.clone() } else { String::new() };
                if !sub.is_empty() {
                    let sub = self.fonts.fit(Face::Sans, 12.5 * s, &sub, tw);
                    self.text(fb, Face::Sans, 12.5, tx, b.y + 58.0 * s, &sub, t::text_dim(), a, 0.0);
                }
            }
        } else {
            fb.fill_rrect(g.bar_art, 10.0 * s, Paint::Solid(t::ink_800()), a);
            self.icon(fb, Icon::Music, g.bar_art.cx(), g.bar_art.cy(), 24.0, t::text_disabled(), a, false);
            if g.bar_info.w > 120.0 * s {
                self.text(
                    fb,
                    Face::Sans,
                    13.5,
                    g.bar_art.right() + 14.0 * s,
                    b.y + 48.0 * s,
                    "Nothing playing",
                    t::text_dim(),
                    a,
                    0.0,
                );
            }
        }
        // Buttons.
        for (btn, r) in g.bar_btns.clone() {
            self.draw_lib_button(fb, btn, r, model, a);
        }
        // Seek bar.
        let hot = self.lib.hover == LibHit::Seek || self.scrub.is_some();
        let tr = g.seek_track;
        let th = if hot { 6.0 * s } else { 4.0 * s };
        let track = RectF::new(tr.x, tr.cy() - th * 0.5, tr.w, th);
        fb.fill_rrect(track, th * 0.5, Paint::Solid(fade(t::white(), 0.2)), a);
        let frac = match (self.scrub, model.duration_us) {
            (Some(f), _) => f,
            (None, Some(d)) if d > 0 => (model.position_us as f32 / d as f32).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let px = track.x + track.w * frac;
        if frac > 0.0 {
            fb.fill_rrect(
                RectF::new(track.x, track.y, (px - track.x).max(th), th),
                th * 0.5,
                Paint::Horizontal(t::magenta_500(), t::violet_400()),
                a,
            );
        }
        if hot {
            let k = RectF::new(px - 7.0 * s, tr.cy() - 7.0 * s, 14.0 * s, 14.0 * s);
            fb.glow_rrect(k, 7.0 * s, 14.0 * s, t::cyan_500(), 0.55 * a);
            fb.fill_rrect(k, 7.0 * s, Paint::Solid(t::cyan_500()), a);
        } else if model.has_media() {
            fb.fill_rrect(
                RectF::new(px - 1.0 * s, tr.cy() - 6.0 * s, 2.0 * s, 12.0 * s),
                1.0 * s,
                Paint::Solid(t::cyan_500()),
                a,
            );
        }
        let cur = format_time(if let (Some(f), Some(d)) = (self.scrub, model.duration_us) {
            (f as f64 * d as f64) as i64
        } else {
            model.position_us
        });
        let cw = self.text_w(Face::Mono, 12.0, &cur, 0.0);
        self.text(fb, Face::Mono, 12.0, g.time_l + 46.0 * s - cw, g.time_y, &cur, t::cyan_500(), a, 0.0);
        let total = model.duration_us.map_or(String::from("--:--"), format_time);
        self.text(fb, Face::Mono, 12.0, g.time_r, g.time_y, &total, t::text_dim(), a, 0.0);
        // Volume.
        if let Some(vh) = g.vol_hit {
            let vt = g.vol_track;
            let vol = if model.muted { 0.0 } else { model.volume };
            fb.fill_rrect(vt, vt.h * 0.5, Paint::Solid(fade(t::white(), 0.22)), a);
            if vol > 0.0 {
                fb.fill_rrect(
                    RectF::new(vt.x, vt.y, (vt.w * vol).max(vt.h), vt.h),
                    vt.h * 0.5,
                    Paint::Solid(fade(t::pink_white(), 0.92)),
                    a,
                );
            }
            let hot =
                self.lib.hover == LibHit::Volume || matches!(self.lib.drag, Some(super::LibDrag::Volume));
            let kr = if hot { 6.5 * s } else { 5.0 * s };
            fb.fill_rrect(
                RectF::new(vt.x + vt.w * vol - kr, vh.cy() - kr, kr * 2.0, kr * 2.0),
                kr,
                Paint::Solid(t::text_strong()),
                a,
            );
        }
    }

    fn draw_lib_button(&mut self, fb: &mut FrameBuffer, btn: Btn, r: RectF, model: &UiModel, a: f32) {
        let s = self.scale;
        let h = LibHit::Bar(btn);
        let hot = self.lib.hover == h;
        let pressed = hot && self.pressed_lib == Some(h);
        let bar_idx = self.lib_bar_index(btn);
        let focused = self.lib.zone == Zone::Bar && self.keyboard_mode && self.lib.bar_focus == bar_idx;
        let rr = if pressed { r.scaled(0.94) } else { r };
        if btn == Btn::Play {
            if hot || focused {
                fb.glow_rrect(rr, rr.h * 0.5, 22.0 * s, t::magenta_500(), 0.6 * a);
            }
            fb.fill_rrect(rr, rr.h * 0.5, Paint::Gradient(t::gradient_tears()), a);
            if hot {
                fb.fill_rrect(rr, rr.h * 0.5, Paint::Solid(t::white()), 0.14 * a);
            }
            let icon = if model.state.is_active() { Icon::Pause } else { Icon::Play };
            self.icon(
                fb,
                icon,
                rr.cx() + if icon == Icon::Play { 1.5 * s } else { 0.0 },
                rr.cy(),
                20.0,
                t::white(),
                a,
                true,
            );
            if focused {
                self.focus_ring(fb, rr, rr.h * 0.5, a);
            }
            return;
        }
        if hot || pressed {
            fb.fill_rrect(rr, 10.0 * s, Paint::Solid(fade(t::white(), 0.12)), a);
        }
        let (icon, on) = match btn {
            Btn::Shuffle => (Icon::Shuffle, model.shuffle),
            Btn::Prev => (Icon::SkipBack, false),
            Btn::Next => (Icon::SkipForward, false),
            Btn::Repeat => (Icon::for_repeat(model.repeat), model.repeat != 0),
            Btn::Mute => (
                if model.muted || model.volume <= 0.0 {
                    Icon::VolumeX
                } else if model.volume < 0.5 {
                    Icon::Volume1
                } else {
                    Icon::Volume2
                },
                false,
            ),
            Btn::QueueView => (Icon::List, self.lib.view == View::Queue),
            Btn::VizView => (Icon::Sparkles, self.lib.view == View::Visualizer),
            Btn::ModeSwitch => (Icon::Film, false),
            _ => (Icon::Play, false),
        };
        let col = if on {
            t::cyan_500()
        } else if hot {
            t::white()
        } else {
            t::text_body()
        };
        let filled = matches!(btn, Btn::Prev | Btn::Next);
        self.icon(fb, icon, rr.cx(), rr.cy(), 19.0, col, a, filled);
        if on && matches!(btn, Btn::Shuffle | Btn::Repeat) {
            fb.fill_rrect(
                RectF::new(rr.cx() - 2.0 * s, rr.bottom() - 4.0 * s, 4.0 * s, 4.0 * s),
                2.0 * s,
                Paint::Solid(t::cyan_500()),
                a,
            );
        }
        if focused {
            self.focus_ring(fb, rr, 10.0 * s, a);
        }
    }

    fn lib_bar_index(&self, btn: Btn) -> usize {
        // The order the geometry pushed them in: shuffle, prev, play, next, repeat, mode, viz, queue, mute.
        [
            Btn::Shuffle,
            Btn::Prev,
            Btn::Play,
            Btn::Next,
            Btn::Repeat,
            Btn::ModeSwitch,
            Btn::VizView,
            Btn::QueueView,
            Btn::Mute,
        ]
        .iter()
        .position(|b| *b == btn)
        .unwrap_or(0)
    }

    fn draw_lib_tooltip(&mut self, fb: &mut FrameBuffer, g: &Geom, model: &UiModel) {
        let LibHit::Bar(btn) = self.lib.hover else { return };
        if !self.menu.is_empty() || self.lib.drag.is_some() || self.now - self.hover_since < TOOLTIP_DELAY_US
        {
            return;
        }
        let Some((_, anchor)) = g.bar_btns.iter().find(|(b, _)| *b == btn) else { return };
        let (label, key) = match btn {
            Btn::Play => (if model.state.is_active() { "Pause" } else { "Play" }, "Space"),
            Btn::Prev => ("Previous", "P"),
            Btn::Next => ("Next", "N"),
            Btn::Shuffle => (model.shuffle_label(), "Z"),
            Btn::Repeat => (model.repeat_label(), "R"),
            Btn::Mute => (if model.muted { "Unmute" } else { "Mute" }, "M"),
            Btn::QueueView => ("Queue", "5"),
            Btn::VizView => {
                (if self.lib.view == View::Visualizer { "Leave visualizer" } else { "Visualizer" }, "V")
            }
            Btn::ModeSwitch => ("Player", "B"),
            _ => return,
        };
        let s = self.scale;
        let lw = self.text_w(Face::SansMedium, 12.5, label, 0.0);
        let kw = self.text_w(Face::MonoBold, 11.0, key, 0.0);
        let w = lw + kw + 40.0 * s;
        let r = RectF::new(
            (anchor.cx() - w * 0.5).clamp(8.0 * s, g.m.w - w - 8.0 * s),
            anchor.y - 12.0 * s - 30.0 * s,
            w,
            30.0 * s,
        );
        fb.shadow_rrect(r, 8.0 * s, 4.0 * s, 12.0 * s, Rgba::new(5, 2, 15, 140), 1.0);
        fb.fill_rrect(r, 8.0 * s, Paint::Solid(t::ink_700()), 1.0);
        fb.stroke_rrect(r, 8.0 * s, 1.0 * s, t::ink_500(), 1.0);
        let x =
            self.text(fb, Face::SansMedium, 12.5, r.x + 12.0 * s, r.cy(), label, t::text_body(), 1.0, 0.0);
        let chip = RectF::new(x + 10.0 * s, r.cy() - 9.0 * s, kw + 12.0 * s, 18.0 * s);
        fb.fill_rrect(chip, 5.0 * s, Paint::Solid(fade(t::cyan_500(), 0.14)), 1.0);
        self.text(fb, Face::MonoBold, 11.0, chip.x + 6.0 * s, chip.cy(), key, t::cyan_400(), 1.0, 0.0);
    }

    // ---- the prompt ------------------------------------------------------------------------------------------------------------------

    fn draw_prompt(&mut self, fb: &mut FrameBuffer, g: &Geom) {
        let s = self.scale;
        let Some(p) = self.lib.prompt.clone() else { return };
        fb.fill_rect_paint(RectF::new(0.0, 0.0, g.m.w, g.m.h), Paint::Solid(t::ink_900()), 0.6);
        let card = self.prompt_card(g);
        fb.shadow_rrect(card, 22.0 * s, 20.0 * s, 56.0 * s, Rgba::new(5, 2, 15, 190), 1.0);
        fb.fill_rrect(card, 22.0 * s, Paint::Solid(t::ink_800()), 1.0);
        fb.stroke_rrect(card, 22.0 * s, 1.0 * s, t::border_subtle(), 1.0);
        fb.fill_rrect(
            RectF::new(card.x + 40.0 * s, card.y + 1.0 * s, card.w - 80.0 * s, 2.0 * s),
            1.0 * s,
            Paint::GradientFaded(t::gradient_tears(), 0.9),
            1.0,
        );
        self.text(
            fb,
            Face::SansBold,
            20.0,
            card.x + 28.0 * s,
            card.y + 44.0 * s,
            &p.title,
            t::text_strong(),
            1.0,
            -0.2,
        );
        let field = RectF::new(card.x + 28.0 * s, card.y + 70.0 * s, card.w - 56.0 * s, 44.0 * s);
        fb.glow_rrect(field, 12.0 * s, 12.0 * s, t::cyan_500(), 0.3);
        fb.fill_rrect(field, 12.0 * s, Paint::Solid(t::ink_900()), 1.0);
        fb.stroke_rrect(field, 12.0 * s, 2.0 * s, t::focus_ring(), 1.0);
        let shown = self.fonts.fit(Face::Sans, 15.0 * s, &p.text, field.w - 32.0 * s);
        let x = self.text(
            fb,
            Face::Sans,
            15.0,
            field.x + 16.0 * s,
            field.cy(),
            &shown,
            t::text_strong(),
            1.0,
            0.0,
        );
        if p.text.is_empty() {
            self.text(
                fb,
                Face::Sans,
                15.0,
                field.x + 16.0 * s,
                field.cy(),
                "Playlist name",
                t::text_dim(),
                1.0,
                0.0,
            );
        }
        if (self.now / 530_000) % 2 == 0 {
            fb.fill_rect_paint(
                RectF::new(
                    x + if p.text.is_empty() { 0.0 } else { 1.0 * s },
                    field.cy() - 10.0 * s,
                    1.5 * s,
                    20.0 * s,
                ),
                Paint::Solid(t::cyan_500()),
                1.0,
            );
        }
        let (ok, cancel) = self.prompt_buttons(g);
        let label = match p.kind {
            super::PromptKind::NewPlaylist(_) => "Create",
            super::PromptKind::Rename(_) => "Rename",
        };
        let okb = PillBtn { id: 0, rect: ok, label: label.into(), icon: Icon::Check, primary: true };
        let cb = PillBtn { id: 1, rect: cancel, label: "Cancel".into(), icon: Icon::X, primary: false };
        self.draw_pill(fb, &okb, self.lib.hover == LibHit::PromptOk, false, false);
        self.draw_pill(fb, &cb, self.lib.hover == LibHit::PromptCancel, false, false);
    }
}

/// What a track row shows.
pub(crate) struct TrackRow<'a> {
    pub num: String,
    pub art: u64,
    pub seed: u32,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub dur: i64,
    pub playing: bool,
    pub active: bool,
    pub missing: bool,
    pub thumbs: bool,
    pub show_artist: bool,
    pub show_album: bool,
}
