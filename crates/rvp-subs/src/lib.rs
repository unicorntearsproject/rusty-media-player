//! Text subtitles: SRT and WebVTT files, the packet payloads of Matroska (`S_TEXT/UTF8`, `S_TEXT/WEBVTT`) and MP4
//! (`tx3g` / `mov_text`, `wvtt`), and a [`CueList`] that answers "what is on screen at time t".
//!
//! Everything is lenient: malformed blocks are skipped, never an error. Styling is dropped (tags such as
//! `<i>`, `<c.red>`, `{\an8}` are stripped, entities decoded) and cue settings are ignored; the UI draws every
//! cue the same way.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::{string::String, vec::Vec};
use rvp_core::Timestamp;

/// Most cues a list keeps, and the longest cue text (bytes).
pub const MAX_CUES: usize = 200_000;
/// See [`MAX_CUES`].
pub const MAX_CUE_TEXT: usize = 4096;

/// One subtitle cue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    /// Start, microseconds.
    pub start: Timestamp,
    /// End, microseconds.
    pub end: Timestamp,
    /// Plain text, lines joined with `\n`.
    pub text: String,
}

/// Text subtitle formats we parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// SubRip.
    Srt,
    /// WebVTT.
    WebVtt,
}

/// Parse `HH:MM:SS,mmm` (or `.` as the separator, or `MM:SS.mmm` as WebVTT allows) to microseconds.
pub fn parse_timestamp(s: &str) -> Option<Timestamp> {
    let s = s.trim();
    let (hms, frac) = s.rsplit_once([',', '.'])?;
    if frac.is_empty() || frac.len() > 9 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Fractions are decimal digits: ".5" is 500 ms, ".05" is 50 ms.
    let mut micros: i64 = 0;
    let mut scale = 100_000i64;
    for b in frac.bytes().take(6) {
        micros += (b - b'0') as i64 * scale;
        scale /= 10;
    }
    let mut parts = hms.rsplit(':');
    let sec: i64 = parts.next()?.trim().parse().ok()?;
    let min: i64 = parts.next()?.trim().parse().ok()?;
    let hour: i64 = match parts.next() {
        Some(h) => h.trim().parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() || sec > 99 || min > 99 || !(0..=10_000).contains(&hour) || sec < 0 || min < 0 {
        return None;
    }
    Some(((hour * 60 + min) * 60 + sec) * 1_000_000 + micros)
}

/// Decode the entities WebVTT and SRT files use: `&amp; &lt; &gt; &nbsp; &lrm; &rlm; &quot; &apos;` and numeric.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return String::from(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            "lrm" | "rlm" => Some('\u{200e}'),
            n if n.starts_with("#x") || n.starts_with("#X") => {
                u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32)
            }
            n if n.starts_with('#') => n[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some('\u{200e}') => {} // direction marks are invisible; drop them
            Some(c) => out.push(c),
            None => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Remove `<...>` tags and `{\...}` override blocks (ASS-style, which SRT files often carry), decode entities.
pub fn clean_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut depth_angle = false;
    let mut depth_brace = false;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' if !depth_brace => {
                // Only treat it as a tag if it closes on this line and starts like one.
                let next = chars.peek().copied().unwrap_or(' ');
                if next.is_ascii_alphabetic() || next == '/' || next.is_ascii_digit() {
                    depth_angle = true;
                } else {
                    out.push(c);
                }
            }
            '>' if depth_angle => depth_angle = false,
            '{' if !depth_angle && chars.peek() == Some(&'\\') => depth_brace = true,
            '}' if depth_brace => depth_brace = false,
            _ if depth_angle || depth_brace => {}
            '\r' => {}
            c => out.push(c),
        }
    }
    let decoded = decode_entities(&out);
    // Trim every line and drop empty lines at the ends; keep blank lines out entirely (they are cue separators).
    let lines: Vec<&str> = decoded.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect();
    lines.join("\n")
}

/// Parse a timing line `start --> end [settings]`.
fn parse_timing(line: &str) -> Option<(Timestamp, Timestamp)> {
    let (a, b) = line.split_once("-->")?;
    let start = parse_timestamp(a)?;
    let end = parse_timestamp(b.split_whitespace().next()?)?;
    Some((start, end))
}

