//! Themes from a design system's style sheet.
//!
//! A person pastes a link to a Claude Design system (or its CSS): the host fetches the style sheet (or the paste is used as it is), this
//! module reads its custom properties ([`css`]), maps them onto the colour tokens the UI draws with ([`crate::tk`]), derives what the
//! sheet does not say, **checks contrast** and repairs or falls back where a pair would be unreadable, and returns a [`Theme`] to preview,
//! apply or throw away. Radii map to a scale of the UI's corner radii; the typeface is only noted (the app keeps its bundled faces).
//!
//! Nothing here touches the screen or the network: it is text in, a theme out, so it is tested with plain strings.
pub mod color;
pub mod css;

use crate::tk::{self, Colors};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use color::{contrast, lightness, mix, parse_color, parse_hsl_triple, to_hex};
use css::{Sheet, resolve};
use theme::Rgba;

/// A theme that can be applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// What to call it (the system's name, the address it came from, or "Pasted theme").
    pub name: String,
    /// The colours.
    pub colors: Colors,
    /// Scale of the UI's corner radii: 1.0 is the Unicorn Tears look, 0 is square.
    pub radius_scale: f32,
    /// What the sheet says about type, when it names a face the app does not have.
    pub font_note: String,
    /// How many colour tokens the sheet supplied (the rest are derived).
    pub found: usize,
    /// What was changed to keep the app readable, and what could not be read.
    pub warnings: Vec<String>,
}

/// Why no theme came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeError {
    /// The sheet had no custom properties at all.
    NoVariables,
    /// It had some, but no page or text colour and no accent to build a theme on.
    NoColors,
}

impl core::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ThemeError::NoVariables => {
                f.write_str("That has no design tokens in it (no custom properties like --background).")
            }
            ThemeError::NoColors => {
                f.write_str("I found tokens, but no page, text or accent colors I can use.")
            }
        }
    }
}

impl Theme {
    /// The Unicorn Tears look the app ships with.
    pub fn unicorn_tears() -> Theme {
        Theme {
            name: "Unicorn Tears".to_string(),
            colors: Colors::DEFAULT,
            radius_scale: 1.0,
            font_note: String::new(),
            found: 0,
            warnings: Vec::new(),
        }
    }

    /// Put this theme on screen (the colours and the corner radii).
    pub fn apply(&self) {
        tk::set(&self.colors);
        crate::gfx::set_radius_scale(self.radius_scale);
    }
}

/// Back to the Unicorn Tears look.
pub fn reset() {
    Theme::unicorn_tears().apply();
}

/// The names a role may go by in a design system's tokens, best first (matched without the leading `--` and a `color-` prefix).
struct Roles;

