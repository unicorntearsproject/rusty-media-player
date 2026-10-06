//! The tag editor: a modal form with a box for every tag, the cover, and Save and Cancel. The app fills it in ([`TagFormSpec`]) and
//! gets back what the person changed ([`UiCommand::SaveTags`]): only the boxes that differ from what they were opened with, so an
//! album's shared fields can be edited without touching what differs from song to song.
use super::geom::PillBtn;
use super::{LibCtx, LibHit, UiCommand};
use crate::font::Face;
use crate::gfx::{FrameBuffer, Paint, RectF, fade};
use crate::icon::Icon;
use crate::tk as t;
use crate::ui::Ui;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_host::{Key, Modifiers};
use theme::Rgba;

/// A tag the form can edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagField {
    /// Title.
    Title,
    /// Artist.
    Artist,
    /// Album.
    Album,
    /// Album artist.
    AlbumArtist,
    /// Genre.
    Genre,
    /// Track number.
    TrackNo,
    /// Tracks on the disc.
    TrackTotal,
    /// Disc number.
    DiscNo,
    /// Discs in the set.
    DiscTotal,
    /// Year.
    Year,
}

impl TagField {
    /// The label over the box.
    pub fn label(self) -> &'static str {
        match self {
            TagField::Title => "Title",
            TagField::Artist => "Artist",
            TagField::Album => "Album",
            TagField::AlbumArtist => "Album artist",
            TagField::Genre => "Genre",
            TagField::TrackNo => "Track",
            TagField::TrackTotal => "of",
            TagField::DiscNo => "Disc",
            TagField::DiscTotal => "of",
            TagField::Year => "Year",
        }
    }

    /// A stable name (the snapshot and the tests).
    pub fn name(self) -> &'static str {
        match self {
            TagField::Title => "title",
            TagField::Artist => "artist",
            TagField::Album => "album",
            TagField::AlbumArtist => "album_artist",
            TagField::Genre => "genre",
            TagField::TrackNo => "track_no",
            TagField::TrackTotal => "track_total",
            TagField::DiscNo => "disc_no",
            TagField::DiscTotal => "disc_total",
            TagField::Year => "year",
        }
    }

    /// A box that takes digits only.
    pub fn numeric(self) -> bool {
        matches!(
            self,
            TagField::TrackNo
                | TagField::TrackTotal
                | TagField::DiscNo
                | TagField::DiscTotal
                | TagField::Year
        )
    }

    fn max_len(self) -> usize {
        if self.numeric() { 4 } else { 200 }
    }
}

/// What the form is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagTarget {
    /// One song (library id).
    Track(u32),
    /// Every song of an album (album id): the fields they share.
    Album(u32),
}

/// What happens to the cover when the form is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoverAction {
    /// Nothing.
    #[default]
    Keep,
    /// Take the picture out.
    Remove,
    /// Put in the picture the person chose (the app has it).
    Replace,
}

/// What the app tells the form to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagFormSpec {
    /// The heading.
    pub title: String,
    /// Under it: the file name, or how many songs.
    pub subtitle: String,
    /// What it edits.
    pub target: TagTarget,
    /// The boxes, in the order they are laid out: the field, its value, and whether the songs differ (an album's boxes).
    pub fields: Vec<(TagField, String, bool)>,
    /// The cover now (a library thumbnail id), 0 if none.
    pub art: u64,
    /// Why nothing can be saved (the host cannot write here): the form opens read-only and says so.
    pub read_only: Option<String>,
}

pub(crate) struct TagForm {
    pub spec: TagFormSpec,
    pub values: Vec<String>,
    pub focus: usize,
    pub cover: CoverAction,
    pub cover_name: String,
}

/// Where everything is.
pub(crate) struct TagLayout {
    pub card: RectF,
    pub title_y: f32,
    pub note: Option<(f32, f32, f32)>,
    /// Per field: the label's position and the box.
    pub fields: Vec<(f32, f32, RectF)>,
    pub thumb: RectF,
    pub cover_text: (f32, f32, f32),
    pub buttons: Vec<PillBtn>,
}

const SAVE: u8 = 0;
const CANCEL: u8 = 1;
const REPLACE: u8 = 2;
const REMOVE: u8 = 3;

impl Ui {
    /// Open the tag editor.
    pub fn open_tag_form(&mut self, spec: TagFormSpec) {
        let values = spec.fields.iter().map(|f| f.1.clone()).collect();
        self.lib.tagform =
            Some(TagForm { spec, values, focus: 0, cover: CoverAction::Keep, cover_name: String::new() });
        self.menu.clear();
        self.dirty = true;
    }

