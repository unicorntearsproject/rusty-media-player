//! Text: the bundled Space Grotesk (UI) and JetBrains Mono (timecodes, key hints) faces rasterised with
//! `fontdue`, with a glyph cache. There are no system fonts in wasm, so the fonts are embedded (SIL OFL 1.1,
//! see `THIRD_PARTY_LICENSES.md`; the files are Latin subsets).
use crate::gfx::FrameBuffer;
use alloc::collections::BTreeMap;
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
            cache: BTreeMap::new(),
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
            let (m, bitmap) = self.font(face).rasterize(ch, px);
            self.cache.insert(
                key,
                Glyph {
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
    fn fit_ellipsizes() {
        let mut f = Fonts::new();
        let s = f.fit(Face::Sans, 14.0, "a very long file name for a short window.mkv", 100.0);
        assert!(s.ends_with('\u{2026}') && f.measure(Face::Sans, 14.0, &s, 0.0) <= 100.0);
        assert_eq!(f.fit(Face::Sans, 14.0, "ok", 100.0), "ok");
    }
}
