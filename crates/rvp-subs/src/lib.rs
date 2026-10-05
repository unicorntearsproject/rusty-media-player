//! SRT and WebVTT subtitle parsing. Milestone 8 completes this; the SRT basics are here.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::{string::String, vec::Vec};
use rvp_core::Timestamp;

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

/// Parse `HH:MM:SS,mmm` (or `.` as the separator, or `MM:SS.mmm` as WebVTT allows) to microseconds.
pub fn parse_timestamp(s: &str) -> Option<Timestamp> {
    let s = s.trim();
    let (hms, ms) = s.rsplit_once([',', '.'])?;
    let ms: i64 = ms.parse().ok()?;
    let mut parts = hms.rsplit(':');
    let sec: i64 = parts.next()?.parse().ok()?;
    let min: i64 = parts.next()?.parse().ok()?;
    let hour: i64 = parts.next().map_or(Some(0), |h| h.parse().ok())?;
    Some(((hour * 60 + min) * 60 + sec) * 1_000_000 + ms * 1_000)
}

/// Parse an SRT file (BOM and CRLF tolerated). Malformed blocks are skipped.
pub fn parse_srt(input: &str) -> Vec<Cue> {
    let input = input.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut cues = Vec::new();
    for block in input.split("\n\n") {
        let mut lines = block.lines().filter(|l| !l.trim().is_empty()).peekable();
        // Optional numeric index line.
        if let Some(first) = lines.peek() {
            if !first.contains("-->") {
                lines.next();
            }
        }
        let Some(timing) = lines.next() else { continue };
        let Some((a, b)) = timing.split_once("-->") else { continue };
        let (Some(start), Some(end)) =
            (parse_timestamp(a), parse_timestamp(b.split_whitespace().next().unwrap_or("")))
        else {
            continue;
        };
        let text: Vec<&str> = lines.collect();
        cues.push(Cue { start, end, text: text.join("\n") });
    }
    cues
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(parse_timestamp("nope"), None);
    }
}