    /// True while the tag editor is open.
    pub fn tag_form_open(&self) -> bool {
        self.lib.tagform.is_some()
    }

    /// The person chose a new cover picture (`Some(name)`), or the choice was dropped.
    pub fn set_tag_cover(&mut self, name: Option<String>) {
        if let Some(f) = &mut self.lib.tagform {
            match name {
                Some(n) => {
                    f.cover = CoverAction::Replace;
                    f.cover_name = n;
                }
                None => {
                    f.cover = CoverAction::Keep;
                    f.cover_name.clear();
                }
            }
            self.dirty = true;
        }
    }

    pub(crate) fn tagform_layout(&mut self, w: f32, h: f32) -> Option<TagLayout> {
        let s = self.scale;
        let (read_only, n_fields) = {
            let f = self.lib.tagform.as_ref()?;
            (f.spec.read_only.is_some(), f.spec.fields.len())
        };
        let kinds: Vec<TagField> = self.lib.tagform.as_ref()?.spec.fields.iter().map(|f| f.0).collect();
        let cw = (700.0 * s).min(w - 32.0 * s).max(280.0 * s);
        let wide = cw >= 560.0 * s;
        let inner = cw - 56.0 * s;
        // Rows of text boxes (a title takes a row; the others pair up when wide), then one row of the small numeric ones.
        let texts: Vec<usize> = (0..n_fields).filter(|&i| !kinds[i].numeric()).collect();
        let nums: Vec<usize> = (0..n_fields).filter(|&i| kinds[i].numeric()).collect();
        let mut rows: Vec<Vec<usize>> = Vec::new();
        let mut pending: Vec<usize> = Vec::new();
        for &i in &texts {
            if kinds[i] == TagField::Title || !wide {
                if !pending.is_empty() {
                    rows.push(core::mem::take(&mut pending));
                }
                rows.push(alloc::vec![i]);
            } else {
                pending.push(i);
                if pending.len() == 2 {
                    rows.push(core::mem::take(&mut pending));
                }
            }
        }
        if !pending.is_empty() {
            rows.push(pending);
        }
        if !nums.is_empty() {
            rows.push(nums);
        }
        // Heights: the heading, the rows, the cover, the buttons; rows shrink when the window is short.
        let head = if read_only { 124.0 * s } else { 84.0 * s };
        let (cover_h, foot) = (84.0 * s, 70.0 * s);
        let avail = h - 24.0 * s - head - cover_h - foot;
        let row_h = (avail / rows.len().max(1) as f32).clamp(54.0 * s, 70.0 * s);
        let ch = head + rows.len() as f32 * row_h + cover_h + foot;
        let card = RectF::new((w - cw) * 0.5, ((h - ch) * 0.4).max(12.0 * s), cw, ch);
        let mut fields = alloc::vec![(0.0, 0.0, RectF::default()); n_fields];
        let box_h = 38.0 * s;
        for (ri, row) in rows.iter().enumerate() {
            let y = card.y + head + ri as f32 * row_h;
            let gap = 14.0 * s;
            let numeric_row = row.iter().all(|&i| kinds[i].numeric());
            let total = row.len() as f32;
            let each = if numeric_row {
                // Five small boxes share the row; a lone one is not stretched across it.
                ((inner - gap * (total - 1.0)) / total).min(120.0 * s)
            } else {
                (inner - gap * (total - 1.0)) / total
            };
            for (k, &i) in row.iter().enumerate() {
                let x = card.x + 28.0 * s + k as f32 * (each + gap);
                fields[i] = (x, y + 12.0 * s, RectF::new(x, y + 20.0 * s, each, box_h));
            }
        }
        let cy = card.y + head + rows.len() as f32 * row_h;
        let thumb = RectF::new(card.x + 28.0 * s, cy + 8.0 * s, 56.0 * s, 56.0 * s);
        let by = card.bottom() - 58.0 * s;
        let mut buttons = Vec::new();
        let mk = |id: u8, x: f32, y: f32, w: f32, label: &str, icon: Icon, primary: bool| PillBtn {
            id,
            rect: RectF::new(x, y, w, 38.0 * s),
            label: label.into(),
            icon,
            primary,
        };
        if read_only {
            buttons.push(mk(
                CANCEL,
                card.right() - 28.0 * s - 110.0 * s,
                by,
                110.0 * s,
                "Close",
                Icon::X,
                true,
            ));
        } else {
            buttons.push(mk(
                SAVE,
                card.right() - 28.0 * s - 120.0 * s,
                by,
                120.0 * s,
                "Save",
                Icon::Check,
                true,
            ));
            buttons.push(mk(
                CANCEL,
                card.right() - 28.0 * s - 120.0 * s - 10.0 * s - 110.0 * s,
                by,
                110.0 * s,
                "Cancel",
                Icon::X,
                false,
            ));
            // A narrow card keeps the two cover buttons as icons.
            let narrow = cw < 460.0 * s;
            let rw = if narrow {
                40.0 * s
            } else {
                (self.text_w(Face::SansMedium, 13.0, "Replace\u{2026}", 0.0) + 8.0 * s + 46.0 * s)
                    .max(120.0 * s)
            };
            let mw = if narrow {
                40.0 * s
            } else {
                (self.text_w(Face::SansMedium, 13.0, "Remove", 0.0) + 8.0 * s + 46.0 * s).max(110.0 * s)
            };
            let bx = thumb.right() + 16.0 * s;
            let y1 = cy + 22.0 * s;
            buttons.push(mk(
                REPLACE,
                bx,
                y1,
                rw,
                if narrow { "" } else { "Replace\u{2026}" },
                Icon::Upload,
                false,
            ));
            buttons.push(mk(
                REMOVE,
                bx + rw + 8.0 * s,
                y1,
                mw,
                if narrow { "" } else { "Remove" },
                Icon::Trash2,
                false,
            ));
        }
        Some(TagLayout {
            card,
            title_y: card.y + 42.0 * s,
            note: read_only.then_some((card.x + 28.0 * s, card.y + 72.0 * s, inner)),
            fields,
            thumb,
            cover_text: (thumb.right() + 16.0 * s, cy + 22.0 * s, inner - 72.0 * s),
            buttons,
        })
    }

