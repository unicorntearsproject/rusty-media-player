//! Text: the bundled Space Grotesk (UI) and JetBrains Mono (timecodes, key hints) faces rasterised with
//! `fontdue`, with a glyph cache. There are no system fonts in wasm, so the fonts are embedded (SIL OFL 1.1,
//! see `THIRD_PARTY_LICENSES.md`; the files are Latin subsets).
//!
//! A character the face does not have is looked up in a chain of fallbacks: a bundled Noto Sans subset (Greek,
//! Cyrillic, Latin extended, Vietnamese), then fonts a host supplies. A desktop host installs a [`FontLoader`] that
//! finds a system font covering the character (CJK, Arabic, ...); it is asked at most once per character and its
//! answer is kept, so a font is only read when a name that needs it is drawn.
use crate::gfx::FrameBuffer;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use fontdue::{Font, FontSettings};
use libm::roundf;
use theme::Rgba;

const SANS: &[u8] = include_bytes!("../assets/fonts/SpaceGrotesk-Regular.ttf");
const SANS_MEDIUM: &[u8] = include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf");
const SANS_BOLD: &[u8] = include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf");
const MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const MONO_BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf");
const NOTO: &[u8] = include_bytes!("../assets/fonts/NotoSans-Subset.ttf");

/// A font file a host found for a character.
pub struct FontData {
    /// The file (TrueType, OpenType or a collection).
    pub bytes: Vec<u8>,
    /// Which font of a collection (0 for a plain file).
    pub index: u32,
}

/// Asked for a font that has the given character when no loaded font does (a desktop host searches the system's fonts).
pub type FontLoader = Box<dyn FnMut(char) -> Option<FontData>>;

/// A bundled typeface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Face {
    /// Space Grotesk Regular.
    Sans,
    /// Space Grotesk Medium.
    SansMedium,
    /// Space Grotesk Bold.
    SansBold,
    /// JetBrains Mono Regular.
    Mono,
    /// JetBrains Mono Bold.
    MonoBold,
}

struct Glyph {
    /// False when no font has the character (the face's own box is drawn).
    found: bool,
    xmin: i32,
    ymin: i32,
    w: u32,
    h: u32,
    advance: f32,
    bitmap: Vec<u8>,
}

/// The font set and its glyph cache.
pub struct Fonts {
    fonts: [Font; 5],
    /// The bundled Noto Sans subset, then whatever the host added.
    fallbacks: Vec<Font>,
    loader: Option<FontLoader>,
    /// Characters no font has (so the loader is not asked again).
    unknown: BTreeSet<char>,
    cache: BTreeMap<(Face, char, u32), Glyph>,
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}

impl Fonts {
    /// Parse the embedded fonts.
    pub fn new() -> Self {
        let load = |b: &[u8]| Font::from_bytes(b, FontSettings::default()).expect("bundled font parses");
        Self {
            fonts: [load(SANS), load(SANS_MEDIUM), load(SANS_BOLD), load(MONO), load(MONO_BOLD)],
            fallbacks: alloc::vec![load(NOTO)],
            loader: None,
            unknown: BTreeSet::new(),
            cache: BTreeMap::new(),
        }
    }

    /// Install the function that finds a system font for characters the bundled fonts lack.
    pub fn set_loader(&mut self, loader: FontLoader) {
        self.loader = Some(loader);
        self.unknown.clear();
        self.cache.retain(|_, g| g.found);
    }

    /// Add a fallback font (after the ones already there). Returns false if the file does not parse.
    pub fn add_fallback(&mut self, bytes: &[u8], index: u32) -> bool {
        let ok = self.add_fallback_keep(bytes, index);
        if ok {
            self.unknown.clear();
            self.cache.retain(|_, g| g.found);
        }
        ok
    }

    /// True if some loaded font has `ch` (the loader is not consulted).
    pub fn has_glyph(&self, face: Face, ch: char) -> bool {
        self.pick(face, ch).is_some()
    }

    /// Which font draws `ch`: `Some(None)` is the face itself, `Some(Some(i))` fallback `i`, `None` no loaded font.
    fn pick(&self, face: Face, ch: char) -> Option<Option<usize>> {
        if ch.is_whitespace() || ch.is_control() || self.font(face).lookup_glyph_index(ch) != 0 {
            return Some(None);
        }
        self.fallbacks.iter().position(|f| f.lookup_glyph_index(ch) != 0).map(Some)
    }

    /// Like [`Fonts::add_fallback`] but keeps the caches (it runs while a glyph is being made).
    fn add_fallback_keep(&mut self, bytes: &[u8], index: u32) -> bool {
        let settings = FontSettings { collection_index: index, ..FontSettings::default() };
        match Font::from_bytes(bytes, settings) {
            Ok(f) => {
                self.fallbacks.push(f);
                true
            }
            Err(_) => false,
        }
    }

    fn font(&self, face: Face) -> &Font {
        &self.fonts[match face {
            Face::Sans => 0,
            Face::SansMedium => 1,
            Face::SansBold => 2,
            Face::Mono => 3,
            Face::MonoBold => 4,
        }]
    }

