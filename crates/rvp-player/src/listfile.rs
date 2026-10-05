//! Playlist files: M3U / M3U8 (with `#EXTINF`) and PLS, read and written. Lenient: a line that makes no sense is
//! skipped, never an error, and entries are never dropped for pointing at something that is missing (the caller flags
//! those). Relative paths are resolved against the location of the playlist file with [`resolve`].
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Most entries read from one file, and the longest path or title kept (bytes): bounds what a hostile file can cost.
pub const MAX_ENTRIES: usize = 100_000;
/// See [`MAX_ENTRIES`].
pub const MAX_FIELD: usize = 4096;

/// One playlist entry as written in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The path or URL exactly as in the file (relative paths are not resolved yet).
    pub path: String,
    /// The title, if the file has one (`#EXTINF`, `TitleN`).
    pub title: Option<String>,
    /// The length in seconds, if the file says so (negative values, used for "unknown", become `None`).
    pub seconds: Option<f64>,
}

fn clip(s: &str) -> String {
    let mut end = s.len().min(MAX_FIELD);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The text without a byte order mark.
fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// Lines split at `\n`, `\r\n` or a lone `\r`.
fn lines(s: &str) -> impl Iterator<Item = &str> {
    s.split(['\n', '\r']).map(str::trim).filter(|l| !l.is_empty())
}

/// Parse an M3U or M3U8 file.
pub fn parse_m3u(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut pending: Option<(Option<f64>, Option<String>)> = None;
    for line in lines(strip_bom(text)) {
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            // `#EXTINF:123,Artist - Title` (attributes such as `tvg-id="x"` may sit between the number and the comma).
            let (head, title) = info.split_once(',').unwrap_or((info, ""));
            let secs = head.split_whitespace().next().and_then(|n| n.parse::<f64>().ok()).filter(|s| s.is_finite() && *s >= 0.0);
            let title = title.trim();
            pending = Some((secs, (!title.is_empty()).then(|| clip(title))));
        } else if line.starts_with('#') {
            // `#EXTM3U`, `#EXTGRP`, comments.
        } else {
            if out.len() >= MAX_ENTRIES {
                break;
            }
            let (seconds, title) = pending.take().unwrap_or((None, None));
            out.push(Entry { path: clip(line), title, seconds });
        }
    }
    out
}

/// Parse a PLS file (`[playlist]`, `FileN=`, `TitleN=`, `LengthN=`; numbering may skip or be out of order).
pub fn parse_pls(text: &str) -> Vec<Entry> {
    let mut slots: Vec<(u32, Entry)> = Vec::new();
    fn slot(slots: &mut Vec<(u32, Entry)>, n: u32) -> &mut Entry {
        if let Some(i) = slots.iter().position(|(k, _)| *k == n) {
            return &mut slots[i].1;
        }
        slots.push((n, Entry { path: String::new(), title: None, seconds: None }));
        &mut slots.last_mut().expect("just pushed").1
    }
    for line in lines(strip_bom(text)) {
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        let split = key.find(|c: char| c.is_ascii_digit()).unwrap_or(key.len());
        let (name, num) = key.split_at(split);
        let Ok(n) = num.parse::<u32>() else { continue };
        if slots.len() >= MAX_ENTRIES && !slots.iter().any(|(k, _)| *k == n) {
            continue;
        }
        match name.to_ascii_lowercase().as_str() {
            "file" => slot(&mut slots, n).path = clip(value),
            "title" => slot(&mut slots, n).title = (!value.is_empty()).then(|| clip(value)),
            "length" => {
                slot(&mut slots, n).seconds = value.parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0);
            }
            _ => {}
        }
    }
    slots.sort_by_key(|(n, _)| *n);
    slots.into_iter().map(|(_, e)| e).filter(|e| !e.path.is_empty()).collect()
}

/// Parse either format (a file starting with `[playlist]` is PLS, anything else is M3U).
pub fn parse(text: &str) -> Vec<Entry> {
    let t = strip_bom(text).trim_start();
    if t.get(..10).is_some_and(|h| h.eq_ignore_ascii_case("[playlist]")) { parse_pls(text) } else { parse_m3u(text) }
}

/// Write an extended M3U file (UTF-8, `\n` line ends).
pub fn export_m3u(entries: &[Entry]) -> String {
    let mut s = String::from("#EXTM3U\n");
    for e in entries {
        if e.title.is_some() || e.seconds.is_some() {
            let secs = e.seconds.map_or(-1, |v| (v + 0.5) as i64);
            let title = e.title.as_deref().unwrap_or("").replace(['\n', '\r'], " ");
            s += &alloc::format!("#EXTINF:{secs},{title}\n");
        }
        s += &e.path.replace(['\n', '\r'], " ");
        s.push('\n');
    }
    s
}

/// Write a PLS file.
pub fn export_pls(entries: &[Entry]) -> String {
    let mut s = String::from("[playlist]\n");
    for (i, e) in entries.iter().enumerate() {
        let n = i + 1;
        s += &alloc::format!("File{n}={}\n", e.path.replace(['\n', '\r'], " "));
        if let Some(t) = &e.title {
            s += &alloc::format!("Title{n}={}\n", t.replace(['\n', '\r'], " "));
        }
        s += &alloc::format!("Length{n}={}\n", e.seconds.map_or(-1, |v| (v + 0.5) as i64));
    }
    s += &alloc::format!("NumberOfEntries={}\nVersion=2\n", entries.len());
    s
}