    pub(crate) fn tagform_hit(&mut self, x: f32, y: f32, w: f32, h: f32) -> LibHit {
        let Some(l) = self.tagform_layout(w, h) else { return LibHit::None };
        for b in &l.buttons {
            if b.rect.contains(x, y) {
                return LibHit::TagButton(b.id);
            }
        }
        let read_only = self.lib.tagform.as_ref().is_some_and(|f| f.spec.read_only.is_some());
        if !read_only {
            for (i, f) in l.fields.iter().enumerate() {
                if f.2.contains(x, y) {
                    return LibHit::TagField(i);
                }
            }
        }
        LibHit::None
    }

    /// What a click on a part of the form does.
    pub(crate) fn tagform_click(&mut self, hit: LibHit) {
        match hit {
            LibHit::TagField(i) => {
                if let Some(f) = &mut self.lib.tagform {
                    f.focus = i.min(f.values.len().saturating_sub(1));
                }
            }
            LibHit::TagButton(SAVE) => self.tagform_save(),
            LibHit::TagButton(CANCEL) => self.lib.tagform = None,
            LibHit::TagButton(REPLACE) => self.lib.commands.push(UiCommand::PickCover),
            LibHit::TagButton(REMOVE) => {
                if let Some(f) = &mut self.lib.tagform {
                    f.cover =
                        if f.cover == CoverAction::Remove { CoverAction::Keep } else { CoverAction::Remove };
                    f.cover_name.clear();
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn tagform_save(&mut self) {
        let Some(f) = self.lib.tagform.take() else { return };
        if f.spec.read_only.is_some() {
            return;
        }
        let mut changes = Vec::new();
        for (i, (field, orig, _)) in f.spec.fields.iter().enumerate() {
            let now = f.values[i].trim();
            if now != orig.trim() {
                changes.push((*field, (!now.is_empty()).then(|| now.to_string())));
            }
        }
        if changes.is_empty() && f.cover == CoverAction::Keep {
            return; // nothing changed: nothing to do
        }
        self.lib.commands.push(UiCommand::SaveTags { target: f.spec.target, changes, cover: f.cover });
    }

    pub(crate) fn tagform_key(&mut self, key: &Key, mods: &Modifiers) {
        let Some(f) = &mut self.lib.tagform else { return };
        let n = f.values.len();
        match key {
            Key::Escape => self.lib.tagform = None,
            Key::Enter => {
                if f.spec.read_only.is_some() {
                    self.lib.tagform = None;
                } else {
                    self.tagform_save();
                }
            }
            Key::Other(k) if k == "Tab" && n > 0 => {
                f.focus = if mods.shift { (f.focus + n - 1) % n } else { (f.focus + 1) % n };
            }
            Key::Down if n > 0 => f.focus = (f.focus + 1) % n,
            Key::Up if n > 0 => f.focus = (f.focus + n - 1) % n,
            Key::Other(k) if k == "Backspace" && n > 0 && f.spec.read_only.is_none() => {
                f.values[f.focus].pop();
            }
            Key::Space | Key::Char(_) if n > 0 && f.spec.read_only.is_none() && !mods.ctrl && !mods.alt => {
                let c = if let Key::Char(c) = key { *c } else { ' ' };
                let field = f.spec.fields[f.focus].0;
                if !c.is_control()
                    && (!field.numeric() || c.is_ascii_digit())
                    && f.values[f.focus].chars().count() < field.max_len()
                {
                    f.values[f.focus].push(c);
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    pub(crate) fn tagform_paste(&mut self, text: &str) {
        let Some(f) = &mut self.lib.tagform else { return };
        if f.values.is_empty() || f.spec.read_only.is_some() {
            return;
        }
        let field = f.spec.fields[f.focus].0;
        let room = field.max_len().saturating_sub(f.values[f.focus].chars().count());
        let clean: String = text
            .chars()
            .filter(|c| !c.is_control() && (!field.numeric() || c.is_ascii_digit()))
            .take(room)
            .collect();
        f.values[f.focus].push_str(&clean);
        self.dirty = true;
    }

    pub(crate) fn draw_tagform(&mut self, fb: &mut FrameBuffer, w: f32, h: f32, ctx: &LibCtx<'_>) {
        let s = self.scale;
        let Some(l) = self.tagform_layout(w, h) else { return };
        self.lib.tagform_json = self.tagform_json(w, h);
        let Some(form) = self.lib.tagform.as_ref() else { return };
        let (spec, values, focus, cover, cover_name) =
            (form.spec.clone(), form.values.clone(), form.focus, form.cover, form.cover_name.clone());
        fb.fill_rect_paint(RectF::new(0.0, 0.0, w, h), Paint::Solid(t::ink_900()), 0.6);
        let card = l.card;
        fb.shadow_rrect(card, 22.0 * s, 20.0 * s, 56.0 * s, Rgba::new(5, 2, 15, 190), 1.0);
        fb.fill_rrect(card, 22.0 * s, Paint::Solid(t::ink_800()), 1.0);
        fb.stroke_rrect(card, 22.0 * s, 1.0 * s, t::border_subtle(), 1.0);
        fb.fill_rrect(
            RectF::new(card.x + 40.0 * s, card.y + 1.0 * s, card.w - 80.0 * s, 2.0 * s),
            1.0 * s,
            Paint::GradientFaded(t::gradient_tears(), 0.9),
            1.0,
        );
        let tx = card.x + 28.0 * s;
        let title = self.fonts.fit(Face::SansBold, 20.0 * s, &spec.title, card.w - 56.0 * s);
        self.text(fb, Face::SansBold, 20.0, tx, l.title_y, &title, t::text_strong(), 1.0, -0.2);
        let sub = self.fonts.fit(Face::Sans, 12.5 * s, &spec.subtitle, card.w - 56.0 * s);
        self.text(fb, Face::Sans, 12.5, tx, l.title_y + 22.0 * s, &sub, t::text_dim(), 1.0, 0.0);
        if let (Some((nx, ny, nw)), Some(why)) = (l.note, &spec.read_only) {
            for (i, line) in self.wrap(Face::Sans, 13.5, why, nw, 4).iter().enumerate() {
                self.text(fb, Face::Sans, 13.5, nx, ny + i as f32 * 20.0 * s, line, t::warning(), 1.0, 0.0);
            }
        }
        for (i, (field, orig, mixed)) in spec.fields.iter().enumerate() {
            let (lx, ly, r) = l.fields[i];
            let label: String = field.label().chars().flat_map(char::to_uppercase).collect();
            self.text(fb, Face::SansBold, 10.5, lx, ly, &label, t::violet_400(), 1.0, 1.4);
            let on = focus == i && spec.read_only.is_none();
            let changed = values[i].trim() != orig.trim();
            if on {
                fb.glow_rrect(r, 10.0 * s, 10.0 * s, t::cyan_500(), 0.28);
            }
            fb.fill_rrect(r, 10.0 * s, Paint::Solid(t::ink_900()), 1.0);
            let edge = if on {
                t::focus_ring()
            } else if changed {
                fade(t::violet_400(), 0.9)
            } else {
                t::border_subtle()
            };
            fb.stroke_rrect(r, 10.0 * s, if on { 2.0 * s } else { 1.0 * s }, edge, 1.0);
            let shown = self.fonts.fit(Face::Sans, 14.0 * s, &values[i], r.w - 24.0 * s);
            let x =
                self.text(fb, Face::Sans, 14.0, r.x + 12.0 * s, r.cy(), &shown, t::text_strong(), 1.0, 0.0);
            if values[i].is_empty() && *mixed {
                self.text(fb, Face::Sans, 14.0, r.x + 12.0 * s, r.cy(), "Different", t::text_dim(), 1.0, 0.0);
            }
            if on && (self.now / 530_000) % 2 == 0 {
                fb.fill_rect_paint(
                    RectF::new(
                        x + if values[i].is_empty() { 0.0 } else { 1.0 * s },
                        r.cy() - 9.0 * s,
                        1.5 * s,
                        18.0 * s,
                    ),
                    Paint::Solid(t::cyan_400()),
                    1.0,
                );
            }
        }
        // The cover: the picture now, and what will happen to it.
        let label = "COVER";
        self.text(fb, Face::SansBold, 10.5, l.thumb.x, l.thumb.y - 6.0 * s, label, t::violet_400(), 1.0, 1.4);
        let (art, tag) = match cover {
            CoverAction::Remove => (0, "The cover will be removed."),
            CoverAction::Replace => (0, ""),
            CoverAction::Keep if spec.art != 0 => (spec.art, "The cover stays."),
            CoverAction::Keep => (0, "No cover yet."),
        };
        self.draw_cover(fb, ctx, l.thumb, 8.0 * s, art, 7, 1.0);
        if cover == CoverAction::Replace {
            self.icon(fb, Icon::Upload, l.thumb.cx(), l.thumb.cy(), 22.0, t::cyan_400(), 1.0, false);
        }
        let text = if cover == CoverAction::Replace {
            alloc::format!("New cover: {cover_name}")
        } else {
            tag.to_string()
        };
        let (cx, cy, cw) = l.cover_text;
        let text = self.fonts.fit(Face::Sans, 13.0 * s, &text, cw.max(40.0 * s));
        self.text(fb, Face::Sans, 13.0, cx, cy - 8.0 * s, &text, t::text_muted(), 1.0, 0.0);
        for b in &l.buttons {
            let h = LibHit::TagButton(b.id);
            let mut b = b.clone();
            if b.id == REMOVE && cover == CoverAction::Remove {
                b.label = "Keep".into();
                b.icon = Icon::Check;
            }
            self.draw_pill(fb, &b, self.lib.hover == h, self.lib_pressed(h), false);
        }
    }

    /// The form for the snapshot: its fields, buttons and what it says.
    pub(crate) fn tagform_json(&mut self, w: f32, h: f32) -> String {
        use alloc::format;
        let Some(l) = self.tagform_layout(w, h) else { return "null".into() };
        let Some(f) = &self.lib.tagform else { return "null".into() };
        let rect =
            |r: RectF| format!("{{\"x\":{:.1},\"y\":{:.1},\"w\":{:.1},\"h\":{:.1}}}", r.x, r.y, r.w, r.h);
        let esc = |s: &str| {
            let mut o = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => o.push_str("\\\""),
                    '\\' => o.push_str("\\\\"),
                    c if (c as u32) < 0x20 => o.push(' '),
                    c => o.push(c),
                }
            }
            o.push('"');
            o
        };
        let fields: Vec<String> = f
            .spec
            .fields
            .iter()
            .enumerate()
            .map(|(i, (k, orig, mixed))| {
                format!(
                    "{{\"key\":\"{}\",\"value\":{},\"orig\":{},\"mixed\":{},\"focus\":{},\"rect\":{}}}",
                    k.name(),
                    esc(&f.values[i]),
                    esc(orig),
                    mixed,
                    i == f.focus,
                    rect(l.fields[i].2)
                )
            })
            .collect();
        let buttons: Vec<String> = l
            .buttons
            .iter()
            .map(|b| format!("{{\"id\":{},\"label\":{},\"rect\":{}}}", b.id, esc(&b.label), rect(b.rect)))
            .collect();
        format!(
            "{{\"title\":{},\"subtitle\":{},\"read_only\":{},\"cover\":\"{:?}\",\"cover_name\":{},\"card\":{},\"fields\":[{}],\"buttons\":[{}]}}",
            esc(&f.spec.title),
            esc(&f.spec.subtitle),
            f.spec.read_only.as_deref().map_or("null".to_string(), esc),
            f.cover,
            esc(&f.cover_name),
            rect(l.card),
            fields.join(","),
            buttons.join(",")
        )
    }
}
