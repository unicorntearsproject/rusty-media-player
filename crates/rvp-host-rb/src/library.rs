//! The media library: roots, and listings read through the cursor (`library_listing`) into one `Listing` per walk.
use crate::api;
use bucket_v0_sys::{self as sys, err};
use rvp_host::{FileEntry, Library, Listing};
use std::collections::{HashMap, VecDeque};

/// How much of a listing one call reads.
const SLICE: usize = 64 * 1024;
/// A record whose id or path claims to be longer than this is not a listing.
const MAX_FIELD: usize = 1 << 20;

/// One line of `library_roots`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootInfo {
    /// Stable id.
    pub id: String,
    /// Name to show.
    pub name: String,
    /// Readable now (a remembered folder on an unplugged disk is not).
    pub readable: bool,
}

/// Undo the escapes of `library_roots` fields (`\t`, `\n`, `\\`).
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The text of `library_roots`: one root per line, `id TAB name TAB readable`.
pub fn parse_roots(text: &str) -> Vec<RootInfo> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let id = unescape(f.next().filter(|s| !s.is_empty())?);
            let name = f.next().map(unescape).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
            let readable = f.next().is_none_or(|r| r.trim() != "0");
            Some(RootInfo { id, name, readable })
        })
        .collect()
}

/// Read the records at the start of `buf` into `out`, removing what was read. A record that is not complete yet stays. Returns
/// false if the data cannot be a listing (an absurd length).
pub fn parse_records(buf: &mut Vec<u8>, out: &mut Vec<FileEntry>) -> bool {
    let mut at = 0;
    loop {
        let rest = &buf[at..];
        if rest.len() < 24 {
            break;
        }
        let size = u64::from_le_bytes(rest[0..8].try_into().unwrap_or([0; 8]));
        let mtime_ms = i64::from_le_bytes(rest[8..16].try_into().unwrap_or([0; 8]));
        let id_len = u32::from_le_bytes(rest[16..20].try_into().unwrap_or([0; 4])) as usize;
        let path_len = u32::from_le_bytes(rest[20..24].try_into().unwrap_or([0; 4])) as usize;
        if id_len > MAX_FIELD || path_len > MAX_FIELD {
            return false;
        }
        let total = (24 + id_len + path_len).div_ceil(8) * 8;
        if rest.len() < total {
            break;
        }
        let id = String::from_utf8_lossy(&rest[24..24 + id_len]).into_owned();
        let path = String::from_utf8_lossy(&rest[24 + id_len..24 + id_len + path_len]).into_owned();
        out.push(FileEntry { id, path, size, mtime_ms });
        at += total;
    }
    buf.drain(..at);
    true
}

/// A listing being read: partial listings append, so the next read continues where this one stopped.
#[derive(Default)]
struct Assembly {
    consumed: u64,
    carry: Vec<u8>,
    files: Vec<FileEntry>,
}

/// `Library` over the App API.
pub struct RbLibrary {
    ready: VecDeque<Listing>,
    assembling: HashMap<String, Assembly>,
    /// Listings handed over so far.
    pub listings: u64,
}

impl RbLibrary {
    /// A library with nothing ready.
    pub fn new() -> Self {
        Self { ready: VecDeque::new(), assembling: HashMap::new(), listings: 0 }
    }

    /// The roots the OS knows.
    pub fn roots() -> Vec<RootInfo> {
        // SAFETY: `text_call` passes a buffer of the size it announces.
        api::text_call(|p, cap| unsafe { sys::library_roots(p, cap) })
            .map(|t| parse_roots(&t))
            .unwrap_or_default()
    }

    /// A `LIBRARY_LISTING` event for `root` arrived: read what the OS has so far; when it was the last part, the listing is ready.
    pub fn on_listing(&mut self, root: &str, partial: bool) {
        let asm = self.assembling.entry(root.to_string()).or_default();
        let mut chunk = vec![0u8; SLICE];
        loop {
            // SAFETY: the root is the string's range and the buffer holds `SLICE` bytes.
            let total = unsafe {
                sys::library_listing(
                    root.as_ptr(),
                    api::len32(root.len()),
                    asm.consumed as i64,
                    chunk.as_mut_ptr(),
                    SLICE as i32,
                )
            };
            if total < 0 {
                api::warn(&format!("library_listing `{root}` failed: {}", api::code_name(total as i32)));
                self.assembling.remove(root);
                return;
            }
            let total = total as u64;
            if total < asm.consumed {
                // A new walk replaced the kept listing: start over.
                *asm = Assembly::default();
                continue;
            }
            let avail = (total - asm.consumed).min(SLICE as u64) as usize;
            if avail == 0 {
                break;
            }
            asm.carry.extend_from_slice(&chunk[..avail]);
            asm.consumed += avail as u64;
            if !parse_records(&mut asm.carry, &mut asm.files) {
                api::warn(&format!("library listing of `{root}` is malformed; dropped"));
                self.assembling.remove(root);
                self.release(root);
                return;
            }
            if asm.consumed >= total {
                break;
            }
        }
        if partial {
            return;
        }
        let Some(asm) = self.assembling.remove(root) else { return };
        if !asm.carry.is_empty() {
            api::warn(&format!("library listing of `{root}` ends in a partial record"));
        }
        let name =
            Self::roots().into_iter().find(|r| r.id == root).map_or_else(|| root.to_string(), |r| r.name);
        self.ready.push_back(Listing { root: root.to_string(), name, files: asm.files });
        self.listings += 1;
        self.release(root);
    }

