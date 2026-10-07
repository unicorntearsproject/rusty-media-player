//! Saving and loading: the index, the playlists and each cover thumbnail are separate blobs under `Storage` keys, in a small
//! little-endian format of our own (versioned; a blob that does not parse is ignored and the library is simply rescanned).
use crate::art::Thumb;
use crate::index::{FolderArt, Library};
use crate::model::*;
use crate::plist::{PlEntry, SavedPlaylist};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Storage key of the index.
pub const INDEX_KEY: &str = "library/index";
/// Storage key of the saved playlists.
pub const PLAYLISTS_KEY: &str = "library/playlists";

/// Storage key of the thumbnail of picture `id`.
pub fn art_key(id: ArtId) -> String {
    format!("library/art/{id:016x}")
}

const INDEX_MAGIC: &[u8; 4] = b"RVPL";
const PLAYLIST_MAGIC: &[u8; 4] = b"RVPP";
const THUMB_MAGIC: &[u8; 4] = b"RVPT";
const VERSION: u8 = 1;
/// The index is at version 2: version 1 plus the loudness of every track (version 1 loads, with no loudness known).
const INDEX_VERSION: u8 = 2;

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        let b = s.as_bytes();
        let n = b.len().min(u16::MAX as usize);
        let mut n2 = n;
        while !s.is_char_boundary(n2) {
            n2 -= 1;
        }
        self.u16(n2 as u16);
        self.0.extend_from_slice(&b[..n2]);
    }
}

struct R<'a>(&'a [u8]);

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("truncated".to_string());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().map_err(|_| "x")?))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| "x")?))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| "x")?))
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().map_err(|_| "x")?))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "not utf-8".to_string())
    }
    /// A count that cannot exceed what the remaining bytes could hold at `min_each` bytes per item.
    fn count(&mut self, min_each: usize) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min_each) > self.0.len() {
            return Err("count too large".to_string());
        }
        Ok(n)
    }
}

impl Library {
    /// The index as bytes (roots, tracks, folder pictures), and mark it saved.
    pub fn save_index(&mut self) -> Vec<u8> {
        self.dirty = false;
        let mut w = W(Vec::new());
        w.0.extend_from_slice(INDEX_MAGIC);
        w.u8(INDEX_VERSION);
        w.u32(self.next_id);
        w.u16(self.roots.len() as u16);
        for r in &self.roots {
            w.str(&r.id);
            w.str(&r.name);
        }
        w.u32(self.tracks.len() as u32);
        for t in &self.tracks {
            w.u32(t.id);
            w.u16(t.root);
            w.str(&t.path);
            w.u64(t.size);
            w.i64(t.mtime_ms);
            w.str(&t.title);
            w.str(&t.artist);
            w.str(&t.album_artist);
            w.str(&t.album);
            w.u16(t.track_no);
            w.u16(t.track_total);
            w.u16(t.disc_no);
            w.u16(t.disc_total);
            w.u32(t.year as u32);
            w.str(&t.genre);
            w.i64(t.duration_us);
            w.u64(t.art);
            w.u8(t.art_embedded as u8 | (t.unreadable as u8) << 1);
            w.str(&t.codec);
            w.u32(t.sample_rate);
            w.u16(t.channels);
            // Loudness: flags, then the track's figure and the album's tag when there are such.
            let l = &t.loudness;
            w.u8(l.lufs.is_some() as u8
                | (l.measured as u8) << 1
                | (l.album_lufs.is_some() as u8) << 2
                | (l.tried as u8) << 3);
            w.u32(l.lufs.unwrap_or(0.0).to_bits());
            w.u32(l.album_lufs.unwrap_or(0.0).to_bits());
        }
        w.u32(self.folder_art.len() as u32);
        for ((root, dir), f) in &self.folder_art {
            w.u16(*root);
            w.str(dir);
            w.str(&f.path);
            w.u64(f.size);
            w.i64(f.mtime_ms);
            w.u64(f.art);
            w.u8(f.read as u8);
        }
        w.0
    }

