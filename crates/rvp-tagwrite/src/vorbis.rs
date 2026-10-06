//! Vorbis comments: the one layout FLAC, Ogg Vorbis and Opus share (a vendor string, then `KEY=value` entries), and the picture block
//! FLAC keeps and Ogg carries base64-encoded in `METADATA_BLOCK_PICTURE`.
use crate::{Change, Cover, Edit, Error, merged_pair, parse_pair};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Who wrote the file, when there was no tag to take the vendor from.
pub(crate) const VENDOR: &str = "Rusty Wave";

/// A comment block: the vendor (bytes, kept as they were) and every entry as it was written.
pub(crate) struct Comments {
    pub vendor: Vec<u8>,
    pub entries: Vec<Vec<u8>>,
}

fn u32le(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
}

impl Comments {
    /// A block with no entries.
    pub fn empty() -> Self {
        Comments { vendor: VENDOR.as_bytes().to_vec(), entries: Vec::new() }
    }

    /// Parse the comment block at the start of `b`; returns it and the bytes it used.
    pub fn parse(b: &[u8]) -> Result<(Comments, usize), Error> {
        let bad = Error::Corrupt("the comment block is cut short");
        let vlen = u32le(b, 0).ok_or(bad.clone())?;
        let vendor = b.get(4..4 + vlen).ok_or(bad.clone())?.to_vec();
        let mut at = 4 + vlen;
        let n = u32le(b, at).ok_or(bad.clone())?;
        at += 4;
        // Each entry takes at least its 4-byte length.
        if n.saturating_mul(4) > b.len().saturating_sub(at) {
            return Err(bad);
        }
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let len = u32le(b, at).ok_or(bad.clone())?;
            let e = b.get(at + 4..at + 4 + len).ok_or(bad.clone())?;
            entries.push(e.to_vec());
            at += 4 + len;
        }
        Ok((Comments { vendor, entries }, at))
    }

    /// The block as bytes (no framing bit).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.vendor.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.vendor);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            out.extend_from_slice(&(e.len() as u32).to_le_bytes());
            out.extend_from_slice(e);
        }
        out
    }

    fn key_of(e: &[u8]) -> Option<&[u8]> {
        e.iter().position(|&c| c == b'=').map(|i| &e[..i])
    }

    fn has_key(e: &[u8], keys: &[&str]) -> bool {
        Self::key_of(e).is_some_and(|k| keys.iter().any(|w| k.eq_ignore_ascii_case(w.as_bytes())))
    }

    /// The text of the first entry with one of `keys`.
    fn first(&self, keys: &[&str]) -> Option<String> {
        self.entries
            .iter()
            .find(|e| Self::has_key(e, keys))
            .and_then(|e| core::str::from_utf8(&e[e.iter().position(|&c| c == b'=')? + 1..]).ok())
            .map(|s| s.to_string())
    }

    fn remove(&mut self, keys: &[&str]) {
        self.entries.retain(|e| !Self::has_key(e, keys));
    }

    fn add(&mut self, key: &str, value: &str) {
        let mut e = key.as_bytes().to_vec();
        e.push(b'=');
        e.extend_from_slice(value.as_bytes());
        self.entries.push(e);
    }

    fn text(&mut self, keys: &[&str], change: &Change<String>) {
        match change {
            Change::Keep => {}
            Change::Clear => self.remove(keys),
            Change::Set(v) => {
                self.remove(keys);
                // An empty value is the same as none.
                if !v.is_empty() {
                    self.add(keys[0], v);
                }
            }
        }
    }

    fn numbers(&mut self, no_keys: &[&str], total_keys: &[&str], no: &Change<u32>, total: &Change<u32>) {
        if no.is_keep() && total.is_keep() {
            return;
        }
        // What is there: "3", "3/12", and a separate total entry.
        let combined = self.first(no_keys).map(|s| parse_pair(&s)).unwrap_or((0, 0));
        let sep_total = self.first(total_keys).and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0);
        let now = (combined.0, if sep_total != 0 { sep_total } else { combined.1 });
        self.remove(no_keys);
        self.remove(total_keys);
        if let Some((n, t)) = merged_pair(now, no, total) {
            if n != 0 {
                self.add(no_keys[0], &n.to_string());
            }
            if t != 0 {
                self.add(total_keys[0], &t.to_string());
            }
        }
    }

    /// Apply the text and number fields of an edit (not the cover).
    pub fn apply(&mut self, edit: &Edit) {
        self.text(&["TITLE"], &edit.title);
        self.text(&["ARTIST"], &edit.artist);
        self.text(&["ALBUM"], &edit.album);
        self.text(&["ALBUMARTIST", "ALBUM ARTIST"], &edit.album_artist);
        self.text(&["GENRE"], &edit.genre);
        self.numbers(
            &["TRACKNUMBER"],
            &["TRACKTOTAL", "TOTALTRACKS", "TRACKSTOTAL"],
            &edit.track_no,
            &edit.track_total,
        );
        self.numbers(
            &["DISCNUMBER"],
            &["DISCTOTAL", "TOTALDISCS", "DISCSTOTAL"],
            &edit.disc_no,
            &edit.disc_total,
        );
        match &edit.year {
            Change::Keep => {}
            Change::Clear => self.remove(&["DATE", "YEAR"]),
            Change::Set(y) => {
                self.remove(&["DATE", "YEAR"]);
                if *y > 0 {
                    self.add("DATE", &y.to_string());
                }
            }
        }
    }

    /// Take out every picture entry (Ogg's `METADATA_BLOCK_PICTURE` and the old `COVERART`).
    pub fn remove_pictures(&mut self) {
        self.remove(&["METADATA_BLOCK_PICTURE", "COVERART", "COVERARTMIME"]);
    }

    /// Add a front cover as a `METADATA_BLOCK_PICTURE` entry.
    pub fn add_picture(&mut self, c: &Cover) {
        self.add("METADATA_BLOCK_PICTURE", &crate::b64::encode(&picture_block(c)));
    }
}

/// A FLAC picture block body for a front cover.
pub(crate) fn picture_block(c: &Cover) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&3u32.to_be_bytes()); // front cover
    b.extend_from_slice(&(c.mime.len() as u32).to_be_bytes());
    b.extend_from_slice(c.mime.as_bytes());
    b.extend_from_slice(&0u32.to_be_bytes()); // no description
    b.extend_from_slice(&[0; 16]); // width, height, depth, colours: not said
    b.extend_from_slice(&(c.data.len() as u32).to_be_bytes());
    b.extend_from_slice(&c.data);
    b
}