    fn release(&self, root: &str) {
        // SAFETY: the root is the string's range.
        unsafe { sys::library_listing_release(root.as_ptr(), api::len32(root.len())) };
    }

    /// Ask the OS for a new walk of `root` (`Effect::Rescan`, and a change on disk).
    pub fn rescan(&self, root: &str) -> i32 {
        // SAFETY: the root is the string's range.
        unsafe { sys::library_rescan(root.as_ptr(), api::len32(root.len())) }
    }

    /// Forget `root` and revoke the app's access (`Effect::Forget`).
    pub fn forget(&mut self, root: &str) {
        self.assembling.remove(root);
        // SAFETY: the root is the string's range.
        unsafe { sys::library_forget(root.as_ptr(), api::len32(root.len())) };
    }

    /// Ask the user to make an unreadable root readable again.
    pub fn reconnect(&self, root: &str) -> i32 {
        // SAFETY: the root is the string's range.
        unsafe { sys::library_reconnect(root.as_ptr(), api::len32(root.len())) }
    }
}

impl Default for RbLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl Library for RbLibrary {
    fn take_listing(&mut self) -> Option<Listing> {
        self.ready.pop_front()
    }

    fn connected_roots(&self) -> Vec<String> {
        Self::roots().into_iter().filter(|r| r.readable).map(|r| r.id).collect()
    }
}

/// True when `code` from a library call says the library is not available here.
pub fn unavailable(code: i32) -> bool {
    code == err::UNSUPPORTED || code == err::DENIED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_unescape() {
        let t = "dir:Music\tMy Music\t1\ndir:a\\tb\tName\\nwith\\\\slash\t0\nbare\n";
        let r = parse_roots(t);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0], RootInfo { id: "dir:Music".into(), name: "My Music".into(), readable: true });
        assert_eq!(r[1].id, "dir:a\tb");
        assert_eq!(r[1].name, "Name\nwith\\slash");
        assert!(!r[1].readable);
        assert_eq!(r[2], RootInfo { id: "bare".into(), name: "bare".into(), readable: true });
        assert_eq!(unescape("a\\qb\\"), "a\\qb\\");
    }

    fn rec(id: &str, path: &str, size: u64) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&size.to_le_bytes());
        v.extend_from_slice(&7i64.to_le_bytes());
        v.extend_from_slice(&(id.len() as u32).to_le_bytes());
        v.extend_from_slice(&(path.len() as u32).to_le_bytes());
        v.extend_from_slice(id.as_bytes());
        v.extend_from_slice(path.as_bytes());
        while v.len() % 8 != 0 {
            v.push(0);
        }
        v
    }

    #[test]
    fn records_split_anywhere() {
        let mut all = rec("i1", "a/b.mp3", 100);
        all.extend(rec("", "", 0));
        all.extend(rec("long-id-0123456789", "música/é.flac", u64::MAX));
        // Feed it in awkward pieces: every split point must give the same three entries.
        for cut in 0..all.len() {
            let mut carry = Vec::new();
            let mut out = Vec::new();
            for piece in [&all[..cut], &all[cut..]] {
                carry.extend_from_slice(piece);
                assert!(parse_records(&mut carry, &mut out));
            }
            assert!(carry.is_empty(), "cut {cut}");
            assert_eq!(out.len(), 3, "cut {cut}");
            assert_eq!(out[0], FileEntry { id: "i1".into(), path: "a/b.mp3".into(), size: 100, mtime_ms: 7 });
            assert_eq!(out[2].path, "música/é.flac");
            assert_eq!(out[2].size, u64::MAX);
        }
        let mut bad = vec![0u8; 24];
        bad[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!parse_records(&mut bad, &mut Vec::new()));
    }
}
