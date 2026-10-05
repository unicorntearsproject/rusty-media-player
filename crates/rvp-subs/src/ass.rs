//! ASS / SSA (Advanced SubStation Alpha): script header, styles, dialogue events and the override tags that
//! matter for reading dialogue: bold, italic, underline, strike-out, colour and alpha, font size, alignment
//! (`\an`, `\a`), position (`\pos`, the start of `\move`), line breaks and `\r`. Karaoke (`\k`), animation (`\t`,
//! `\fad`, `\move` after its start), clips, drawings (`\p`, they are dropped), blur, rotation and the other
//! effects are accepted and ignored.
//!
//! Colours are `0xRRGGBBAA` (alpha 255 = opaque; ASS counts transparency, so `&H00` is opaque).
use crate::{Cue, MAX_CUE_TEXT, Rich, Span};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_core::Timestamp;

/// Largest `PlayResX`/`PlayResY` taken (real scripts use a few hundred to a few thousand).
const MAX_PLAY_RES: i32 = 100_000;

/// A named style of the script.
#[derive(Debug, Clone, PartialEq)]
pub struct AssStyle {
    /// Name, matched case-sensitively by events (`Default` when a script has none).
    pub name: String,
    /// Font size in script units (`PlayResY` is the full picture height).
    pub size: f32,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underline.
    pub underline: bool,
    /// Strike-out.
    pub strike: bool,
    /// Primary (text) colour, `0xRRGGBBAA`.
    pub primary: u32,
    /// Outline colour, `0xRRGGBBAA`.
    pub outline: u32,
    /// Alignment, numpad layout: 1-3 bottom, 4-6 middle, 7-9 top (left, centre, right).
    pub align: u8,
    /// Margins in script units.
    pub margin_l: i32,
    /// See `margin_l`.
    pub margin_r: i32,
    /// See `margin_l`.
    pub margin_v: i32,
}

impl Default for AssStyle {
    fn default() -> Self {
        Self {
            name: String::from("Default"),
            size: 20.0,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            primary: 0xFFFF_FFFF,
            outline: 0x0000_00FF,
            align: 2,
            margin_l: 10,
            margin_r: 10,
            margin_v: 10,
        }
    }
}

/// The header of an ASS script: the picture size it was written for, the styles and the order of the event fields.
#[derive(Debug, Clone, PartialEq)]
pub struct AssScript {
    /// `PlayResX` and `PlayResY` (384x288 when the script does not say, as renderers assume).
    pub play_res: (i32, i32),
    /// Styles in file order; the first is the fallback for an unknown style name.
    pub styles: Vec<AssStyle>,
    /// `WrapStyle: 2` (only `\N` breaks lines; with 0 and 1 `\n` is a space).
    pub hard_wrap: bool,
    /// Column names of the `Dialogue:` lines in a file (`Format:` of `[Events]`), lower case.
    events_format: Vec<String>,
}

impl Default for AssScript {
    fn default() -> Self {
        Self {
            play_res: (384, 288),
            styles: alloc::vec![AssStyle::default()],
            hard_wrap: false,
            events_format: Vec::new(),
        }
    }
}

/// Parse an ASS colour: `&HAABBGGRR`, `&HBBGGRR&` or a decimal number, into `0xRRGGBBAA` (opaque unless the value
/// carries an alpha byte).
fn parse_colour(s: &str) -> Option<u32> {
    let s = s.trim();
    let v = if let Some(h) = s.strip_prefix("&H").or_else(|| s.strip_prefix("&h")) {
        u32::from_str_radix(h.trim_end_matches('&'), 16).ok()?
    } else if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("$")) {
        u32::from_str_radix(h, 16).ok()?
    } else {
        // Old scripts write decimal, possibly negative (a signed 32-bit value).
        let n: i64 = s.parse().ok()?;
        n as u32
    };
    let (r, g, b, a) = (v & 255, (v >> 8) & 255, (v >> 16) & 255, 255 - ((v >> 24) & 255));
    Some(r << 24 | g << 16 | b << 8 | a)
}