/// Parse an SRT file (BOM and CRLF tolerated). Malformed blocks are skipped, tags are stripped.
pub fn parse_srt(input: &str) -> Vec<Cue> {
    let input = input.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    // A block is a run of non-blank lines; a blank line inside text would end the cue, as players do.
    let mut block: Vec<&str> = Vec::new();
    let flush = |block: &mut Vec<&str>, cues: &mut Vec<Cue>| {
        if block.is_empty() {
            return;
        }
        // Skip an optional numeric index (any lines before the timing line).
        if let Some(pos) = block.iter().position(|l| l.contains("-->")) {
            if let Some((start, end)) = parse_timing(block[pos]) {
                let text = clean_text(&block[pos + 1..].join("\n"));
                if !text.is_empty() && end >= start {
                    cues.push(Cue { start, end, text });
                }
            }
        }
        block.clear();
    };
    for line in input.split('\n') {
        if line.trim().is_empty() {
            flush(&mut block, &mut cues);
        } else {
            block.push(line);
        }
    }
    flush(&mut block, &mut cues);
    cues
}

/// Parse a WebVTT file: the `WEBVTT` header, `NOTE`, `STYLE` and `REGION` blocks are skipped, cue identifiers and
/// settings are ignored, tags are stripped.
pub fn parse_vtt(input: &str) -> Vec<Cue> {
    let input = input.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    let mut first = true;
    let flush = |block: &mut Vec<&str>, cues: &mut Vec<Cue>, first: &mut bool| {
        if block.is_empty() {
            return;
        }
        let head = block[0];
        if *first && head.starts_with("WEBVTT") {
            // The header block, possibly with metadata lines.
        } else if head.starts_with("NOTE") || head.starts_with("STYLE") || head.starts_with("REGION") {
            // Comments and style sheets.
        } else if let Some(pos) = block.iter().take(2).position(|l| l.contains("-->")) {
            if let Some((start, end)) = parse_timing(block[pos]) {
                let text = clean_text(&block[pos + 1..].join("\n"));
                if !text.is_empty() && end >= start {
                    cues.push(Cue { start, end, text });
                }
            }
        }
        *first = false;
        block.clear();
    };
    for line in input.split('\n') {
        if line.trim().is_empty() {
            flush(&mut block, &mut cues, &mut first);
        } else {
            block.push(line);
        }
    }
    flush(&mut block, &mut cues, &mut first);
    cues
}

/// Detect the format of a subtitle file's text and parse it.
pub fn parse(input: &str) -> Vec<Cue> {
    let t = input.trim_start_matches('\u{feff}');
    let mut cues = if t.starts_with("WEBVTT") { parse_vtt(t) } else { parse_srt(t) };
    sort_cues(&mut cues);
    cues
}

/// Sort by start time (stable, so cues with equal starts keep file order).
pub fn sort_cues(cues: &mut [Cue]) {
    cues.sort_by_key(|c| c.start);
}

/// Decode an MP4 `tx3g` (`mov_text`) sample: a 16-bit big-endian length, that many bytes of UTF-8 text, then
/// optional style boxes (ignored). An empty sample clears the screen.
pub fn decode_mov_text(data: &[u8]) -> Option<String> {
    if data.len() < 2 {
        return None;
    }
    let len = u16::from_be_bytes([data[0], data[1]]) as usize;
    let text = data.get(2..2 + len.min(data.len() - 2))?;
    let s = core::str::from_utf8(text).ok()?;
    let s = clean_text(s);
    (!s.is_empty()).then_some(s)
}