    /// A library from bytes made by [`Library::save_index`]. Playlists and thumbnails are loaded separately. Every root starts
    /// unconnected until the host lists it again.
    pub fn load_index(bytes: &[u8]) -> Result<Library, String> {
        let mut r = R(bytes);
        if r.take(4)? != INDEX_MAGIC {
            return Err("not a library index".to_string());
        }
        let version = r.u8()?;
        if !(1..=INDEX_VERSION).contains(&version) {
            return Err("not a library index".to_string());
        }
        let mut l = Library::new();
        l.next_id = r.u32()?.max(1);
        let nroots = r.u16()? as usize;
        for _ in 0..nroots {
            let (id, name) = (r.str()?, r.str()?);
            l.roots.push(Root { id, name, connected: false });
        }
        let n = r.count(40)?;
        let mut last = 0u32;
        for _ in 0..n {
            let id = r.u32()?;
            if id <= last && !l.tracks.is_empty() {
                return Err("track ids out of order".to_string());
            }
            last = id;
            let root = r.u16()?;
            if root as usize >= l.roots.len() {
                return Err("track in an unknown root".to_string());
            }
            let path = r.str()?;
            let size = r.u64()?;
            let mtime_ms = r.i64()?;
            let (title, artist, album_artist, album) = (r.str()?, r.str()?, r.str()?, r.str()?);
            let (track_no, track_total, disc_no, disc_total) = (r.u16()?, r.u16()?, r.u16()?, r.u16()?);
            let year = r.u32()? as i32;
            let genre = r.str()?;
            let duration_us = r.i64()?;
            let art = r.u64()?;
            let flags = r.u8()?;
            let codec = r.str()?;
            let sample_rate = r.u32()?;
            let channels = r.u16()?;
            let mut loudness = TrackLoudness::default();
            if version >= 2 {
                let flags = r.u8()?;
                let (lufs, album) = (f32::from_bits(r.u32()?), f32::from_bits(r.u32()?));
                loudness = TrackLoudness {
                    lufs: (flags & 1 != 0 && lufs.is_finite()).then_some(lufs),
                    measured: flags & 2 != 0,
                    album_lufs: (flags & 4 != 0 && album.is_finite()).then_some(album),
                    tried: flags & 8 != 0,
                };
            }
            l.tracks.push(Track {
                id,
                root,
                path,
                size,
                mtime_ms,
                title,
                artist,
                album_artist,
                album,
                track_no,
                track_total,
                disc_no,
                disc_total,
                year,
                genre,
                duration_us,
                art,
                art_embedded: flags & 1 != 0,
                codec,
                sample_rate,
                channels,
                unreadable: flags & 2 != 0,
                loudness,
                src: String::new(),
            });
        }
        let nf = r.count(20)?;
        for _ in 0..nf {
            let root = r.u16()?;
            let dir = r.str()?;
            let path = r.str()?;
            let size = r.u64()?;
            let mtime_ms = r.i64()?;
            let art = r.u64()?;
            let read = r.u8()? != 0;
            l.folder_art
                .insert((root, dir), FolderArt { path, size, mtime_ms, art, read, id: String::new() });
        }
        l.rebuild();
        l.dirty = false;
        Ok(l)
    }

    /// The saved playlists as bytes, and mark them saved.
    pub fn save_playlists(&mut self) -> Vec<u8> {
        self.playlists_dirty = false;
        let mut w = W(Vec::new());
        w.0.extend_from_slice(PLAYLIST_MAGIC);
        w.u8(VERSION);
        w.u32(self.next_playlist_id);
        w.u32(self.playlists.len() as u32);
        for p in &self.playlists {
            w.u32(p.id);
            w.str(&p.name);
            w.u32(p.entries.len() as u32);
            for e in &p.entries {
                w.str(&e.path);
                match &e.title {
                    Some(t) => {
                        w.u8(1);
                        w.str(t);
                    }
                    None => w.u8(0),
                }
                match e.seconds {
                    Some(s) => {
                        w.u8(1);
                        w.u32(s.clamp(0.0, 4.0e9) as u32);
                    }
                    None => w.u8(0),
                }
            }
        }
        w.0
    }

    /// Load playlists saved by [`Library::save_playlists`] (replacing the current ones); entries are matched to tracks again.
    pub fn load_playlists(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut r = R(bytes);
        if r.take(4)? != PLAYLIST_MAGIC || r.u8()? != VERSION {
            return Err("not a playlist file".to_string());
        }
        let next = r.u32()?;
        let n = r.count(10)?;
        let mut lists = Vec::new();
        for _ in 0..n {
            let id = r.u32()?;
            let name = r.str()?;
            let ne = r.count(4)?;
            let mut entries = Vec::new();
            for _ in 0..ne {
                let path = r.str()?;
                let title = if r.u8()? != 0 { Some(r.str()?) } else { None };
                let seconds = if r.u8()? != 0 { Some(r.u32()? as f64) } else { None };
                entries.push(PlEntry { path, title, seconds, track: None });
            }
            lists.push(SavedPlaylist { id, name, entries });
        }
        self.next_playlist_id = next.max(lists.iter().map(|p| p.id + 1).max().unwrap_or(1));
        self.playlists = lists;
        self.resolve_playlists();
        self.playlists_dirty = false;
        self.rev += 1;
        Ok(())
    }

