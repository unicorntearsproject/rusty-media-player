//! Generates `crates/theme/src/tokens.rs` from the design-system CSS custom properties.
//!
//! Supported value forms: `#rrggbb`, `rgb()/rgba()`, `var(--x)` aliases, `linear-gradient`,
//! `radial-gradient`, shadow lists, `rem/px/em/ms` and bare numbers, `cubic-bezier()`, and quoted font
//! stacks. Anything else is reported and skipped so a new token never silently breaks the build.
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const FILES: &[&str] = &["colors.css", "typography.css", "spacing.css"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn snapshot_dir() -> PathBuf {
    root().join("crates/theme/tokens")
}

fn output_path() -> PathBuf {
    root().join("crates/theme/src/tokens.rs")
}

pub fn run(sync: bool) -> Result<(), String> {
    if sync {
        let src = std::env::var("UT_DESIGN_SYSTEM")
            .unwrap_or_else(|_| "/home/jj/projects/unicorn-tears/claude-design-system".into());
        for f in FILES {
            let from = Path::new(&src).join("tokens").join(f);
            std::fs::copy(&from, snapshot_dir().join(f))
                .map_err(|e| format!("copy {}: {e}", from.display()))?;
        }
        println!("synced token CSS from {src}");
    }
    let out = generate(&snapshot_dir())?;
    std::fs::write(output_path(), out).map_err(|e| e.to_string())?;
    println!("wrote {}", output_path().display());
    Ok(())
}

/// Strip `/* ... */` comments.
fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i..].find("*/") {
            Some(j) => rest = &rest[i + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Collect `--name: value;` declarations in file order.
fn parse_declarations(css: &str) -> Vec<(String, String)> {
    let css = strip_comments(css);
    let mut out = Vec::new();
    for decl in css.split(';') {
        let decl = decl.trim().trim_start_matches(['{', '}']).trim();
        // A declaration may be preceded by the selector's `:root {`.
        let decl = decl.rsplit_once('{').map_or(decl, |(_, d)| d).trim();
        if let Some(rest) = decl.strip_prefix("--") {
            if let Some((name, value)) = rest.split_once(':') {
                out.push((name.trim().to_string(), value.trim().to_string()));
            }
        }
    }
    out
}

/// Split on `sep` at parenthesis depth 0 (and outside quotes).
fn split_top(s: &str, sep: char) -> Vec<String> {
    let (mut depth, mut quote) = (0i32, None::<char>);
    let (mut parts, mut cur) = (Vec::new(), String::new());
    for ch in s.chars() {
        match (ch, quote) {
            ('\'' | '"', None) => quote = Some(ch),
            (q, Some(open)) if q == open => quote = None,
            ('(', None) => depth += 1,
            (')', None) => depth -= 1,
            _ => {}
        }
        if ch == sep && depth == 0 && quote.is_none() {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    parts.push(cur);
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// Split on whitespace at depth 0.
fn split_ws(s: &str) -> Vec<String> {
    let mut depth = 0i32;
    let (mut parts, mut cur) = (Vec::new(), String::new());
    for ch in s.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if ch.is_whitespace() && depth == 0 {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

fn const_name(token: &str) -> String {
    token.replace('-', "_").to_uppercase()
}

fn num(s: &str) -> Result<f32, String> {
    s.trim().parse::<f32>().map_err(|_| format!("bad number `{s}`"))
}

/// A length in px: `1.5rem`, `14px`, `0`.
fn length(s: &str) -> Result<f32, String> {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("rem") {
        Ok(num(v)? * 16.0)
    } else if let Some(v) = s.strip_suffix("px") {
        num(v)
    } else {
        num(s)
    }
}

fn f32_lit(v: f32) -> String {
    format!("{v:?}")
}

struct Ctx<'a> {
    raw: &'a BTreeMap<String, String>,
}

/// Rust expression for a colour value.
fn color_expr(v: &str, ctx: &Ctx) -> Result<String, String> {
    let v = v.trim();
    if let Some(h) = v.strip_prefix('#') {
        let (r, g, b) = match h.len() {
            6 => (&h[0..2], &h[2..4], &h[4..6]),
            _ => return Err(format!("unsupported hex colour `{v}`")),
        };
        let p = |x: &str| u8::from_str_radix(x, 16).map_err(|_| format!("bad hex `{v}`"));
        return Ok(format!("Rgba::rgb({}, {}, {})", p(r)?, p(g)?, p(b)?));
    }
    if let Some(inner) =
        v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb(")).and_then(|s| s.strip_suffix(')'))
    {
        let p: Vec<&str> = inner.split(',').map(str::trim).collect();
        if p.len() < 3 {
            return Err(format!("bad rgb() `{v}`"));
        }
        let ch = |s: &str| num(s).map(|x| x.round().clamp(0.0, 255.0) as u8);
        let a = if p.len() > 3 { (num(p[3])? * 255.0).round().clamp(0.0, 255.0) as u8 } else { 255 };
        return Ok(format!("Rgba::new({}, {}, {}, {})", ch(p[0])?, ch(p[1])?, ch(p[2])?, a));
    }
    if let Some(name) = v.strip_prefix("var(--").and_then(|s| s.strip_suffix(')')) {
        if !ctx.raw.contains_key(name) {
            return Err(format!("unknown var `{name}`"));
        }
        return Ok(const_name(name));
    }
    Err(format!("not a colour: `{v}`"))
}

fn stops_expr(parts: &[String], ctx: &Ctx) -> Result<String, String> {
    let mut out = String::from("&[");
    for p in parts {
        let (c, pos) = p.rsplit_once(char::is_whitespace).ok_or_else(|| format!("bad stop `{p}`"))?;
        let pos = num(pos.trim().strip_suffix('%').ok_or_else(|| format!("stop without %: `{p}`"))?)? / 100.0;
        write!(out, "GradientStop {{ pos: {}, color: {} }}, ", f32_lit(pos), color_expr(c, ctx)?).unwrap();
    }
    out.push(']');
    Ok(out)
}

enum Kind {
    Color,
    Linear,
    Radial,
    Shadows,
    Px,
    Ms,
    Float,
    Bezier,
    Str,
}

impl Kind {
    fn ty(&self) -> &'static str {
        match self {
            Kind::Color => "Rgba",
            Kind::Linear => "LinearGradient",
            Kind::Radial => "RadialGradient",
            Kind::Shadows => "&[Shadow]",
            Kind::Px | Kind::Float => "f32",
            Kind::Ms => "u32",
            Kind::Bezier => "CubicBezier",
            Kind::Str => "&str",
        }
    }
}

fn classify(v: &str, ctx: &Ctx) -> Result<(Kind, String), String> {
    let v = v.trim();
    if let Some(name) = v.strip_prefix("var(--").and_then(|s| s.strip_suffix(')')) {
        let target = ctx.raw.get(name).ok_or_else(|| format!("unknown var `{name}`"))?;
        let (k, _) = classify(target, ctx)?;
        return Ok((k, const_name(name)));
    }
    if v.starts_with('#') || v.starts_with("rgb") {
        return Ok((Kind::Color, color_expr(v, ctx)?));
    }
    if let Some(inner) = v.strip_prefix("linear-gradient(").and_then(|s| s.strip_suffix(')')) {
        let parts = split_top(inner, ',');
        let angle = num(parts[0].strip_suffix("deg").ok_or("linear-gradient needs a deg angle")?)?;
        return Ok((
            Kind::Linear,
            format!(
                "LinearGradient {{ angle_deg: {}, stops: {} }}",
                f32_lit(angle),
                stops_expr(&parts[1..], ctx)?
            ),
        ));
    }
    if let Some(inner) = v.strip_prefix("radial-gradient(").and_then(|s| s.strip_suffix(')')) {
        let parts = split_top(inner, ',');
        let (size, at) =
            parts[0].split_once(" at ").ok_or("radial-gradient needs `<rx>% <ry>% at <cx>% <cy>%`")?;
        let pct = |s: &str| num(s.trim().trim_end_matches('%'));
        let s: Vec<&str> = size.split_whitespace().collect();
        let a: Vec<&str> = at.split_whitespace().collect();
        if s.len() != 2 || a.len() != 2 {
            return Err(format!("unsupported radial-gradient `{v}`"));
        }
        return Ok((
            Kind::Radial,
            format!(
                "RadialGradient {{ rx_pct: {}, ry_pct: {}, cx_pct: {}, cy_pct: {}, stops: {} }}",
                f32_lit(pct(s[0])?),
                f32_lit(pct(s[1])?),
                f32_lit(pct(a[0])?),
                f32_lit(pct(a[1])?),
                stops_expr(&parts[1..], ctx)?
            ),
        ));
    }
    if let Some(inner) = v.strip_prefix("cubic-bezier(").and_then(|s| s.strip_suffix(')')) {
        let p: Vec<f32> = inner.split(',').map(num).collect::<Result<_, _>>()?;
        if p.len() != 4 {
            return Err(format!("bad cubic-bezier `{v}`"));
        }
        return Ok((
            Kind::Bezier,
            format!(
                "CubicBezier({}, {}, {}, {})",
                f32_lit(p[0]),
                f32_lit(p[1]),
                f32_lit(p[2]),
                f32_lit(p[3])
            ),
        ));
    }
    if v.starts_with('\'') {
        return Ok((Kind::Str, format!("{:?}", v)));
    }
    // Shadow list: each layer is `[inset] <x> <y> <blur> [spread] <colour>`.
    let layers = split_top(v, ',');
    if layers.iter().all(|l| {
        let w = split_ws(l);
        w.first().is_some_and(|f| f == "inset" || f.chars().next().is_some_and(|c| c.is_ascii_digit()))
            && w.len() >= 4
    }) && layers.iter().any(|l| l.contains("rgb") || l.contains('#') || l.contains("var("))
    {
        let mut out = String::from("&[");
        for l in &layers {
            let mut w = split_ws(l);
            let inset = w.first().is_some_and(|f| f == "inset");
            if inset {
                w.remove(0);
            }
            let color = w.pop().ok_or("empty shadow")?;
            let nums: Vec<f32> = w.iter().map(|x| length(x)).collect::<Result<_, _>>()?;
            if nums.len() < 3 {
                return Err(format!("bad shadow `{l}`"));
            }
            write!(
                out,
                "Shadow {{ inset: {inset}, x: {}, y: {}, blur: {}, color: {} }}, ",
                f32_lit(nums[0]),
                f32_lit(nums[1]),
                f32_lit(nums[2]),
                color_expr(&color, ctx)?
            )
            .unwrap();
        }
        out.push(']');
        return Ok((Kind::Shadows, out));
    }
    if let Some(ms) = v.strip_suffix("ms") {
        return Ok((Kind::Ms, format!("{}", num(ms)? as u32)));
    }
    if v.ends_with("rem") || v.ends_with("px") {
        return Ok((Kind::Px, f32_lit(length(v)?)));
    }
    if let Some(em) = v.strip_suffix("em") {
        return Ok((Kind::Float, f32_lit(num(em)?)));
    }
    if let Ok(n) = v.parse::<f32>() {
        return Ok((Kind::Float, f32_lit(n)));
    }
    Err(format!("unsupported value `{v}`"))
}

pub fn generate(dir: &Path) -> Result<String, String> {
    let mut decls: Vec<(String, String)> = Vec::new();
    for f in FILES {
        let css =
            std::fs::read_to_string(dir.join(f)).map_err(|e| format!("{}: {e}", dir.join(f).display()))?;
        decls.extend(parse_declarations(&css));
    }
    let raw: BTreeMap<String, String> = decls.iter().cloned().collect();
    let ctx = Ctx { raw: &raw };
    let mut out = String::new();
    out.push_str("// @generated by `cargo xtask theme` from crates/theme/tokens/*.css. DO NOT EDIT.\n");
    out.push_str("//! Design-system tokens. Lengths in px, durations in ms, colours straight-alpha RGBA8.\n");
    out.push_str("#![allow(missing_docs)]\n\nuse crate::{CubicBezier, GradientStop, LinearGradient, RadialGradient, Rgba, Shadow};\n\n");
    let mut skipped = Vec::new();
    for (name, value) in &decls {
        match classify(value, &ctx) {
            Ok((kind, expr)) => {
                writeln!(out, "pub const {}: {} = {};", const_name(name), kind.ty(), expr).unwrap();
            }
            Err(e) => skipped.push(format!("--{name}: {e}")),
        }
    }
    if !skipped.is_empty() {
        return Err(format!("unsupported tokens (extend xtask/src/theme.rs):\n  {}", skipped.join("\n  ")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_rs_is_in_sync_with_snapshot_css() {
        let generated = generate(&snapshot_dir()).expect("generate");
        let on_disk = std::fs::read_to_string(output_path())
            .expect("crates/theme/src/tokens.rs missing; run `cargo xtask theme`");
        assert_eq!(generated, on_disk, "tokens.rs is stale; run `cargo xtask theme`");
    }

    #[test]
    fn parses_values() {
        let raw: BTreeMap<String, String> = [("ink".to_string(), "#07060d".to_string())].into();
        let ctx = Ctx { raw: &raw };
        assert_eq!(color_expr("rgba(255,43,214,.55)", &ctx).unwrap(), "Rgba::new(255, 43, 214, 140)");
        assert_eq!(classify("1.5rem", &ctx).unwrap().1, "24.0");
        assert_eq!(classify("220ms", &ctx).unwrap().1, "220");
        assert_eq!(classify("var(--ink)", &ctx).unwrap().1, "INK");
        assert!(classify("0 8px 28px rgba(5,2,15,.55)", &ctx).is_ok());
        assert!(classify("whatever(", &ctx).is_err());
    }

    #[test]
    fn split_top_respects_parens() {
        assert_eq!(split_top("a(1,2), b, c(3)", ','), ["a(1,2)", "b", "c(3)"]);
    }
}