/// Decode an MP4 `wvtt` sample: boxes, of which `vttc` holds a `payl` box with the cue text (`vtte` is an empty
/// cue). Several `vttc` boxes make several lines.
pub fn decode_wvtt_sample(data: &[u8]) -> Option<String> {
    fn boxes(mut d: &[u8], mut f: impl FnMut(&[u8; 4], &[u8])) {
        while d.len() >= 8 {
            let size = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as usize;
            let ty = [d[4], d[5], d[6], d[7]];
            let size = if size < 8 || size > d.len() { d.len() } else { size };
            f(&ty, &d[8..size]);
            d = &d[size..];
        }
    }
    let mut lines: Vec<String> = Vec::new();
    boxes(data, |ty, body| {
        if ty == b"vttc" {
            boxes(body, |t2, b2| {
                if t2 == b"payl" {
                    if let Ok(s) = core::str::from_utf8(b2) {
                        let s = clean_text(s);
                        if !s.is_empty() {
                            lines.push(s);
                        }
                    }
                }
            });
        }
    });
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Decode a Matroska subtitle block (`S_TEXT/UTF8` or `S_TEXT/WEBVTT`): the payload is the cue text.
pub fn decode_mkv_text(data: &[u8]) -> Option<String> {
    let s = core::str::from_utf8(data).ok()?;
    // Matroska SRT blocks may use "\N" for a line break (an ASS habit).
    let s = clean_text(&s.replace("\\N", "\n").replace("\\n", "\n"));
    (!s.is_empty()).then_some(s)
}

/// Cues sorted by start time, with lookup of what is on screen.
#[derive(Debug, Clone, Default)]
pub struct CueList {
    cues: Vec<Cue>,
}

impl CueList {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// A list from parsed cues.
    pub fn from_cues(mut cues: Vec<Cue>) -> Self {
        cues.truncate(MAX_CUES);
        for c in cues.iter_mut().filter(|c| c.text.len() > MAX_CUE_TEXT) {
            let mut end = MAX_CUE_TEXT;
            while !c.text.is_char_boundary(end) {
                end -= 1;
            }
            c.text.truncate(end);
        }
        sort_cues(&mut cues);
        Self { cues }
    }

    /// Insert a cue, keeping the order. A cue identical to one already present (same start and text) is ignored,
    /// so re-reading a stretch of a file after a seek adds nothing.
    pub fn insert(&mut self, cue: Cue) {
        // Bounded: a hostile file must not be able to grow the list (and the cost of every lookup) without limit.
        if self.cues.len() >= MAX_CUES {
            return;
        }
        let mut cue = cue;
        if cue.text.len() > MAX_CUE_TEXT {
            let mut end = MAX_CUE_TEXT;
            while !cue.text.is_char_boundary(end) {
                end -= 1;
            }
            cue.text.truncate(end);
        }
        let i = self.cues.partition_point(|c| c.start <= cue.start);
        if self.cues[..i].iter().rev().take_while(|c| c.start == cue.start).any(|c| c.text == cue.text) {
            return;
        }
        self.cues.insert(i, cue);
    }

    /// All cues.
    pub fn cues(&self) -> &[Cue] {
        &self.cues
    }

    /// Number of cues.
    pub fn len(&self) -> usize {
        self.cues.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    /// The text on screen at `t` (start inclusive, end exclusive). With overlapping cues the texts are joined,
    /// earliest first.
    pub fn text_at(&self, t: Timestamp) -> Option<String> {
        let hi = self.cues.partition_point(|c| c.start <= t);
        let mut lines: Vec<&str> = Vec::new();
        // Cues are sorted by start; look back over a bounded window for ones that are still active.
        for c in self.cues[..hi].iter().rev().take(16) {
            if c.end > t {
                lines.push(&c.text);
            }
        }
        if lines.is_empty() {
            return None;
        }
        lines.reverse();
        Some(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn parses_basic_srt_with_bom_and_crlf() {
        let src = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,500\r\nHello\r\nworld\r\n\r\n2\r\n00:01:00,000 --> 00:01:01,000\r\nBye\r\n";
        let cues = parse_srt(src);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0], Cue { start: 1_000_000, end: 2_500_000, text: "Hello\nworld".into() });
        assert_eq!(cues[1].start, 60_000_000);
    }

    #[test]
    fn timestamp_forms() {
        assert_eq!(parse_timestamp("01:02:03,004"), Some(3_723_004_000));
        assert_eq!(parse_timestamp("02:03.500"), Some(123_500_000));
        assert_eq!(parse_timestamp("00:00:01.5"), Some(1_500_000));
        assert_eq!(parse_timestamp("00:00:01.05"), Some(1_050_000));
        assert_eq!(parse_timestamp("00:00:01.123456"), Some(1_123_456));
        assert_eq!(parse_timestamp("nope"), None);
        assert_eq!(parse_timestamp("1:2:3:4.000"), None);
        assert_eq!(parse_timestamp("00:00:01."), None);
    }

    #[test]
    fn srt_strips_styling_and_decodes_entities() {
        let src = "1\n00:00:01,000 --> 00:00:02,000\n<i>Hello</i> <font color=\"#ff0000\">red</font> {\\an8}top &amp; &lt;b&gt; &#65;&#x42;\n\n";
        let c = parse_srt(src);
        assert_eq!(c[0].text, "Hello red top & <b> AB");
    }

    #[test]
    fn srt_edge_cases() {
        // No index lines, mixed index and blank-line quirks, multiple blank lines, trailing spaces, a bad block,
        // a cue with the end before the start, an empty cue, a plain "<" in dialogue.
        let src = "00:00:01,000 --> 00:00:02,000\nA\n\n\n\n2\n00:00:03,000 --> 00:00:04,000 X1:100 X2:200 Y1:50 Y2:80\nB   \n  C\n\nbroken\nblock\n\n4\n00:00:06,000 --> 00:00:05,000\nbackwards\n\n5\n00:00:07,000 --> 00:00:08,000\n\n\n6\n00:00:09,000 --> 00:00:10,000\nx < y and 3 > 2\n";
        let c = parse_srt(src);
        let t: Vec<&str> = c.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(t, ["A", "B\nC", "x < y and 3 > 2"]);
        assert_eq!(c[1].start, 3_000_000);
    }

    #[test]
    fn overlapping_cues_are_both_shown() {
        let c = parse("1\n00:00:01,000 --> 00:00:04,000\nlong\n\n2\n00:00:02,000 --> 00:00:03,000\nshort\n");
        let l = CueList::from_cues(c);
        assert_eq!(l.text_at(500_000), None);
        assert_eq!(l.text_at(1_500_000).as_deref(), Some("long"));
        assert_eq!(l.text_at(2_500_000).as_deref(), Some("long\nshort"));
        assert_eq!(l.text_at(3_500_000).as_deref(), Some("long"));
        assert_eq!(l.text_at(4_000_000), None, "end is exclusive");
    }

    #[test]
    fn webvtt_header_notes_styles_ids_and_settings() {
        let src = "\u{feff}WEBVTT - a title\nKind: captions\n\nSTYLE\n::cue { color: red }\n\nNOTE this is\na comment\n\nREGION\nid:r1\n\nintro\n00:01.000 --> 00:02.500 align:start position:10% line:0\n<v Roger>Hello <c.loud>there</c></v>\n<00:01.500>again\n\n00:00:03.000 --> 00:00:04.000\nSecond &amp; last\n\n";
        let c = parse_vtt(src);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], Cue { start: 1_000_000, end: 2_500_000, text: "Hello there\nagain".into() });
        assert_eq!(c[1].text, "Second & last");
    }

    #[test]
    fn webvtt_with_crlf_and_hours() {
        let src = "WEBVTT\r\n\r\n01:00:00.000 --> 01:00:01.000\r\nAn hour in\r\n\r\n";
        let c = parse(src);
        assert_eq!(c[0].start, 3_600_000_000);
        assert_eq!(c[0].text, "An hour in");
    }

    #[test]
    fn parse_sorts_by_start() {
        let c = parse("2\n00:00:05,000 --> 00:00:06,000\nlate\n\n1\n00:00:01,000 --> 00:00:02,000\nearly\n");
        assert_eq!(c[0].text, "early");
    }

    #[test]
    fn packet_payloads() {
        // tx3g: length, text, then a style box that is ignored.
        let mut s = alloc::vec![0, 5];
        s.extend_from_slice(b"Hello");
        s.extend_from_slice(&[0, 0, 0, 10, b's', b't', b'y', b'l', 0, 0]);
        assert_eq!(decode_mov_text(&s).as_deref(), Some("Hello"));
        assert_eq!(decode_mov_text(&[0, 0]), None, "an empty sample clears the screen");
        // wvtt: vttc { payl }.
        let payl = b"<i>Hi</i>";
        let mut vttc = alloc::vec![];
        vttc.extend_from_slice(&((8 + payl.len()) as u32).to_be_bytes());
        vttc.extend_from_slice(b"payl");
        vttc.extend_from_slice(payl);
        let mut sample = alloc::vec![];
        sample.extend_from_slice(&((8 + vttc.len()) as u32).to_be_bytes());
        sample.extend_from_slice(b"vttc");
        sample.extend_from_slice(&vttc);
        assert_eq!(decode_wvtt_sample(&sample).as_deref(), Some("Hi"));
        assert_eq!(decode_wvtt_sample(&[0, 0, 0, 8, b'v', b't', b't', b'e']), None);
        assert_eq!(decode_mkv_text(b"Line one\\NLine two").as_deref(), Some("Line one\nLine two"));
        assert_eq!(decode_mkv_text(b"<i>x</i>").as_deref(), Some("x"));
    }

    #[test]
    fn cue_list_ignores_duplicates_and_keeps_order() {
        let mut l = CueList::new();
        for (s, e, t) in [(3, 4, "c"), (1, 2, "a"), (3, 4, "c"), (2, 3, "b")] {
            l.insert(Cue { start: s * 1_000_000, end: e * 1_000_000, text: t.to_string() });
        }
        let t: Vec<&str> = l.cues().iter().map(|c| c.text.as_str()).collect();
        assert_eq!(t, ["a", "b", "c"]);
    }
}
