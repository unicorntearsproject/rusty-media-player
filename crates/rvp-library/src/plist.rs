//! Saved playlists: lists of library tracks (and entries that could not be matched to one), with M3U, M3U8 and PLS import and
//! export. Entries that match nothing are kept and flagged as missing, never dropped.
use crate::fold::fold;
use crate::index::Library;
use crate::model::TrackId;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_player::listfile;

/// One entry of a saved playlist.
#[derive(Debug, Clone, PartialEq)]
pub struct PlEntry {
    /// Where the file is: the path below its library root for a library track (the path as written in the playlist file
    /// for one that matched nothing).
    pub path: String,
    /// Title (`Artist - Title` for a library track unless the file said otherwise).
    pub title: Option<String>,
    /// Length in whole seconds.
    pub seconds: Option<f64>,
    /// The library track it resolved to, if any. `None` means missing.
    pub track: Option<TrackId>,
}

/// A saved playlist.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedPlaylist {
    /// Stable id.
    pub id: u32,
    /// Name.
    pub name: String,
    /// Entries in play order.
    pub entries: Vec<PlEntry>,
}

impl SavedPlaylist {
    /// The tracks that resolved, in order (missing entries are skipped).
    pub fn track_ids(&self) -> Vec<TrackId> {
        self.entries.iter().filter_map(|e| e.track).collect()
    }

    /// How many entries match no library track.
    pub fn missing(&self) -> usize {
        self.entries.iter().filter(|e| e.track.is_none()).count()
    }
}

/// A playlist file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFormat {
    /// Extended M3U in UTF-8 (`.m3u8`, and `.m3u`).
    M3u8,
    /// PLS (`.pls`).
    Pls,
}

impl ListFormat {
    /// File extension to export with.
    pub fn extension(self) -> &'static str {
        match self {
            ListFormat::M3u8 => "m3u8",
            ListFormat::Pls => "pls",
        }
    }

    /// The format a file name suggests, if it names a playlist file.
    pub fn from_name(name: &str) -> Option<Self> {
        let n = name.to_ascii_lowercase();
        if n.ends_with(".m3u8") || n.ends_with(".m3u") {
            Some(ListFormat::M3u8)
        } else if n.ends_with(".pls") {
            Some(ListFormat::Pls)
        } else {
            None
        }
    }
}

/// `path` as components: `\` read as `/`, `file://` and percent escapes undone, `.` dropped, folded for comparison.
fn components(path: &str) -> Vec<String> {
    let p = if path.starts_with("file://") { listfile::resolve("", path) } else { path.to_string() };
    p.replace('\\', "/").split('/').filter(|c| !c.is_empty() && *c != ".").map(fold).collect()
}

fn whole_seconds(us: i64) -> Option<f64> {
    (us > 0).then(|| libm::floor((us as f64) / 1_000_000.0 + 0.5))
}

impl Library {
    /// The saved playlists, in the order they were made.
    pub fn playlists(&self) -> &[SavedPlaylist] {
        &self.playlists
    }

    /// The playlist with `id`.
    pub fn playlist(&self, id: u32) -> Option<&SavedPlaylist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    fn playlist_mut(&mut self, id: u32) -> Option<&mut SavedPlaylist> {
        self.playlists_dirty = true;
        self.rev += 1;
        self.playlists.iter_mut().find(|p| p.id == id)
    }

    /// A new empty playlist.
    pub fn create_playlist(&mut self, name: &str) -> u32 {
        let id = self.next_playlist_id.max(1);
        self.next_playlist_id = id + 1;
        let name = if name.trim().is_empty() { "New playlist" } else { name.trim() };
        self.playlists.push(SavedPlaylist { id, name: name.to_string(), entries: Vec::new() });
        self.playlists_dirty = true;
        self.rev += 1;
        id
    }

    /// Delete a playlist.
    pub fn delete_playlist(&mut self, id: u32) {
        self.playlists.retain(|p| p.id != id);
        self.playlists_dirty = true;
        self.rev += 1;
    }

    /// Rename a playlist.
    pub fn rename_playlist(&mut self, id: u32, name: &str) {
        if let Some(p) = self.playlist_mut(id) {
            if !name.trim().is_empty() {
                p.name = name.trim().to_string();
            }
        }
    }

    fn entry_for(&self, id: TrackId) -> Option<PlEntry> {
        let t = self.track(id)?;
        Some(PlEntry {
            path: t.path.clone(),
            title: Some(alloc::format!("{} - {}", t.display_artist(), t.display_title())),
            seconds: whole_seconds(t.duration_us),
            track: Some(id),
        })
    }

    /// Append tracks to a playlist.
    pub fn playlist_add(&mut self, id: u32, tracks: &[TrackId]) {
        let entries: Vec<PlEntry> = tracks.iter().filter_map(|&t| self.entry_for(t)).collect();
        if let Some(p) = self.playlist_mut(id) {
            p.entries.extend(entries);
        }
    }

