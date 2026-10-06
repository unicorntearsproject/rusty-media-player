//! CSS colours: parse what design systems write (`#rgb`, `#rrggbb[aa]`, `rgb()`, `hsl()`, `oklch()`, a few names, `transparent`), and the
//! maths the theme checks need (relative luminance, WCAG contrast, mixing).
use alloc::string::String;
use libm::{cbrtf, cosf, fabsf, powf, roundf, sinf};
use theme::Rgba;

/// A named CSS colour a design system might use (the common ones; anything else is not understood and the token keeps its default).
const NAMES: &[(&str, [u8; 3])] = &[
    ("white", [255, 255, 255]),
    ("black", [0, 0, 0]),
    ("red", [255, 0, 0]),
    ("green", [0, 128, 0]),
    ("blue", [0, 0, 255]),
    ("yellow", [255, 255, 0]),
    ("cyan", [0, 255, 255]),
    ("aqua", [0, 255, 255]),
    ("magenta", [255, 0, 255]),
    ("fuchsia", [255, 0, 255]),
    ("orange", [255, 165, 0]),
    ("purple", [128, 0, 128]),
    ("pink", [255, 192, 203]),
    ("gray", [128, 128, 128]),
    ("grey", [128, 128, 128]),
    ("silver", [192, 192, 192]),
    ("lime", [0, 255, 0]),
    ("navy", [0, 0, 128]),
    ("teal", [0, 128, 128]),
    ("indigo", [75, 0, 130]),
    ("violet", [238, 130, 238]),
    ("gold", [255, 215, 0]),
    ("crimson", [220, 20, 60]),
];

fn hex_digit(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

fn clamp8(v: f32) -> u8 {
    roundf(v.clamp(0.0, 255.0)) as u8
}

/// A number with an optional `%` (scaled to `full` when it has one) or the keyword `none`.
fn number(t: &str, full: f32) -> Option<f32> {
    let t = t.trim();
    if t == "none" {
        return Some(0.0);
    }
    match t.strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().ok().map(|v| v / 100.0 * full),
        None => t.parse::<f32>().ok(),
    }
}

/// An angle in degrees (`deg`, `turn`, `rad` or a bare number).
fn angle(t: &str) -> Option<f32> {
    let t = t.trim();
    if let Some(v) = t.strip_suffix("deg") {
        v.trim().parse().ok()
    } else if let Some(v) = t.strip_suffix("turn") {
        v.trim().parse::<f32>().ok().map(|x| x * 360.0)
    } else if let Some(v) = t.strip_suffix("rad") {
        v.trim().parse::<f32>().ok().map(|x| x * 180.0 / core::f32::consts::PI)
    } else {
        number(t, 360.0)
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let h = ((h % 360.0) + 360.0) % 360.0 / 60.0;
    let c = (1.0 - fabsf(2.0 * l - 1.0)) * s;
    let x = c * (1.0 - fabsf(h % 2.0 - 1.0));
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [(r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0]
}

fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * powf(v, 1.0 / 2.4) - 0.055 }
}

/// OKLCH to sRGB (0..255, clipped).
fn oklch_to_rgb(l: f32, c: f32, h_deg: f32) -> [f32; 3] {
    let h = h_deg.to_radians();
    let (a, b) = (c * cosf(h), c * sinf(h));
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_4 * s3;
    let bl = -0.004_196_086 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    [srgb_encode(r) * 255.0, srgb_encode(g) * 255.0, srgb_encode(bl) * 255.0]
}

/// Split the arguments of a CSS function: commas, or spaces with an optional `/ alpha`.
fn args(inner: &str) -> alloc::vec::Vec<String> {
    let cleaned = inner.replace('/', " / ").replace(',', " ");
    cleaned.split_whitespace().filter(|s| *s != "/").map(String::from).collect()
}