    fn glyph(&mut self, face: Face, ch: char, px: f32) -> &Glyph {
        let key = (face, ch, (px * 4.0) as u32);
        if !self.cache.contains_key(&key) {
            let mut which = self.pick(face, ch);
            if which.is_none() && !self.unknown.contains(&ch) {
                // Ask the host once for a font with this character.
                if let Some(d) = self.loader.as_mut().and_then(|l| l(ch)) {
                    if self.add_fallback_keep(&d.bytes, d.index) {
                        which = self.pick(face, ch);
                    }
                }
                if which.is_none() {
                    self.unknown.insert(ch);
                }
            }
            let font = match which {
                Some(Some(i)) => &self.fallbacks[i],
                _ => self.font(face),
            };
            let (m, bitmap) = font.rasterize(ch, px);
            self.cache.insert(
                key,
                Glyph {
                    found: which.is_some(),
                    xmin: m.xmin,
                    ymin: m.ymin,
                    w: m.width as u32,
                    h: m.height as u32,
                    advance: m.advance_width,
                    bitmap,
                },
            );
        }
        &self.cache[&key]
    }

    /// Width of `text` in pixels at size `px`, with `tracking` extra pixels after every glyph.
    pub fn measure(&mut self, face: Face, px: f32, text: &str, tracking: f32) -> f32 {
        let mut w = 0.0;
        for ch in text.chars() {
            w += self.glyph(face, ch, px).advance + tracking;
        }
        w
    }

    /// Draw `text` with its left edge at `x` and its baseline at `baseline`. Returns the right edge.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        fb: &mut FrameBuffer,
        face: Face,
        px: f32,
        x: f32,
        baseline: f32,
        text: &str,
        color: Rgba,
        opacity: f32,
        tracking: f32,
    ) -> f32 {
        let mut pen = x;
        for ch in text.chars() {
            let g = self.glyph(face, ch, px);
            if g.w > 0 {
                let gx = roundf(pen) as i32 + g.xmin;
                let gy = roundf(baseline) as i32 - g.ymin - g.h as i32;
                fb.blit_mask(gx, gy, g.w, g.h, &g.bitmap, color, opacity);
            }
            pen += g.advance + tracking;
        }
        pen
    }

    /// Draw `text` horizontally centred on `cx` with its cap-height centred on `cy`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_centered(
        &mut self,
        fb: &mut FrameBuffer,
        face: Face,
        px: f32,
        cx: f32,
        cy: f32,
        text: &str,
        color: Rgba,
        opacity: f32,
        tracking: f32,
    ) {
        let w = self.measure(face, px, text, tracking) - tracking;
        self.draw(fb, face, px, cx - w * 0.5, cy + px * 0.36, text, color, opacity, tracking);
    }

    /// `text`, cut and ended with an ellipsis if it is wider than `max_w`.
    pub fn fit(&mut self, face: Face, px: f32, text: &str, max_w: f32) -> String {
        if self.measure(face, px, text, 0.0) <= max_w {
            return text.into();
        }
        let mut out: Vec<char> = text.chars().collect();
        while !out.is_empty() {
            out.pop();
            let mut s: String = out.iter().collect();
            s.push('\u{2026}');
            if self.measure(face, px, &s, 0.0) <= max_w {
                return s;
            }
        }
        "\u{2026}".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theme::tokens;

    #[test]
    fn measures_and_draws() {
        let mut f = Fonts::new();
        let w = f.measure(Face::Sans, 16.0, "Play", 0.0);
        assert!(w > 20.0 && w < 60.0, "{w}");
        // JetBrains Mono is tabular: every digit has the same advance.
        let a = f.measure(Face::Mono, 14.0, "0:00", 0.0);
        let b = f.measure(Face::Mono, 14.0, "8:88", 0.0);
        assert!((a - b).abs() < 0.01);
        let mut fb = FrameBuffer::new(80, 30);
        f.draw(&mut fb, Face::SansBold, 20.0, 4.0, 22.0, "Hi", tokens::WHITE, 1.0, 0.0);
        assert!(fb.pixels.chunks_exact(4).any(|p| p[0] > 200));
    }

    #[test]
    fn falls_back_for_cyrillic_and_greek_and_asks_the_loader_once_for_the_rest() {
        let mut f = Fonts::new();
        // Space Grotesk is Latin only; the bundled Noto subset draws Cyrillic and Greek.
        assert_eq!(f.font(Face::Sans).lookup_glyph_index('Ж'), 0);
        assert!(f.has_glyph(Face::Sans, 'Ж') && f.has_glyph(Face::Mono, 'λ'));
        let cyr = f.measure(Face::Sans, 16.0, "Привет", 0.0);
        assert!(cyr > 40.0, "{cyr}");
        let mut fb = FrameBuffer::new(120, 30);
        f.draw(&mut fb, Face::Sans, 20.0, 4.0, 22.0, "Жук", tokens::WHITE, 1.0, 0.0);
        assert!(fb.pixels.chunks_exact(4).filter(|p| p[0] > 100).count() > 40, "Cyrillic is drawn");
        // A CJK character has no font until the loader supplies one; it is asked once per character.
        assert!(!f.has_glyph(Face::Sans, '日'));
        let asked = alloc::rc::Rc::new(core::cell::Cell::new(0));
        let a2 = asked.clone();
        f.set_loader(Box::new(move |_| {
            a2.set(a2.get() + 1);
            None
        }));
        f.measure(Face::Sans, 16.0, "日日日", 0.0);
        f.measure(Face::Sans, 18.0, "日", 0.0);
        assert_eq!(asked.get(), 1);
    }

    #[test]
    fn fit_ellipsizes() {
        let mut f = Fonts::new();
        let s = f.fit(Face::Sans, 14.0, "a very long file name for a short window.mkv", 100.0);
        assert!(s.ends_with('\u{2026}') && f.measure(Face::Sans, 14.0, &s, 0.0) <= 100.0);
        assert_eq!(f.fit(Face::Sans, 14.0, "ok", 100.0), "ok");
    }
}