fn parse_num(s: &str) -> Option<f32> {
    let s = s.trim();
    // Hand-rolled: `str::parse::<f32>` is fine in no_std, but accept things like "20." and "+3" leniently.
    s.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn parse_int(s: &str) -> Option<i32> {
    let s = s.trim();
    s.parse::<i32>().ok().or_else(|| parse_num(s).map(|f| f as i32))
}

/// ASS `Bold` values: -1 or any non-zero is on.
fn parse_flag(s: &str) -> bool {
    parse_int(s).is_some_and(|v| v != 0)
}

/// SSA (V4) alignment (1-3 bottom, 5-7 top, 9-11 middle, in a line of three) to the numpad layout.
fn ssa_align(a: i32) -> u8 {
    let col = match a & 3 {
        1 => 0,
        2 => 1,
        3 => 2,
        _ => 1,
    };
    let row = match a {
        5..=7 => 2,
        9..=11 => 1,
        _ => 0,
    };
    (row * 3 + col + 1) as u8
}

fn ass_align(a: i32) -> u8 {
    if (1..=9).contains(&a) { a as u8 } else { 2 }
}

impl AssScript {
    /// Parse the header part of a script (`[Script Info]`, `[V4+ Styles]`/`[V4 Styles]`; the `[Events]` format line is
    /// remembered). This is what Matroska keeps in the track's `CodecPrivate`; a whole file works as well.
    pub fn parse_header(text: &str) -> Self {
        let mut script = Self { styles: Vec::new(), play_res: (0, 0), ..Self::default() };
        let mut section = "";
        let mut style_format: Vec<String> = Vec::new();
        let mut v4 = false;
        for line in text.trim_start_matches('\u{feff}').lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('!') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = match line.to_ascii_lowercase().as_str() {
                    "[script info]" => "info",
                    "[v4+ styles]" | "[v4 styles]" | "[v4++ styles]" => {
                        v4 = line.eq_ignore_ascii_case("[v4 styles]");
                        "styles"
                    }
                    "[events]" => "events",
                    _ => "other",
                };
                continue;
            }
            let Some((key, value)) = line.split_once(':') else { continue };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
            match section {
                "info" => match key.as_str() {
                    "playresx" => script.play_res.0 = parse_int(value).unwrap_or(0),
                    "playresy" => script.play_res.1 = parse_int(value).unwrap_or(0),
                    "wrapstyle" => script.hard_wrap = value.trim() == "2",
                    _ => {}
                },
                "styles" if key == "format" => {
                    style_format = value.split(',').map(|f| f.trim().to_ascii_lowercase()).collect();
                }
                "styles" if key == "style" && script.styles.len() < 512 => {
                    let cols: Vec<&str> = value.split(',').collect();
                    script.styles.push(parse_style(&style_format, &cols, v4));
                }
                "events" if key == "format" => {
                    script.events_format = value.split(',').map(|f| f.trim().to_ascii_lowercase()).collect();
                }
                _ => {}
            }
        }
        // Players take the missing size from the other axis' 4:3 default.
        // (Bounded: a script's numbers are whatever its author typed, and later products of them must not overflow.)
        let (x, y) = (script.play_res.0.clamp(0, MAX_PLAY_RES), script.play_res.1.clamp(0, MAX_PLAY_RES));
        script.play_res = match (x > 0, y > 0) {
            (true, true) => (x, y),
            (true, false) => (x, (x * 3 / 4).max(1)),
            (false, true) => ((y * 4 / 3).max(1), y),
            (false, false) => (384, 288),
        };
        if script.styles.is_empty() {
            script.styles.push(AssStyle::default());
        }
        script
    }

    fn style(&self, name: &str) -> &AssStyle {
        let name = name.trim().trim_start_matches('*');
        self.styles.iter().find(|s| s.name == name).unwrap_or(&self.styles[0])
    }

    /// The cue for one Matroska ASS block: `ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text`, shown
    /// from `start` for `duration`. `None` for a block without visible text.
    pub fn cue_from_block(&self, data: &[u8], start: Timestamp, duration: Timestamp) -> Option<Cue> {
        let s = core::str::from_utf8(data).ok()?;
        let mut f = s.splitn(9, ',');
        let _read_order = f.next()?;
        let layer = f.next()?;
        let style = f.next()?;
        let _name = f.next()?;
        let (ml, mr, mv) = (f.next()?, f.next()?, f.next()?);
        let _effect = f.next()?;
        let text = f.next()?;
        self.build(start, start.saturating_add(duration.max(0)), layer, style, [ml, mr, mv], text)
    }

    /// The cues of a whole `.ass`/`.ssa` file.
    pub fn parse_file(input: &str) -> Vec<Cue> {
        let script = Self::parse_header(input);
        // Default column order of both dialects when a script has no `Format:` line of its own.
        let default_fmt: Vec<String> =
            ["layer", "start", "end", "style", "name", "marginl", "marginr", "marginv", "effect", "text"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let fmt = if script.events_format.is_empty() { &default_fmt } else { &script.events_format };
        let col = |n: &str| fmt.iter().position(|c| c == n);
        let (c_start, c_end, c_text) = (col("start"), col("end"), col("text"));
        let (Some(c_start), Some(c_end), Some(c_text)) = (c_start, c_end, c_text) else { return Vec::new() };
        let mut cues = Vec::new();
        let mut in_events = false;
        for line in input.trim_start_matches('\u{feff}').lines() {
            let line = line.trim_start();
            if line.starts_with('[') {
                in_events = line.trim_end().eq_ignore_ascii_case("[events]");
                continue;
            }
            if !in_events || cues.len() >= crate::MAX_CUES {
                continue;
            }
            let Some(rest) = line.strip_prefix("Dialogue:").or_else(|| line.strip_prefix("dialogue:")) else {
                continue;
            };
            // The text is the last column and may hold commas.
            let cols: Vec<&str> = rest.trim_start().splitn(fmt.len(), ',').collect();
            if cols.len() < fmt.len() {
                continue;
            }
            let (Some(start), Some(end)) =
                (crate::parse_timestamp(cols[c_start]), crate::parse_timestamp(cols[c_end]))
            else {
                continue;
            };
            if end < start {
                continue;
            }
            let get = |n: &str| col(n).map_or("", |i| cols[i]);
            let margins = [get("marginl"), get("marginr"), get("marginv")];
            if let Some(c) = script.build(start, end, get("layer"), get("style"), margins, cols[c_text]) {
                cues.push(c);
            }
        }
        cues
    }

    fn build(
        &self,
        start: Timestamp,
        end: Timestamp,
        layer: &str,
        style: &str,
        margins: [&str; 3],
        text: &str,
    ) -> Option<Cue> {
        let base = self.style(style);
        let text = if text.len() > MAX_CUE_TEXT {
            let mut e = MAX_CUE_TEXT;
            while !text.is_char_boundary(e) {
                e -= 1;
            }
            &text[..e]
        } else {
            text
        };
        let margin = |s: &str, d: i32| match parse_int(s) {
            Some(v) if v > 0 => v,
            _ => d,
        };
        let mut rich = Rich {
            lines: Vec::new(),
            align: base.align,
            pos: None,
            play_res: self.play_res,
            margin_l: margin(margins[0], base.margin_l),
            margin_r: margin(margins[1], base.margin_r),
            margin_v: margin(margins[2], base.margin_v),
            layer: parse_int(layer).unwrap_or(0),
            outline: base.outline,
        };
        let mut st = Run::from_style(base, self.play_res.1);
        let mut line: Vec<Span> = Vec::new();
        let mut drawing = false;
        let mut buf = String::new();
        let flush = |buf: &mut String, st: &Run, line: &mut Vec<Span>| {
            if buf.is_empty() {
                return;
            }
            let t = core::mem::take(buf);
            match line.last_mut() {
                Some(l) if l.same_look(st) => l.text.push_str(&t),
                _ => line.push(st.span(t)),
            }
        };
        let mut chars = text.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            match c {
                '{' => {
                    if let Some(close) = text[i..].find('}') {
                        flush(&mut buf, &st, &mut line);
                        let block = &text[i + 1..i + close];
                        self.apply_tags(block, base, &mut st, &mut rich, &mut drawing);
                        // Skip to after the brace.
                        while chars.peek().is_some_and(|&(j, _)| j <= i + close) {
                            chars.next();
                        }
                    } else {
                        buf.push(c);
                    }
                }
                '\\' => match chars.peek().map(|&(_, n)| n) {
                    Some('N') => {
                        chars.next();
                        flush(&mut buf, &st, &mut line);
                        rich.lines.push(core::mem::take(&mut line));
                    }
                    Some('n') => {
                        chars.next();
                        if self.hard_wrap {
                            flush(&mut buf, &st, &mut line);
                            rich.lines.push(core::mem::take(&mut line));
                        } else if !drawing {
                            buf.push(' ');
                        }
                    }
                    Some('h') => {
                        chars.next();
                        if !drawing {
                            buf.push('\u{a0}');
                        }
                    }
                    _ => {
                        if !drawing {
                            buf.push(c);
                        }
                    }
                },
                _ if drawing => {}
                '\r' => {}
                c => buf.push(c),
            }
        }
        flush(&mut buf, &st, &mut line);
        rich.lines.push(line);
        // Trim: spans that are only spaces at the ends of a line, empty lines at the ends.
        for l in &mut rich.lines {
            if let Some(f) = l.first_mut() {
                let t = f.text.trim_start_matches(' ').len();
                f.text.drain(..f.text.len() - t);
            }
            if let Some(f) = l.last_mut() {
                let t = f.text.trim_end_matches(' ').len();
                f.text.truncate(t);
            }
            l.retain(|s| !s.text.is_empty());
        }
        while rich.lines.last().is_some_and(Vec::is_empty) {
            rich.lines.pop();
        }
        while rich.lines.first().is_some_and(Vec::is_empty) {
            rich.lines.remove(0);
        }
        if rich.lines.is_empty() {
            return None;
        }
        let plain: Vec<String> =
            rich.lines.iter().map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>()).collect();
        let mut cue = Cue::new(start, end, plain.join("\n"));
        // Nothing but the default look and place: keep it plain, so it renders like any other text track.
        cue.rich = Some(rich);
        Some(cue)
    }

    fn apply_tags(&self, block: &str, base: &AssStyle, st: &mut Run, rich: &mut Rich, drawing: &mut bool) {
        // Split on backslashes outside parentheses.
        let mut depth = 0i32;
        let mut start = 0usize;
        let bytes = block.as_bytes();
        let mut tags: Vec<&str> = Vec::new();
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'(' => depth += 1,
                b')' => depth = (depth - 1).max(0),
                b'\\' if depth == 0 => {
                    tags.push(&block[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        tags.push(&block[start..]);
        for tag in tags.into_iter().skip(1) {
            let tag = tag.trim();
            // `\t(...)` and friends: parameters follow in parentheses; the longest names first.
            let lower = tag.to_ascii_lowercase();
            let arg = |name: &str| -> Option<&str> { lower.strip_prefix(name).map(|_| &tag[name.len()..]) };
            if lower.starts_with("pos(") || lower.starts_with("move(") {
                let open = tag.find('(').unwrap_or(0);
                let inner = tag[open + 1..].trim_end_matches(')');
                let mut it = inner.split(',').map(parse_num);
                if let (Some(Some(x)), Some(Some(y))) = (it.next(), it.next()) {
                    rich.pos = Some((x as i32, y as i32));
                }
            } else if lower.starts_with("t(")
                || lower.starts_with("fad(")
                || lower.starts_with("fade(")
                || lower.starts_with("clip(")
                || lower.starts_with("iclip(")
                || lower.starts_with("org(")
                || lower.starts_with("fn")
                || lower.starts_with("fsc")
                || lower.starts_with("fsp")
                || lower.starts_with("fr")
                || lower.starts_with("fa")
                || lower.starts_with("fe")
            {
                // Ignored: animation, clips, fonts by name, scaling, spacing, rotation, shear, encoding.
            } else if let Some(v) = arg("an") {
                if let Some(a) = parse_int(v) {
                    rich.align = ass_align(a);
                }
            } else if let Some(v) = arg("alpha").or_else(|| arg("1a")) {
                if let Some(a) = parse_alpha(v) {
                    st.colour = (st.colour & !0xFF) | a;
                }
            } else if let Some(v) = arg("1c").or_else(|| arg("c")) {
                if v.trim().is_empty() {
                    st.colour = base.primary;
                } else if let Some(c) = parse_colour(v) {
                    // `\c&HBBGGRR&` has no alpha: keep the current one.
                    st.colour = (c & !0xFF) | (st.colour & 0xFF);
                }
            } else if lower.starts_with("2c")
                || lower.starts_with("3c")
                || lower.starts_with("4c")
                || lower.starts_with("2a")
                || lower.starts_with("3a")
                || lower.starts_with("4a")
            {
                // Secondary, outline and shadow colours and alphas.
            } else if let Some(v) = arg("fs") {
                let v = v.trim();
                st.size = if v.is_empty() {
                    base.size
                } else {
                    parse_num(v).filter(|s| *s > 0.0).unwrap_or(st.size)
                }
                .min(1000.0);
            } else if let Some(v) = arg("a") {
                if let Some(a) = parse_int(v) {
                    rich.align = ssa_align(a);
                }
            } else if let Some(v) = arg("b") {
                if !lower.starts_with("bord") && !lower.starts_with("be") && !lower.starts_with("blur") {
                    st.bold =
                        if v.trim().is_empty() { base.bold } else { parse_int(v).is_some_and(|n| n != 0) };
                }
            } else if let Some(v) = arg("i") {
                if !lower.starts_with("iclip") {
                    st.italic = if v.trim().is_empty() { base.italic } else { parse_flag(v) };
                }
            } else if let Some(v) = arg("u") {
                st.underline = if v.trim().is_empty() { base.underline } else { parse_flag(v) };
            } else if let Some(v) = arg("s") {
                if !lower.starts_with("shad") {
                    st.strike = if v.trim().is_empty() { base.strike } else { parse_flag(v) };
                }
            } else if let Some(v) = arg("p") {
                // `\p0` ends drawing mode; `\pbo` and `\pos` are not this tag.
                if !lower.starts_with("pbo") {
                    *drawing = parse_int(v).is_some_and(|n| n > 0);
                }
            } else if lower.starts_with('r') {
                // Reset to the line's style (or the named one).
                let name = &tag[1..];
                let s = if name.trim().is_empty() { base } else { self.style(name) };
                *st = Run::from_style(s, self.play_res.1);
            }
            // Everything else (\k, \kf, \ko, \K, \bord, \shad, \blur, \q, ...): ignored.
        }
    }
}

/// `\alpha&H80&`: ASS alpha 0 = opaque, 255 = transparent. Returns the opaque-is-255 byte.
fn parse_alpha(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches("&H").trim_start_matches("&h").trim_end_matches('&');
    let v = u32::from_str_radix(s, 16).ok()? & 255;
    Some(255 - v)
}

fn parse_style(format: &[String], cols: &[&str], v4: bool) -> AssStyle {
    let mut s = AssStyle::default();
    let get = |name: &str| format.iter().position(|f| f == name).and_then(|i| cols.get(i)).map(|v| v.trim());
    if let Some(n) = get("name") {
        s.name = n.to_string();
    }
    if let Some(v) = get("fontsize").and_then(parse_num) {
        s.size = v.clamp(1.0, 1000.0);
    }
    if let Some(v) = get("bold") {
        s.bold = parse_flag(v);
    }
    if let Some(v) = get("italic") {
        s.italic = parse_flag(v);
    }
    if let Some(v) = get("underline") {
        s.underline = parse_flag(v);
    }
    if let Some(v) = get("strikeout") {
        s.strike = parse_flag(v);
    }
    if let Some(c) = get("primarycolour").and_then(parse_colour) {
        s.primary = c;
    }
    if let Some(c) = get("outlinecolour").or_else(|| get("tertiarycolour")).and_then(parse_colour) {
        s.outline = c;
    }
    if let Some(a) = get("alignment").and_then(parse_int) {
        s.align = if v4 { ssa_align(a) } else { ass_align(a) };
    }
    if let Some(v) = get("marginl").and_then(parse_int) {
        s.margin_l = v;
    }
    if let Some(v) = get("marginr").and_then(parse_int) {
        s.margin_r = v;
    }
    if let Some(v) = get("marginv").and_then(parse_int) {
        s.margin_v = v;
    }
    s
}

/// The look of the text being read.
#[derive(Clone)]
struct Run {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    colour: u32,
    size: f32,
    play_res_y: i32,
}

impl Run {
    fn from_style(s: &AssStyle, play_res_y: i32) -> Self {
        Self {
            bold: s.bold,
            italic: s.italic,
            underline: s.underline,
            strike: s.strike,
            colour: s.primary,
            size: s.size,
            play_res_y,
        }
    }

    fn span(&self, text: String) -> Span {
        Span {
            text,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            colour: self.colour,
            size_permille: (self.size * 1000.0 / self.play_res_y.max(1) as f32 + 0.5) as i32,
        }
    }
}

impl Span {
    fn same_look(&self, r: &Run) -> bool {
        let o = r.span(String::new());
        self.bold == o.bold
            && self.italic == o.italic
            && self.underline == o.underline
            && self.strike == o.strike
            && self.colour == o.colour
            && self.size_permille == o.size_permille
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 640\nPlayResY: 360\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,36,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,0,2,10,10,20,1\nStyle: Sign,Arial,28,&H0000FFFF,&H000000FF,&H00000000,&H00000000,-1,-1,0,0,100,100,0,0,1,2,0,8,10,10,30,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:02.50,Default,,0,0,0,,Hello, {\\i1}world{\\i0}!\nDialogue: 1,0:00:03.00,0:00:04.00,Sign,,0,0,0,,{\\an9\\pos(600,40)\\c&H0000FF&}Top right\\Nsecond\nComment: 0,0:00:05.00,0:00:06.00,Default,,0,0,0,,hidden\nDialogue: 0,0:00:07.00,0:00:08.00,Default,,0,0,0,,{\\p1}m 0 0 l 100 0 100 100{\\p0}Visible\n";

    #[test]
    fn header_styles_and_play_res() {
        let s = AssScript::parse_header(SCRIPT);
        assert_eq!(s.play_res, (640, 360));
        assert_eq!(s.styles.len(), 2);
        assert_eq!(s.styles[0].size, 36.0);
        assert_eq!(s.styles[0].primary, 0xFFFF_FFFF);
        assert_eq!(s.styles[1].primary, 0xFFFF_00FF, "BGR order: &H00FFFF is yellow");
        assert!(s.styles[1].bold && s.styles[1].italic);
        assert_eq!(s.styles[1].align, 8);
    }

    #[test]
    fn file_dialogue_timing_text_and_styles() {
        let cues = AssScript::parse_file(SCRIPT);
        assert_eq!(cues.len(), 3, "comments are skipped");
        let c = &cues[0];
        assert_eq!((c.start, c.end), (1_000_000, 2_500_000));
        assert_eq!(c.text, "Hello, world!");
        let r = c.rich.as_ref().unwrap();
        assert_eq!(r.lines.len(), 1);
        let spans = &r.lines[0];
        assert_eq!(spans.len(), 3);
        assert!(!spans[0].italic && spans[1].italic && !spans[2].italic);
        assert_eq!(spans[1].text, "world");
        assert_eq!(r.align, 2);
        assert_eq!(r.margin_v, 20);
        // 36 of 360 lines of the picture.
        assert_eq!(spans[0].size_permille, 100);

        let c = &cues[1];
        assert_eq!(c.text, "Top right\nsecond");
        let r = c.rich.as_ref().unwrap();
        assert_eq!(r.align, 9);
        assert_eq!(r.pos, Some((600, 40)));
        assert_eq!(r.layer, 1);
        assert_eq!(r.lines.len(), 2);
        assert_eq!(r.lines[0][0].colour, 0xFF00_00FF, "\\c&H0000FF& is red (BGR)");
        assert!(r.lines[0][0].bold, "the style is bold");

        // Drawing mode text is not shown.
        assert_eq!(cues[2].text, "Visible");
    }

    #[test]
    fn matroska_block_and_overrides() {
        let s = AssScript::parse_header(SCRIPT);
        let c = s
            .cue_from_block(
                b"12,0,Default,,0,0,0,,{\\b1\\c&HFF0000&}Bold blue{\\r} plain\\Nline two",
                5_000_000,
                2_000_000,
            )
            .unwrap();
        assert_eq!((c.start, c.end), (5_000_000, 7_000_000));
        assert_eq!(c.text, "Bold blue plain\nline two");
        let r = c.rich.unwrap();
        assert!(r.lines[0][0].bold);
        assert_eq!(r.lines[0][0].colour, 0x0000_FFFF);
        assert!(!r.lines[0][1].bold, "\\r goes back to the style");
        assert_eq!(r.lines[0][1].colour, 0xFFFF_FFFF);
        // Karaoke, animation and effects are ignored.
        let c = s
            .cue_from_block(
                b"0,0,Default,,0,0,0,,{\\k20\\fad(100,100)\\t(0,500,\\fs50)\\blur2\\bord3}Sing along",
                0,
                1_000_000,
            )
            .unwrap();
        assert_eq!(c.text, "Sing along");
        assert_eq!(c.rich.unwrap().lines[0].len(), 1);
        // No text at all: no cue.
        assert!(s.cue_from_block(b"0,0,Default,,0,0,0,,{\\an8}", 0, 1_000_000).is_none());
        assert!(s.cue_from_block(b"nonsense", 0, 1_000_000).is_none());
    }

    #[test]
    fn ssa_alignment_and_colours() {
        let ssa = "[Script Info]\nPlayResX: 320\n[V4 Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, TertiaryColour, BackColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, AlphaLevel, Encoding\nStyle: Default,Arial,20,16777215,255,0,0,-1,0,1,2,0,6,10,10,10,0,0\n[Events]\nFormat: Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: Marked=0,0:00:01.00,0:00:02.00,Default,,0,0,0,,Top text\n";
        let s = AssScript::parse_header(ssa);
        assert_eq!(s.play_res, (320, 240), "missing PlayResY follows 4:3");
        assert_eq!(s.styles[0].align, 8, "SSA 6 is top centre");
        assert_eq!(s.styles[0].primary, 0xFFFF_FFFF, "decimal colour");
        assert!(s.styles[0].bold);
        let cues = AssScript::parse_file(ssa);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "Top text");
    }

    #[test]
    fn alpha_hard_spaces_and_unknown_styles() {
        let s = AssScript::parse_header(SCRIPT);
        let c = s.cue_from_block(b"0,0,Nope,,0,0,0,,{\\alpha&H80&}A\\hB\\nC", 0, 1_000_000).unwrap();
        assert_eq!(c.text, "A\u{a0}B C");
        assert_eq!(c.rich.unwrap().lines[0][0].colour & 0xFF, 0x7F);
    }

    #[test]
    fn hostile_input_is_bounded_and_does_not_panic() {
        let s = AssScript::parse_header("[V4+ Styles]\nStyle: ,,,,\n");
        for blob in [
            &b"{"[..],
            b"{\\",
            b"{\\pos(",
            b"{\\t(((",
            b"\\",
            b"\xff\xfe,,,,,,,,",
            b"1,2,3,4,5,6,7,8,{\\fs99999999999}x",
        ] {
            let _ = s.cue_from_block(blob, 0, 1);
        }
        let _ = AssScript::parse_file(
            "[Events]\nFormat: Start, End, Text\nDialogue: 0:00:00.00,0:00:01.00\nDialogue: x,y,z\n",
        );
    }
}