impl Roles {
    const BG: &'static [&'static str] = &[
        "bg-page",
        "background",
        "bg",
        "page",
        "canvas",
        "base",
        "surface-0",
        "ink-900",
        "bg-base",
        "app-background",
        "body-bg",
    ];
    const SURFACE: &'static [&'static str] = &[
        "bg-surface",
        "surface",
        "card",
        "panel",
        "ink-800",
        "popover",
        "surface-1",
        "bg-card",
        "bg-panel",
        "bg-secondary",
    ];
    const RAISED: &'static [&'static str] = &[
        "bg-surface-raised",
        "surface-raised",
        "raised",
        "elevated",
        "surface-2",
        "ink-700",
        "bg-elevated",
        "bg-tertiary",
        "muted",
    ];
    const HOVER: &'static [&'static str] =
        &["bg-surface-hover", "surface-hover", "hover", "ink-600", "surface-3", "bg-hover", "accent-bg"];
    const BORDER: &'static [&'static str] =
        &["border-default", "border", "ink-500", "outline", "divider", "stroke", "input", "border-color"];
    const TEXT: &'static [&'static str] = &[
        "text-strong",
        "foreground",
        "text",
        "fg",
        "text-primary",
        "on-background",
        "color-text",
        "content",
        "ink",
        "body-color",
    ];
    const TEXT_BODY: &'static [&'static str] =
        &["text-body", "card-foreground", "text-default", "on-surface", "body-text", "text-secondary"];
    const TEXT_MUTED: &'static [&'static str] = &[
        "text-muted",
        "muted-foreground",
        "text-secondary",
        "subtle",
        "text-subtle",
        "fg-muted",
        "muted-text",
        "secondary-text",
    ];
    const TEXT_DIM: &'static [&'static str] =
        &["text-dim", "text-tertiary", "dim", "fg-dim", "text-quaternary", "caption"];
    const TEXT_DISABLED: &'static [&'static str] =
        &["text-disabled", "disabled", "placeholder", "text-placeholder", "fg-disabled"];
    const ACCENT1: &'static [&'static str] = &[
        "accent-primary",
        "primary",
        "brand",
        "brand-primary",
        "accent",
        "magenta-500",
        "primary-500",
        "color-primary",
        "main",
    ];
    const ACCENT2: &'static [&'static str] = &[
        "accent-secondary",
        "secondary-accent",
        "secondary-brand",
        "cyan-500",
        "info",
        "secondary-500",
        "highlight",
        "link",
    ];
    const ACCENT3: &'static [&'static str] =
        &["accent-tertiary", "tertiary", "violet-500", "purple-500", "tertiary-500", "accent-3"];
    const FOCUS: &'static [&'static str] = &["focus-ring", "ring", "focus", "outline-color"];
    const WARNING: &'static [&'static str] = &["warning", "warn", "caution", "amber-500", "yellow-500"];
    const DANGER: &'static [&'static str] =
        &["danger", "destructive", "error", "negative", "red-500", "critical"];
    const SUCCESS: &'static [&'static str] = &["success", "positive", "ok", "green-500", "lime-500"];
    const RADIUS: &'static [&'static str] = &[
        "radius",
        "radius-md",
        "radius-lg",
        "border-radius",
        "rounded",
        "radius-base",
        "radius-default",
        "corner-radius",
    ];
}

