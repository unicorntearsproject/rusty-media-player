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
/// The button ids of the links inside the closing line (the X profile and the GitHub issues), after the pills.
pub(crate) const SUGGEST_X: u8 = 4;
pub(crate) const SUGGEST_GITHUB: u8 = 5;

/// The closing line. The two links are words in it; where the host cannot open links the addresses follow them as text.
fn suggestion_words(links: bool) -> Vec<(&'static str, Option<u8>, bool)> {
    // (word, link id, glued to the word before it)
    let mut v = alloc::vec![
        ("Please", None, false),
        ("provide", None, false),
        ("suggestions,", None, false),
        ("requests", None, false),
        ("&", None, false),
        ("bug", None, false),
        ("reports", None, false),
        ("via", None, false),
        ("X", Some(SUGGEST_X), false),
    ];
    if !links {
        v.push(("(x.com/djunicorntears)", None, false));
    }
    v.push(("or", None, false));
    v.push(("GitHub", Some(SUGGEST_GITHUB), false));
    if !links {
        v.push(("(github.com/unicorntearsproject/rusty-media-player/issues)", None, false));
    }
    v.push(("!", None, true));
    v
}

const LICENSES: (u8, &str, Icon) = (2, "Licenses", Icon::Download);
const SHORTCUTS_BTN: (u8, &str, Icon) = (3, "Keyboard shortcuts", Icon::Info);

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
        defs.push(SHORTCUTS_BTN);
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
        // The closing line: suggestions go to X or GitHub, both words are links where the host opens them.
        let suggest_y = y + 34.0 * s;
        let words = suggestion_words(model.app.links);
        let space = self.text_w(Face::Sans, 15.0, "a a", 0.0) - 2.0 * self.text_w(Face::Sans, 15.0, "a", 0.0);
        let (mut cx, mut line_y) = (x, suggest_y);
        let mut placed: Vec<(&str, Option<u8>, f32, f32)> = Vec::new();
        for (word, link, glued) in &words {
            let ww = self.text_w(Face::Sans, 15.0, word, 0.0);
            let gap = if *glued || cx == x { 0.0 } else { space };
            if cx > x && !*glued && cx + gap + ww > x + maxw {
                cx = x;
                line_y += 24.0 * s;
            } else {
                cx += gap;
            }
            placed.push((word, *link, cx, line_y));
            cx += ww;
        }
        for (word, link, wx, wy) in &placed {
            if let Some(id) = link.filter(|_| model.app.links) {
                let ww = self.text_w(Face::Sans, 15.0, word, 0.0);
                btns.push(PillBtn {
                    id,
                    rect: RectF::new(*wx - 4.0 * s, *wy - 12.0 * s, ww + 8.0 * s, 24.0 * s),
                    label: (*word).into(),
                    icon: Icon::ExternalLink,
                    primary: false,
                });
            }
        }
        if let Some(fb) = fb.as_deref_mut() {
            for (word, link, wx, wy) in &placed {
                let is_link = link.is_some() && model.app.links;
                let hot = link.is_some_and(|id| self.lib.hover == LibHit::Button(id));
                let col = if is_link { if hot { t::white() } else { t::cyan_400() } } else { t::text_body() };
                let face = if is_link { Face::SansMedium } else { Face::Sans };
                let ww = self.text_w(Face::Sans, 15.0, word, 0.0);
                self.text(fb, face, 15.0, *wx, *wy, word, col, 1.0, 0.0);
                if is_link {
                    fb.fill_rect_paint(
                        RectF::new(*wx, *wy + 9.0 * s, ww, if hot { 2.0 * s } else { 1.0 * s }),
                        crate::gfx::Paint::Solid(col),
                        1.0,
                    );
                }
            }
        }
        y = line_y + 24.0 * s;
        if let Some(fb) = fb {
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
            // (The links inside the closing line were drawn with it.)
            for b in btns.iter().filter(|b| b.id < SUGGEST_X) {
                let h = LibHit::Button(b.id);
                self.draw_pill(fb, b, self.lib.hover == h, self.lib_pressed(h), false);
            }
        }
        btns
    }
}