    /// Remove the entry at `index`.
    pub fn playlist_remove(&mut self, id: u32, index: usize) {
        if let Some(p) = self.playlist_mut(id) {
            if index < p.entries.len() {
                p.entries.remove(index);
            }
        }
    }

    /// Move the entry at `index` by `delta` places.
    pub fn playlist_move(&mut self, id: u32, index: usize, delta: i32) {
        if let Some(p) = self.playlist_mut(id) {
            if index < p.entries.len() {
                let to = (index as i32 + delta).clamp(0, p.entries.len() as i32 - 1) as usize;
                let e = p.entries.remove(index);
                p.entries.insert(to, e);
            }
        }
    }

    /// Replace the entries of a playlist with `tracks` (saving the queue over an existing playlist).
    pub fn playlist_set(&mut self, id: u32, tracks: &[TrackId]) {
        let entries: Vec<PlEntry> = tracks.iter().filter_map(|&t| self.entry_for(t)).collect();
        if let Some(p) = self.playlist_mut(id) {
            p.entries = entries;
        }
    }

    /// The library track a playlist entry's path points at: the track whose path, below its root's name or not, shares the
    /// longest tail of directory names with it (at least the file name). `None` if no file has that name.
    pub fn resolve_path(&self, path: &str) -> Option<TrackId> {
        let want = components(path);
        let name = want.last()?;
        let cands = self.by_file.get(name)?;
        let mut best: Option<(usize, TrackId)> = None;
        for &id in cands {
            let Some(t) = self.track(id) else { continue };
            let mut have = components(&t.path);
            if let Some(r) = self.roots.get(t.root as usize) {
                have.insert(0, fold(&r.name));
            }
            let score = want.iter().rev().zip(have.iter().rev()).take_while(|(a, b)| a == b).count();
            if score >= 1 && best.is_none_or(|(s, _)| score > s) {
                best = Some((score, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Match the entries that point at nothing, and unlink those whose track left the library.
    pub(crate) fn resolve_playlists(&mut self) {
        let mut lists = core::mem::take(&mut self.playlists);
        let mut changed = false;
        for p in &mut lists {
            for e in &mut p.entries {
                if e.track.is_some_and(|t| self.track(t).is_none_or(|t| t.unreadable)) {
                    e.track = None;
                    changed = true;
                }
                if e.track.is_none() {
                    if let Some(id) = self.resolve_path(&e.path) {
                        e.track = Some(id);
                        changed = true;
                    }
                }
            }
        }
        self.playlists = lists;
        self.playlists_dirty |= changed;
    }

    /// Make a playlist from the text of an M3U, M3U8 or PLS file. `base` is where the playlist file is (a path, possibly
    /// empty): relative entries are resolved against it, then matched to library tracks by path tail. Entries that match
    /// nothing are kept and show as missing.
    pub fn import_playlist(&mut self, name: &str, text: &str, base: &str) -> u32 {
        let id = self.create_playlist(name);
        let mut entries = Vec::new();
        for e in listfile::parse(text) {
            let path = listfile::resolve(base, &e.path);
            let track = self.resolve_path(&path);
            let (path, title, seconds) = match track.and_then(|t| self.entry_for(t)) {
                Some(lib) => {
                    (lib.path, e.title.or(lib.title), e.seconds.map(|s| libm::floor(s + 0.5)).or(lib.seconds))
                }
                None => (path, e.title, e.seconds.map(|s| libm::floor(s + 0.5))),
            };
            entries.push(PlEntry { path, title, seconds, track });
        }
        if let Some(p) = self.playlist_mut(id) {
            p.entries = entries;
        }
        id
    }

    /// The text of a playlist file for playlist `id`.
    pub fn export_playlist(&self, id: u32, format: ListFormat) -> Option<String> {
        let p = self.playlist(id)?;
        let entries: Vec<listfile::Entry> = p
            .entries
            .iter()
            .map(|e| listfile::Entry { path: e.path.clone(), title: e.title.clone(), seconds: e.seconds })
            .collect();
        Some(match format {
            ListFormat::M3u8 => listfile::export_m3u(&entries),
            ListFormat::Pls => listfile::export_pls(&entries),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::tests::{lib_with, track};
    use alloc::vec;

    fn library() -> Library {
        lib_with(vec![
            track(1, "Aurora Vale/Polar Nights/01 Glass.mp3", "Aurora Vale", "Polar Nights", "Glass", 1),
            track(2, "Aurora Vale/Polar Nights/02 Ember.mp3", "Aurora Vale", "Polar Nights", "Ember", 2),
            track(3, "Other/Mix/01 Glass.mp3", "Other", "Mix", "Glass", 1),
        ])
    }

    #[test]
    fn resolves_by_the_longest_matching_tail() {
        let l = library();
        assert_eq!(l.resolve_path("Aurora Vale/Polar Nights/01 Glass.mp3"), Some(1));
        assert_eq!(l.resolve_path("/home/me/Music/Other/Mix/01 Glass.mp3"), Some(3));
        assert_eq!(l.resolve_path("C:\\Music\\r\\Aurora Vale\\Polar Nights\\02 Ember.mp3"), Some(2));
        assert_eq!(l.resolve_path("file:///x/Aurora%20Vale/Polar%20Nights/01%20Glass.mp3"), Some(1));
        assert_eq!(
            l.resolve_path("01 glass.MP3"),
            Some(1),
            "a bare name matches the first file of that name"
        );
        assert_eq!(l.resolve_path("nothing.mp3"), None);
        assert_eq!(l.resolve_path("http://x/stream"), None);
    }

    #[test]
    fn m3u8_and_pls_round_trip_with_missing_entries() {
        let mut l = library();
        let text = "\u{feff}#EXTM3U\r\n#EXTINF:215,Some Title\r\nAurora Vale/Polar Nights/02 Ember.mp3\r\n/gone/away.flac\r\n#EXTINF:-1,Stream\r\nhttp://x/y\r\n";
        let id = l.import_playlist("Imported", text, "");
        let p = l.playlist(id).unwrap().clone();
        assert_eq!(p.entries.len(), 3, "nothing is dropped");
        assert_eq!(p.entries[0].track, Some(2));
        assert_eq!(p.entries[0].title.as_deref(), Some("Some Title"));
        assert_eq!(p.entries[0].seconds, Some(215.0));
        assert_eq!((p.entries[1].track, p.entries[2].track), (None, None));
        assert_eq!(p.missing(), 2);
        assert_eq!(p.track_ids(), [2]);
        for fmt in [ListFormat::M3u8, ListFormat::Pls] {
            let out = l.export_playlist(id, fmt).unwrap();
            let id2 = l.import_playlist("Again", &out, "");
            assert_eq!(
                l.playlist(id2).unwrap().entries,
                p.entries,
                "{fmt:?}: import, export, import gives an equal list"
            );
            // And the export of the second import is byte for byte the first export.
            assert_eq!(l.export_playlist(id2, fmt).unwrap(), out);
        }
    }

    #[test]
    fn relative_entries_resolve_against_the_playlist_location() {
        let mut l = library();
        let id =
            l.import_playlist("Rel", "../Other/Mix/01 Glass.mp3\n./x/../missing.mp3\n", "lists/mine.m3u");
        let p = l.playlist(id).unwrap();
        assert_eq!(p.entries[0].track, Some(3));
        assert_eq!(p.entries[1].path, "lists/missing.mp3", "the base directory is kept for what is missing");
        assert_eq!(p.entries[1].track, None);
    }

    #[test]
    fn missing_entries_resolve_once_the_files_appear() {
        let mut l = library();
        let id = l.import_playlist("P", "Later/Folder/new.mp3\n", "");
        assert_eq!(l.playlist(id).unwrap().missing(), 1);
        l.tracks.push(track(9, "Later/Folder/new.mp3", "X", "Y", "Z", 1));
        l.rebuild();
        assert_eq!(l.playlist(id).unwrap().missing(), 0);
        // And an entry goes back to missing when its track leaves.
        l.tracks.retain(|t| t.id != 9);
        l.rebuild();
        assert_eq!(l.playlist(id).unwrap().missing(), 1);
        assert_eq!(l.playlist(id).unwrap().entries.len(), 1);
    }

    #[test]
    fn edits() {
        let mut l = library();
        let id = l.create_playlist("  Mine ");
        assert_eq!(l.playlist(id).unwrap().name, "Mine");
        l.playlist_add(id, &[1, 2, 3, 99]);
        assert_eq!(l.playlist(id).unwrap().track_ids(), [1, 2, 3], "unknown tracks are skipped");
        l.playlist_move(id, 2, -2);
        assert_eq!(l.playlist(id).unwrap().track_ids(), [3, 1, 2]);
        l.playlist_remove(id, 1);
        assert_eq!(l.playlist(id).unwrap().track_ids(), [3, 2]);
        l.playlist_set(id, &[2]);
        assert_eq!(l.playlist(id).unwrap().track_ids(), [2]);
        l.rename_playlist(id, "");
        assert_eq!(l.playlist(id).unwrap().name, "Mine");
        l.delete_playlist(id);
        assert!(l.playlists().is_empty());
        assert_eq!(ListFormat::from_name("a.M3U8"), Some(ListFormat::M3u8));
        assert_eq!(ListFormat::from_name("a.pls"), Some(ListFormat::Pls));
        assert_eq!(ListFormat::from_name("a.mp3"), None);
    }
}