/// A CSS colour value, or `None` when it is not one this reads.
pub fn parse_color(value: &str) -> Option<Rgba> {
    let v = value.trim().trim_end_matches("!important").trim();
    let lower = v.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix('#') {
        let d: Option<alloc::vec::Vec<u8>> = hex.bytes().map(hex_digit).collect();
        let d = d?;
        return match d.len() {
            3 | 4 => {
                let e = |i: usize| d[i] * 17;
                Some(Rgba::new(e(0), e(1), e(2), if d.len() == 4 { e(3) } else { 255 }))
            }
            6 | 8 => {
                let e = |i: usize| d[i] * 16 + d[i + 1];
                Some(Rgba::new(e(0), e(2), e(4), if d.len() == 8 { e(6) } else { 255 }))
            }
            _ => None,
        };
    }
    if lower == "transparent" {
        return Some(Rgba::new(0, 0, 0, 0));
    }
    if let Some((name, rgb)) = NAMES.iter().find(|(n, _)| *n == lower) {
        let _ = name;
        return Some(Rgba::new(rgb[0], rgb[1], rgb[2], 255));
    }
    let open = lower.find('(')?;
    let close = lower.rfind(')')?;
    let (func, inner) = (&lower[..open], &lower[open + 1..close]);
    let a = args(inner);
    let alpha = |i: usize| -> Option<u8> {
        match a.get(i) {
            Some(t) => Some(clamp8(number(t, 1.0)? * 255.0)),
            None => Some(255),
        }
    };
    match func {
        "rgb" | "rgba" => {
            if a.len() < 3 {
                return None;
            }
            let c = |i: usize| number(&a[i], 255.0).map(clamp8);
            Some(Rgba::new(c(0)?, c(1)?, c(2)?, alpha(3)?))
        }
        "hsl" | "hsla" => {
            if a.len() < 3 {
                return None;
            }
            let rgb = hsl_to_rgb(
                angle(&a[0])?,
                number(&a[1], 1.0)?.clamp(0.0, 1.0),
                number(&a[2], 1.0)?.clamp(0.0, 1.0),
            );
            Some(Rgba::new(clamp8(rgb[0]), clamp8(rgb[1]), clamp8(rgb[2]), alpha(3)?))
        }
        "oklch" => {
            if a.len() < 3 {
                return None;
            }
            let rgb = oklch_to_rgb(number(&a[0], 1.0)?, number(&a[1], 0.4)?, angle(&a[2])?);
            Some(Rgba::new(clamp8(rgb[0]), clamp8(rgb[1]), clamp8(rgb[2]), alpha(3)?))
        }
        "oklab" => {
            if a.len() < 3 {
                return None;
            }
            let (l, aa, bb) = (number(&a[0], 1.0)?, number(&a[1], 0.4)?, number(&a[2], 0.4)?);
            let c = libm::sqrtf(aa * aa + bb * bb);
            let h = libm::atan2f(bb, aa).to_degrees();
            let rgb = oklch_to_rgb(l, c, h);
            Some(Rgba::new(clamp8(rgb[0]), clamp8(rgb[1]), clamp8(rgb[2]), alpha(3)?))
        }
        _ => None,
    }
}

/// A bare triple as shadcn-style systems write for `hsl(var(--x))`: `222.2 84% 4.9%` (hue, saturation, lightness).
pub fn parse_hsl_triple(value: &str) -> Option<Rgba> {
    let a = args(value.trim());
    if a.len() != 3 || !a[1].ends_with('%') || !a[2].ends_with('%') {
        return None;
    }
    let rgb = hsl_to_rgb(
        a[0].parse().ok()?,
        number(&a[1], 1.0)?.clamp(0.0, 1.0),
        number(&a[2], 1.0)?.clamp(0.0, 1.0),
    );
    Some(Rgba::new(clamp8(rgb[0]), clamp8(rgb[1]), clamp8(rgb[2]), 255))
}

/// `#rrggbb` (or `#rrggbbaa` when not opaque).
pub fn to_hex(c: Rgba) -> String {
    if c.a == 255 {
        alloc::format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        alloc::format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    }
}