    /// Thumbnails to load from storage: the ones a view asked for that were dropped from memory, and, while the budget
    /// has room, the other pictures the tracks use (so a small library is complete in memory after a few ticks).
    pub fn wanted_art(&self) -> Vec<ArtId> {
        self.thumbs.wanted(|| self.used_art().into_iter().collect())
    }

    /// Put a thumbnail loaded from storage in memory (without marking it unsaved).
    pub fn load_thumb(&mut self, id: ArtId, bytes: &[u8]) -> bool {
        match decode_thumb(bytes) {
            Some(t) => {
                self.thumbs.insert_loaded(id, t);
                self.rev += 1;
                true
            }
            None => false,
        }
    }
}

/// A thumbnail as bytes.
pub fn encode_thumb(t: &Thumb) -> Vec<u8> {
    let mut w = W(Vec::with_capacity(9 + t.rgb.len()));
    w.0.extend_from_slice(THUMB_MAGIC);
    w.u8(VERSION);
    w.u16(t.w);
    w.u16(t.h);
    w.0.extend_from_slice(&t.rgb);
    w.0
}

/// A thumbnail from bytes made by [`encode_thumb`].
pub fn decode_thumb(bytes: &[u8]) -> Option<Thumb> {
    let mut r = R(bytes);
    if r.take(4).ok()? != THUMB_MAGIC || r.u8().ok()? != VERSION {
        return None;
    }
    let (w, h) = (r.u16().ok()?, r.u16().ok()?);
    let rgb = r.take(w as usize * h as usize * 3).ok()?.to_vec();
    (w > 0 && h > 0).then_some(Thumb { w, h, rgb })
}

const FAVORITES_MAGIC: &[u8; 4] = b"RVPF";
const HISTORY_MAGIC: &[u8; 4] = b"RVPH";

impl Library {
    /// The favorites as bytes, and mark them saved.
    pub fn save_favorites(&mut self) -> Vec<u8> {
        self.favorites_dirty = false;
        let mut w = W(Vec::new());
        w.0.extend_from_slice(FAVORITES_MAGIC);
        w.u8(VERSION);
        w.u32(self.favorites_set().len() as u32);
        for k in self.favorites_set() {
            w.str(k);
        }
        w.0
    }

    /// Put the favorites saved by [`Library::save_favorites`] into this library.
    pub fn load_favorites(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut r = R(bytes);
        if r.take(4)? != FAVORITES_MAGIC || r.u8()? != VERSION {
            return Err("not a favorites file".to_string());
        }
        let n = r.count(2)?;
        let mut set = alloc::collections::BTreeSet::new();
        for _ in 0..n {
            set.insert(r.str()?);
        }
        self.replace_favorites(set);
        Ok(())
    }
}

impl Library {
    /// The play history as bytes (a table of file keys, then 13 bytes a play), and mark it saved.
    pub fn save_history(&mut self) -> Vec<u8> {
        self.history.dirty = false;
        let plays = self.history.plays.clone();
        let mut keys: Vec<&str> = Vec::new();
        let mut index: alloc::collections::BTreeMap<&str, u32> = alloc::collections::BTreeMap::new();
        for p in &plays {
            if !index.contains_key(p.key.as_str()) {
                index.insert(p.key.as_str(), keys.len() as u32);
                keys.push(p.key.as_str());
            }
        }
        let mut w = W(Vec::with_capacity(16 + plays.len() * 13 + keys.len() * 48));
        w.0.extend_from_slice(HISTORY_MAGIC);
        w.u8(VERSION);
        w.u32(keys.len() as u32);
        for k in &keys {
            w.str(k);
        }
        w.u32(plays.len() as u32);
        for p in &plays {
            w.u32(index[p.key.as_str()]);
            w.u32(p.at.clamp(0, u32::MAX as i64) as u32);
            w.u32(p.heard_ms);
            w.u8(u8::from(p.finished) | (u8::from(p.video) << 1));
        }
        w.0
    }