/// Percent-decode (`%20` and friends) the path of a `file://` URL.
fn url_path(url: &str) -> String {
    let rest = url.strip_prefix("file://").unwrap_or(url);
    // `file://localhost/x` and `file:///x`.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16)) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// True for a path that does not depend on where the playlist is: a URL with a scheme, a Unix absolute path, or a
/// Windows drive or UNC path.
fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
        || p.starts_with("\\\\")
        || p.as_bytes().get(1) == Some(&b':') && p.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        || p.find("://").is_some_and(|i| i > 0 && p[..i].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.'))
}

/// Resolve `path` (an entry of a playlist file) against `base`, the location of the playlist file itself. `file://`
/// URLs become paths; backslashes in a relative path are read as separators (Windows playlists); `.` and `..`
/// segments are folded away. Absolute paths and other URLs are returned as they are.
pub fn resolve(base: &str, path: &str) -> String {
    let path = path.trim();
    if path.starts_with("file://") {
        return url_path(path);
    }
    if is_absolute(path) {
        return path.to_string();
    }
    let rel = path.replace('\\', "/");
    let dir = match base.rfind(['/', '\\']) {
        Some(i) => &base[..=i],
        None => "",
    };
    let joined = alloc::format!("{dir}{rel}");
    let abs = joined.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !abs {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let body = parts.join("/");
    if abs { alloc::format!("/{body}") } else { body }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn e(path: &str, title: Option<&str>, seconds: Option<f64>) -> Entry {
        Entry { path: path.to_string(), title: title.map(ToString::to_string), seconds }
    }

    #[test]
    fn m3u_with_extinf_bom_and_crlf() {
        let text = "\u{feff}#EXTM3U\r\n#EXTINF:215,Artist - Song\r\nmusic/a.mp3\r\n\r\n# a comment\r\n#EXTINF:-1,Live\r\nhttp://x/y\r\nplain.flac\r\n";
        assert_eq!(
            parse_m3u(text),
            vec![
                e("music/a.mp3", Some("Artist - Song"), Some(215.0)),
                e("http://x/y", Some("Live"), None),
                e("plain.flac", None, None),
            ]
        );
    }

    #[test]
    fn m3u_with_lone_cr_and_attributes() {
        let text = "#EXTM3U\r#EXTINF:12 tvg-id=\"x\",Name, with comma\ra.ogg\rb.ogg";
        let p = parse_m3u(text);
        assert_eq!(p[0], e("a.ogg", Some("Name, with comma"), Some(12.0)));
        assert_eq!(p[1], e("b.ogg", None, None));
    }

    #[test]
    fn pls_out_of_order_and_gaps() {
        let text = "[playlist]\nNumberOfEntries=3\nFile3=c.mp3\nTitle3=Third\nLength3=60\nFile1=a.mp3\nfile2 = b.mp3\nLength2=-1\nTitle9=orphan\n";
        assert_eq!(parse_pls(text), vec![e("a.mp3", None, None), e("b.mp3", None, None), e("c.mp3", Some("Third"), Some(60.0))]);
        assert_eq!(parse(text), parse_pls(text));
    }

    #[test]
    fn export_then_import_is_the_same_list() {
        let list = vec![e("a.mp3", Some("A"), Some(10.0)), e("dir/b c.flac", None, None), e("http://h/s", Some("S, x"), None)];
        assert_eq!(parse(&export_m3u(&list)), list);
        let pls = export_pls(&list);
        assert_eq!(parse(&pls), list);
    }

    #[test]
    fn resolve_relative_absolute_and_urls() {
        assert_eq!(resolve("/music/lists/a.m3u", "../x/b.mp3"), "/music/x/b.mp3");
        assert_eq!(resolve("/music/lists/a.m3u", "c/./d.mp3"), "/music/lists/c/d.mp3");
        assert_eq!(resolve("/music/a.m3u", "/abs/e.mp3"), "/abs/e.mp3");
        assert_eq!(resolve("lists/a.m3u", "..\\up\\f.mp3"), "up/f.mp3");
        assert_eq!(resolve("a.m3u", "g.mp3"), "g.mp3");
        assert_eq!(resolve("/m/a.m3u", "http://h/p.mp3"), "http://h/p.mp3");
        assert_eq!(resolve("/m/a.m3u", "C:\\Music\\h.mp3"), "C:\\Music\\h.mp3");
        assert_eq!(resolve("/m/a.m3u", "file:///home/u/My%20Music/i.mp3"), "/home/u/My Music/i.mp3");
        assert_eq!(resolve("/m/a.m3u", "../../../j.mp3"), "/j.mp3");
        assert_eq!(resolve("m/a.m3u", "../../j.mp3"), "../j.mp3");
    }

    #[test]
    fn hostile_input_is_bounded() {
        let big = "x".repeat(10_000);
        assert!(parse_m3u(&big)[0].path.len() <= MAX_FIELD);
        let many: String = (0..MAX_ENTRIES + 10).map(|i| alloc::format!("f{i}.mp3\n")).collect();
        assert_eq!(parse_m3u(&many).len(), MAX_ENTRIES);
        assert!(parse_pls("[playlist]\nFile99999999999999999999=x\nFile=y\nfile-1=z").is_empty());
    }
}