/// WCAG relative luminance (0 black .. 1 white), ignoring alpha.
pub fn luminance(c: Rgba) -> f32 {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.039_28 { s / 12.92 } else { powf((s + 0.055) / 1.055, 2.4) }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast ratio of two colours (1 .. 21).
pub fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// `a` moved `k` of the way towards `b` (alpha follows `a`).
pub fn mix(a: Rgba, b: Rgba, k: f32) -> Rgba {
    let m = |x: u8, y: u8| clamp8(x as f32 + (y as f32 - x as f32) * k);
    Rgba::new(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b), a.a)
}

/// A rough lightness (perceptual, OKLab L) to tell pale colours from dark ones.
pub fn lightness(c: Rgba) -> f32 {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.040_45 { s / 12.92 } else { powf((s + 0.055) / 1.055, 2.4) }
    };
    let (r, g, b) = (lin(c.r), lin(c.g), lin(c.b));
    let l = cbrtf(0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b);
    let m = cbrtf(0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b);
    let s = cbrtf(0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b);
    0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(v: &str) -> Rgba {
        parse_color(v).unwrap_or_else(|| panic!("{v}"))
    }

    #[test]
    fn hex_forms() {
        assert_eq!(c("#ff2bd6"), Rgba::rgb(255, 43, 214));
        assert_eq!(c("#F2B"), Rgba::rgb(255, 34, 187));
        assert_eq!(c("#ff2bd680"), Rgba::new(255, 43, 214, 128));
        assert_eq!(c("#0008"), Rgba::new(0, 0, 0, 136));
        assert!(parse_color("#12345").is_none() && parse_color("#gg0000").is_none());
    }

    #[test]
    fn functions_in_old_and_new_syntax() {
        assert_eq!(c("rgb(255, 43, 214)"), Rgba::rgb(255, 43, 214));
        assert_eq!(c("rgb(255 43 214 / 50%)"), Rgba::new(255, 43, 214, 128));
        assert_eq!(c("rgba(255,255,255,.08)"), Rgba::new(255, 255, 255, 20));
        assert_eq!(c("hsl(0 100% 50%)"), Rgba::rgb(255, 0, 0));
        assert_eq!(c("hsl(120deg, 100%, 25%)"), Rgba::rgb(0, 128, 0));
        assert_eq!(c("white"), Rgba::rgb(255, 255, 255));
        assert_eq!(c("transparent").a, 0);
        assert_eq!(parse_hsl_triple("0 100% 50%"), Some(Rgba::rgb(255, 0, 0)));
        assert!(parse_hsl_triple("12px 4px").is_none());
    }

    #[test]
    fn oklch_matches_known_colours() {
        // oklch(0.628 0.2577 29.23) is sRGB red; white and black are exact at the ends.
        let red = c("oklch(0.628 0.2577 29.23)");
        assert!(red.r > 250 && red.g < 8 && red.b < 8, "{red:?}");
        assert_eq!(c("oklch(1 0 0)"), Rgba::rgb(255, 255, 255));
        assert_eq!(c("oklch(0 0 0)"), Rgba::rgb(0, 0, 0));
        let mid = c("oklch(50% 0 0)");
        assert!(mid.r == mid.g && mid.g == mid.b && (90..130).contains(&mid.r), "{mid:?}");
    }

    #[test]
    fn contrast_follows_wcag() {
        let (w, b) = (Rgba::rgb(255, 255, 255), Rgba::rgb(0, 0, 0));
        assert!((contrast(w, b) - 21.0).abs() < 0.01);
        assert!((contrast(w, w) - 1.0).abs() < 0.001);
        // The known pair: #777 on white is just under 4.5.
        assert!((4.4..4.6).contains(&contrast(Rgba::rgb(0x77, 0x77, 0x77), w)));
        assert!(luminance(b) < 0.001 && luminance(w) > 0.99);
        assert!(lightness(w) > 0.99 && lightness(b) < 0.01);
        assert_eq!(to_hex(Rgba::new(255, 43, 214, 255)), "#ff2bd6");
        assert_eq!(to_hex(Rgba::new(255, 43, 214, 20)), "#ff2bd614");
        assert_eq!(mix(b, w, 0.5), Rgba::rgb(128, 128, 128));
    }
}