    /// Put the history saved by [`Library::save_history`] into this library (at most [`crate::MAX_PLAYS`] plays are taken).
    pub fn load_history(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut r = R(bytes);
        if r.take(4)? != HISTORY_MAGIC || r.u8()? != VERSION {
            return Err("not a history file".to_string());
        }
        let nk = r.count(2)?;
        let mut keys = Vec::with_capacity(nk);
        for _ in 0..nk {
            keys.push(r.str()?);
        }
        let np = r.count(13)?;
        let mut plays = Vec::with_capacity(np.min(crate::MAX_PLAYS));
        for i in 0..np {
            let k = r.u32()? as usize;
            let at = r.u32()? as i64;
            let heard_ms = r.u32()?;
            let flags = r.u8()?;
            let key = keys.get(k).ok_or_else(|| "a play names a missing file".to_string())?.clone();
            plays.push(crate::Play {
                seq: i as u64,
                key,
                video: flags & 2 != 0,
                at,
                heard_ms,
                finished: flags & 1 != 0,
            });
        }
        if plays.len() > crate::MAX_PLAYS {
            let drop = plays.len() - crate::MAX_PLAYS;
            plays.drain(..drop);
        }
        self.history.next_seq = np as u64;
        self.history.plays = plays;
        self.history.recount();
        self.history.dirty = false;
        self.rev += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::tests::{lib_with, track};
    use alloc::vec;

    #[test]
    fn index_round_trips() {
        let mut t = track(7, "a/b/01 x.mp3", "Zo\u{eb}", "Alb", "T", 1);
        t.year = 1999;
        t.art = 0xdead_beef;
        t.art_embedded = true;
        let mut u = track(9, "c.flac", "", "", "", 0);
        u.unreadable = true;
        let mut l = lib_with(vec![t, u]);
        l.folder_art.insert(
            (0, "a".into()),
            FolderArt {
                path: "a/cover.jpg".into(),
                size: 5,
                mtime_ms: 6,
                art: 77,
                read: true,
                id: "x".into(),
            },
        );
        let bytes = l.save_index();
        assert!(!l.index_dirty());
        let m = Library::load_index(&bytes).unwrap();
        assert_eq!(m.all_tracks().len(), 2);
        assert_eq!(m.all_tracks()[0], Track { src: String::new(), ..l.all_tracks()[0].clone() });
        assert!(m.all_tracks()[1].unreadable);
        assert_eq!(m.track_count(), 1);
        assert_eq!(m.albums().len(), 1);
        assert!(!m.roots()[0].connected, "roots are unconnected until the host lists them");
        assert_eq!(m.folder_art.len(), 1);
        assert_eq!(m.folder_art[&(0, "a".to_string())].art, 77);
        let mut m2 = m;
        assert_eq!(m2.save_index(), bytes, "saving what was loaded gives the same bytes");
    }

    #[test]
    fn damaged_blobs_are_refused_not_trusted() {
        let mut l = lib_with(vec![track(1, "a.mp3", "A", "B", "C", 1)]);
        let bytes = l.save_index();
        assert!(Library::load_index(&bytes[..bytes.len() - 5]).is_err());
        assert!(Library::load_index(b"nope").is_err());
        assert!(Library::load_index(&[]).is_err());
        let mut huge = bytes.clone();
        huge[11] = 0xff; // an absurd root count
        let _ = Library::load_index(&huge);
        for i in 0..bytes.len() {
            let mut b = bytes.clone();
            b[i] ^= 0xff;
            let _ = Library::load_index(&b); // must not panic
        }
        assert!(decode_thumb(&[1, 2, 3]).is_none());
    }

    #[test]
    fn playlists_round_trip_and_resolve_again() {
        let mut l = lib_with(vec![track(1, "a/1.mp3", "A", "B", "C", 1)]);
        let id = l.import_playlist("P", "#EXTM3U\n#EXTINF:5,x\na/1.mp3\nmissing.mp3\n", "");
        let bytes = l.save_playlists();
        let mut m = lib_with(vec![track(1, "a/1.mp3", "A", "B", "C", 1)]);
        m.load_playlists(&bytes).unwrap();
        assert_eq!(m.playlist(id).unwrap().entries, l.playlist(id).unwrap().entries);
        assert!(m.load_playlists(&bytes[..6]).is_err());
        assert_eq!(m.create_playlist("next"), id + 1, "ids keep counting");
    }

    #[test]
    fn thumbs_round_trip() {
        let t = Thumb { w: 2, h: 1, rgb: vec![1, 2, 3, 4, 5, 6] };
        assert_eq!(decode_thumb(&encode_thumb(&t)), Some(t));
        assert_eq!(art_key(255), "library/art/00000000000000ff");
    }
}
