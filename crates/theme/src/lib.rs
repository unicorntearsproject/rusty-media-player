//! Unicorn Tears design-system tokens as Rust constants.
//!
//! `tokens.rs` is **generated** by `cargo xtask theme` from `tokens/*.css`, a vendored snapshot of
//! `claude-design-system/tokens` (refresh the snapshot with `cargo xtask theme --sync`). Do not edit it.
//! Lengths are CSS pixels (`rem` x 16), durations are milliseconds, colours are straight-alpha RGBA8.
#![no_std]

#[rustfmt::skip]
pub mod tokens;

/// A straight-alpha RGBA8 colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha (255 = opaque).
    pub a: u8,
}

impl Rgba {
    /// From four channels.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Packed `0xRRGGBBAA`.
    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << 24) | ((self.g as u32) << 16) | ((self.b as u32) << 8) | self.a as u32
    }
}

/// One stop of a gradient; `pos` is 0.0..=1.0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    /// Position along the gradient.
    pub pos: f32,
    /// Colour at the stop.
    pub color: Rgba,
}

/// A CSS `linear-gradient(<angle>deg, ...)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearGradient {
    /// CSS angle in degrees (0 = to top, 90 = to right, 180 = to bottom).
    pub angle_deg: f32,
    /// Stops in order.
    pub stops: &'static [GradientStop],
}

/// A CSS `radial-gradient(<rx>% <ry>% at <cx>% <cy>%, ...)` (ellipse).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadialGradient {
    /// Horizontal radius, percent of the box width.
    pub rx_pct: f32,
    /// Vertical radius, percent of the box height.
    pub ry_pct: f32,
    /// Centre x, percent.
    pub cx_pct: f32,
    /// Centre y, percent.
    pub cy_pct: f32,
    /// Stops in order.
    pub stops: &'static [GradientStop],
}

/// One CSS box-shadow layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    /// Inner shadow.
    pub inset: bool,
    /// X offset in px.
    pub x: f32,
    /// Y offset in px.
    pub y: f32,
    /// Blur radius in px.
    pub blur: f32,
    /// Colour.
    pub color: Rgba,
}

/// A CSS `cubic-bezier(x1, y1, x2, y2)` easing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicBezier(pub f32, pub f32, pub f32, pub f32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_tokens_match_the_design_system() {
        assert_eq!(tokens::INK_900, Rgba::rgb(0x07, 0x06, 0x0d));
        assert_eq!(tokens::MAGENTA_500.to_u32(), 0xff2bd6ff);
        assert_eq!(tokens::CYAN_500, Rgba::rgb(0x19, 0xe3, 0xff));
        assert_eq!(tokens::BG_PAGE, tokens::INK_900); // alias resolution
        assert_eq!(tokens::SPACE_4, 16.0); // rem -> px
        assert_eq!(tokens::DUR_BASE, 220);
        assert_eq!(tokens::GRADIENT_TEARS.stops.len(), 3);
        assert_eq!(tokens::GLOW_MAGENTA.len(), 2);
    }
}