fn norm(name: &str) -> String {
    let n = name.trim_start_matches("--").to_ascii_lowercase();
    for p in ["color-", "colour-", "clr-", "c-"] {
        if let Some(rest) = n.strip_prefix(p) {
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    n
}

/// A CSS length in pixels (`14px`, `0.875rem`, `1em`, `0`), for the radius.
fn px(value: &str) -> Option<f32> {
    let v = value.trim();
    if v == "0" {
        return Some(0.0);
    }
    let num = |s: &str| s.trim().parse::<f32>().ok();
    if let Some(n) = v.strip_suffix("px") {
        num(n)
    } else if let Some(n) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        num(n).map(|x| x * 16.0)
    } else {
        None
    }
}

/// The first family of a `font-family` value.
fn first_family(value: &str) -> Option<String> {
    let f = value.split(',').next()?.trim().trim_matches(|c| c == '\'' || c == '"').trim();
    (!f.is_empty()).then(|| f.to_string())
}

/// Read the style sheets (the first is the main one; the others are what it imported or linked) and make a theme.
pub fn from_css(sheets: &[&str], name: &str) -> Result<Theme, ThemeError> {
    let mut sheet = Sheet::default();
    for s in sheets {
        sheet.add(s);
    }
    if sheet.is_empty() {
        return Err(ThemeError::NoVariables);
    }
    let vars = sheet.variables(true);
    let mut colors: BTreeMap<String, Rgba> = BTreeMap::new();
    let mut lengths: BTreeMap<String, f32> = BTreeMap::new();
    let mut fonts: BTreeMap<String, String> = BTreeMap::new();
    for (k, raw) in &vars {
        let Some(value) = resolve(raw, &vars) else { continue };
        let key = norm(k);
        if let Some(c) = parse_color(&value).or_else(|| parse_hsl_triple(&value)) {
            colors.insert(key, c);
        } else if let Some(p) = px(&value) {
            lengths.insert(key, p);
        } else if key.starts_with("font") && !key.contains("size") && !key.contains("weight") {
            if let Some(f) = first_family(&value) {
                fonts.insert(key, f);
            }
        }
    }
    let pick = |names: &[&str]| -> Option<Rgba> {
        names.iter().find_map(|n| colors.get(*n).copied()).map(opaque_over_black)
    };
    let (bg, text, a1) = (pick(Roles::BG), pick(Roles::TEXT), pick(Roles::ACCENT1));
    if bg.is_none() && text.is_none() && a1.is_none() {
        return Err(ThemeError::NoColors);
    }
    let mut found = 0usize;
    let mut count = |c: Option<Rgba>| {
        if c.is_some() {
            found += 1;
        }
        c
    };
    let (bg, text, a1) = (count(bg), count(text), count(a1));
    let (surface, raised, hover, border) = (
        count(pick(Roles::SURFACE)),
        count(pick(Roles::RAISED)),
        count(pick(Roles::HOVER)),
        count(pick(Roles::BORDER)),
    );
    let (body, muted, dim, disabled) = (
        count(pick(Roles::TEXT_BODY)),
        count(pick(Roles::TEXT_MUTED)),
        count(pick(Roles::TEXT_DIM)),
        count(pick(Roles::TEXT_DISABLED)),
    );
    let (a2, a3, focus) =
        (count(pick(Roles::ACCENT2)), count(pick(Roles::ACCENT3)), count(pick(Roles::FOCUS)));
    let (warning, danger, success) =
        (count(pick(Roles::WARNING)), count(pick(Roles::DANGER)), count(pick(Roles::SUCCESS)));

    let d = &Colors::DEFAULT;
    // The page: what the sheet says; or what the text colour suggests (light text, dark page); or the default.
    let bg = bg.unwrap_or_else(|| match text {
        Some(t) if lightness(t) < 0.5 => Rgba::rgb(250, 250, 252),
        _ => d.ink_900,
    });
    let dark = lightness(bg) < 0.55;
    let (toward, away) = if dark {
        (Rgba::rgb(255, 255, 255), Rgba::rgb(0, 0, 0))
    } else {
        (Rgba::rgb(0, 0, 0), Rgba::rgb(255, 255, 255))
    };
    let text = text.unwrap_or(toward);
    let ink = |k: f32| mix(bg, text, k);
    let surface = surface.unwrap_or_else(|| ink(0.05));
    let raised = raised.unwrap_or_else(|| ink(0.09));
    let hover = hover.unwrap_or_else(|| ink(0.14));
    let border = border.unwrap_or_else(|| ink(0.2));
    let muted = muted.unwrap_or_else(|| mix(text, bg, 0.30));
    let dim = dim.unwrap_or_else(|| mix(text, bg, 0.45));
    let disabled = disabled.unwrap_or_else(|| mix(text, bg, 0.62));
    let body = body.unwrap_or(text);
    // Accents: the primary one, and the others from it when the sheet has only one.
    let a1 = a1.unwrap_or(d.magenta_500);
    let a2 = a2.unwrap_or_else(|| mix(a1, text, 0.35));
    let a3 = a3.unwrap_or_else(|| mix(a1, bg, 0.25));
    let lift = |c: Rgba| mix(c, toward, 0.28);
    let deep = |c: Rgba| mix(c, away, 0.22);
    let deeper = |c: Rgba| mix(c, away, 0.40);
    let on = if dark { Rgba::rgb(255, 255, 255) } else { Rgba::rgb(12, 12, 16) };
    let mut colors_out = Colors {
        ink_900: bg,
        bg_page: bg,
        ink_850: mix(bg, surface, 0.5),
        ink_800: surface,
        ink_700: raised,
        ink_600: hover,
        ink_500: border,
        magenta_500: a1,
        magenta_400: lift(a1),
        magenta_700: deeper(a1),
        cyan_500: a2,
        cyan_400: lift(a2),
        cyan_600: deep(a2),
        violet_500: a3,
        violet_400: lift(a3),
        violet_600: deep(a3),
        lime_500: success.unwrap_or(d.lime_500),
        white: on,
        pink_white: body,
        text_strong: text,
        text_body: body,
        text_muted: muted,
        text_dim: dim,
        text_disabled: disabled,
        border_subtle: Rgba::new(on.r, on.g, on.b, 20),
        focus_ring: focus.unwrap_or(a2),
        warning: warning.unwrap_or(d.warning),
        danger: danger.unwrap_or(d.danger),
        tears: [a1, a3, a2],
        tears_v: [a2, a3, a1],
        night: [mix(bg, a3, 0.22), surface, bg],
    };
    let mut warnings = Vec::new();
    check_contrast(&mut colors_out, dark, &mut warnings);

    let radius_scale = Roles::RADIUS
        .iter()
        .find_map(|n| lengths.get(*n).copied())
        .map_or(1.0, |p| (p / 14.0).clamp(0.0, 2.0));
    let font = ["font-sans", "font-body", "font-family", "font-base", "font-ui", "font-display"]
        .iter()
        .find_map(|n| fonts.get(*n).cloned());
    let font_note = match font {
        Some(f) if !f.to_ascii_lowercase().contains("space grotesk") && !is_generic_family(&f) => {
            alloc::format!(
                "Typeface {f}: Rusty Wave keeps its own faces (Space Grotesk and JetBrains Mono); a link cannot bring a font."
            )
        }
        _ => String::new(),
    };
    Ok(Theme { name: name.to_string(), colors: colors_out, radius_scale, font_note, found, warnings })
}

fn is_generic_family(f: &str) -> bool {
    matches!(
        f.to_ascii_lowercase().as_str(),
        "system-ui" | "sans-serif" | "serif" | "monospace" | "ui-monospace" | "-apple-system"
    )
}

/// A colour with transparency drawn over black, so a translucent token (a border) does not make a text colour see-through.
fn opaque_over_black(c: Rgba) -> Rgba {
    if c.a == 255 {
        return c;
    }
    let k = c.a as f32 / 255.0;
    mix(Rgba::rgb(0, 0, 0), Rgba::rgb(c.r, c.g, c.b), k)
}

/// One readability rule: this colour on that background needs this ratio.
struct Rule {
    what: &'static str,
    min: f32,
}

/// Repair or replace the colours that would be unreadable: nudge a colour away from its background in ten steps; if that still fails,
/// the Unicorn Tears token takes its place when that passes, else plain white or black. Every change is reported.
fn check_contrast(c: &mut Colors, dark: bool, warnings: &mut Vec<String>) {
    let bg = c.ink_900;
    let surface = c.ink_800;
    // The page and the surfaces must be told apart, or every card and panel disappears.
    if contrast(bg, surface) < 1.04 && bg == surface {
        c.ink_800 = mix(bg, c.text_strong, 0.05);
        c.ink_700 = mix(bg, c.text_strong, 0.09);
        warnings
            .push("The page and surface colors were the same, so surfaces were tinted apart.".to_string());
    }
    let toward = if dark { Rgba::rgb(255, 255, 255) } else { Rgba::rgb(0, 0, 0) };
    type Get = fn(&mut Colors) -> &mut Rgba;
    let rules: [(Get, Get, Rule); 9] = [
        (|c| &mut c.text_strong, |c| &mut c.ink_900, Rule { what: "text on the page", min: 4.5 }),
        (|c| &mut c.text_body, |c| &mut c.ink_800, Rule { what: "body text on surfaces", min: 4.5 }),
        (|c| &mut c.text_muted, |c| &mut c.ink_800, Rule { what: "secondary text", min: 4.5 }),
        (|c| &mut c.text_dim, |c| &mut c.ink_800, Rule { what: "dim text", min: 3.0 }),
        (|c| &mut c.text_disabled, |c| &mut c.ink_800, Rule { what: "disabled text", min: 2.0 }),
        (|c| &mut c.magenta_500, |c| &mut c.ink_900, Rule { what: "the main accent", min: 3.0 }),
        (|c| &mut c.cyan_500, |c| &mut c.ink_800, Rule { what: "the second accent", min: 3.0 }),
        (|c| &mut c.violet_400, |c| &mut c.ink_800, Rule { what: "the third accent", min: 3.0 }),
        (|c| &mut c.focus_ring, |c| &mut c.ink_900, Rule { what: "the focus ring", min: 3.0 }),
    ];
    let defaults = Colors::DEFAULT;
    for (fg, back, rule) in rules {
        let back_colour = *back(c);
        let fg_default = *fg(&mut defaults.clone());
        let cur = *fg(c);
        if contrast(cur, back_colour) >= rule.min {
            continue;
        }
        let mut fixed = None;
        for step in 1..=10 {
            let t = mix(cur, toward, step as f32 * 0.1);
            if contrast(t, back_colour) >= rule.min {
                fixed = Some(t);
                break;
            }
        }
        let value = fixed
            .or_else(|| (contrast(fg_default, back_colour) >= rule.min).then_some(fg_default))
            .unwrap_or(toward);
        *fg(c) = value;
        warnings.push(alloc::format!("Adjusted {} to {} so it stays readable.", rule.what, to_hex(value)));
    }
    // Labels on the main accent (buttons): the on-colour against it.
    let accent = c.magenta_500;
    if contrast(c.white, accent) < 3.0 {
        let alt = if dark { Rgba::rgb(12, 12, 16) } else { Rgba::rgb(255, 255, 255) };
        if contrast(alt, accent) > contrast(c.white, accent) {
            c.white = alt;
            warnings.push(
                "Button labels switched to the opposite shade for contrast on the main accent.".to_string(),
            );
        }
    }
}

// ---- keeping a theme ----------------------------------------------------------------------------------------------------------------

/// Where the applied theme is kept.
pub const THEME_KEY: &str = "settings/theme";

impl Theme {
    /// The text to keep (the colours as the app uses them, so reading it back needs no design system).
    pub fn to_text(&self) -> String {
        let mut s = String::from("rvp-theme 1\n");
        s.push_str(&alloc::format!("name={}\n", self.name.replace(['\n', '\r'], " ")));
        s.push_str(&alloc::format!("radius={:.3}\n", self.radius_scale));
        s.push_str(&alloc::format!("note={}\n", self.font_note.replace(['\n', '\r'], " ")));
        for (k, v) in self.colors.entries() {
            s.push_str(&alloc::format!("{k}={}\n", to_hex(v)));
        }
        s
    }

    /// Read kept text; anything unreadable gives `None` (and the default look stays).
    pub fn from_text(text: &str) -> Option<Theme> {
        let mut lines = text.lines();
        if lines.next()?.trim() != "rvp-theme 1" {
            return None;
        }
        let mut t = Theme::unicorn_tears();
        t.name = String::new();
        let mut any = false;
        for l in lines {
            let Some((k, v)) = l.split_once('=') else { continue };
            match k.trim() {
                "name" => t.name = v.trim().to_string(),
                "radius" => t.radius_scale = v.trim().parse::<f32>().ok()?.clamp(0.0, 2.0),
                "note" => t.font_note = v.trim().to_string(),
                token => {
                    if let Some(c) = parse_color(v) {
                        any |= t.colors.set_named(token, c);
                    }
                }
            }
        }
        if !any || t.name.is_empty() {
            return None;
        }
        // Stored text can be edited by hand: it gets the same readability check as a theme that was just made.
        let dark = lightness(t.colors.ink_900) < 0.55;
        check_contrast(&mut t.colors, dark, &mut t.warnings);
        Some(t)
    }
}

#[cfg(test)]
mod tests;
