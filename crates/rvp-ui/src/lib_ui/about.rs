//! The About page: three short paragraphs (the person, the app, the system it grew up in), the version and build, and the links.
//! Drawn and hit-tested from one layout, so a button is where it looks like it is.
use super::LibHit;
use super::geom::PillBtn;
use crate::font::Face;
use crate::gfx::{FrameBuffer, RectF};
use crate::icon::Icon;
use crate::model::UiModel;
use crate::tk as t;
use crate::ui::Ui;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The page's text: a kicker and a paragraph each.
pub(crate) const PARAGRAPHS: [(&str, &str); 3] = [
    (
        "DJ Unicorn Tears",
        "DJ Unicorn Tears is a DJ, a visualizer builder and, between sets, a principal cloud and infrastructure architect: ten-plus \
         years of keeping production AWS standing, a Linux habit, a Terraform tab that never closes and a lifelong soft spot for the \
         demoscene. Unicorn Viz, the free audio-reactive visualizer for DJs and VJs that doesn't suck, came out of that same neon \
         brain. 10% of proceeds helps animals in need.",
    ),
    (
        "Rusty Wave",
        "Rusty Wave is the media player that does not waste your time: music and video, a library with posters and favorites, \
         playlists, subtitles and a visualizer, all decoded by Rust you can read. It runs the same in a browser tab, on a desktop and \
         inside Rusty Bucket. Everything stays on your device: no accounts, no tracking, no cloud that belongs to someone else.",
    ),
    (
        "Rusty Bucket",
        "Rusty Bucket is an operating system written from scratch in Rust, where the whole desktop is a tabbed browser and every \
         program is a sandboxed WebAssembly app. Rusty Wave is its built-in Media app and never needs the bucket to run. Come and \
         look at the bucket: rustybucket.ai",
    ),
];

/// The buttons: id (what [`super::LibAction::About`] carries), label, shown only where the host opens links.
const LINKS: [(u8, &str, Icon); 2] =
    [(0, "Rusty Bucket", Icon::ExternalLink), (1, "@djunicorntears on X", Icon::ExternalLink)];
const LICENSES: (u8, &str, Icon) = (2, "Licenses", Icon::Download);

impl Ui {
    /// Lay out the About page in `rect` (the page's block on screen) and, when `fb` is given, draw it. Returns the buttons.
    pub(crate) fn about_page(
        &mut self,
        mut fb: Option<&mut FrameBuffer>,
        rect: RectF,
        model: &UiModel,
    ) -> Vec<PillBtn> {
        let s = self.scale;
        let x = rect.x + 28.0 * s;
        let maxw = (rect.w - 56.0 * s).clamp(200.0 * s, 680.0 * s);
        let mut y = rect.y + 28.0 * s;
        // The logo, the name, the version and the build.
        if let Some(fb) = fb.as_deref_mut() {
            crate::logo::draw(fb, RectF::new(x, y, 72.0 * s, 72.0 * s), 1.0);
            self.text(
                fb,
                Face::SansBold,
                32.0,
                x + 88.0 * s,
                y + 26.0 * s,
                "Rusty Wave",
                t::text_strong(),
                1.0,
                -0.4,
            );
            let build = format!("Version {} \u{b7} build {}", model.version, model.commit);
            self.text(fb, Face::Mono, 12.0, x + 88.0 * s, y + 56.0 * s, &build, t::text_dim(), 1.0, 0.0);
        }
        y += 104.0 * s;
        for (kicker, para) in PARAGRAPHS {
            if let Some(fb) = fb.as_deref_mut() {
                let label: String = kicker.chars().flat_map(char::to_uppercase).collect();
                self.text(fb, Face::SansBold, 11.0, x, y, &label, t::violet_400(), 1.0, 1.6);
            }
            y += 24.0 * s;
            for line in self.wrap(Face::Sans, 15.0, para, maxw, 12) {
                if let Some(fb) = fb.as_deref_mut() {
                    self.text(fb, Face::Sans, 15.0, x, y, &line, t::text_body(), 1.0, 0.0);
                }
                y += 24.0 * s;
            }
            y += 16.0 * s;
        }
        // The links: buttons where the host can open them, else the addresses as text.
        let mut btns: Vec<PillBtn> = Vec::new();
        let mut defs: Vec<(u8, &str, Icon)> = Vec::new();
        if model.app.links {
            defs.extend(LINKS);
        } else {
            for line in ["rustybucket.ai", "x.com/djunicorntears"] {
                if let Some(fb) = fb.as_deref_mut() {
                    self.text(fb, Face::Mono, 13.0, x, y, line, t::cyan_400(), 1.0, 0.0);
                }
                y += 22.0 * s;
            }
            y += 8.0 * s;
        }
        defs.push(LICENSES);
        let mut bx = x;
        for (id, label, icon) in defs {
            let w = self.text_w(Face::SansMedium, 13.0, label, 0.0) + 8.0 * s + 46.0 * s;
            if bx + w > x + maxw && bx > x {
                bx = x;
                y += 50.0 * s;
            }
            btns.push(PillBtn {
                id,
                rect: RectF::new(bx, y, w, 40.0 * s),
                label: label.into(),
                icon,
                primary: id == 0,
            });
            bx += w + 10.0 * s;
        }
        y += 40.0 * s + 28.0 * s;
        if let Some(fb) = fb.as_deref_mut() {
            self.text(
                fb,
                Face::Sans,
                12.0,
                x,
                y,
                "Rusty Wave is free software, MIT or Apache-2.0. Made with Rust.",
                t::text_dim(),
                1.0,
                0.0,
            );
            for b in &btns {
                let h = LibHit::Button(b.id);
                self.draw_pill(fb, b, self.lib.hover == h, self.lib_pressed(h), false);
            }
        }
        btns
    }
}
