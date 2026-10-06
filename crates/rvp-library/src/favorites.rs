//! Favorites: the songs and videos the person hearted. They are kept by what identifies a file (its folder's name, its name and
//! its length), not by library id, so they survive a rescan, a move of the whole folder tree and a tag edit (none of which changes
//! those). The folder's name is part of it because the same file name and length turn up in many albums ("01 Intro.mp3").
use crate::fold::{fold, natural};
use crate::index::Library;
use crate::model::{Track, TrackId};
use crate::video::{Video, VideoId};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Storage key of the favorites.
pub const FAVORITES_KEY: &str = "library/favorites";

/// What identifies a file: the lower-cased name of its folder and of the file, and its length in milliseconds.
fn key_of(path: &str, duration_us: i64) -> String {
    let mut parts = path.rsplit('/');
    let name = parts.next().unwrap_or(path);
    let folder = parts.next().unwrap_or("");
    format!("{}/{}|{}", folder.to_lowercase(), name.to_lowercase(), duration_us.max(0) / 1000)
}

impl Track {
    /// The key favorites are kept under.
    pub fn favorite_key(&self) -> String {
        key_of(&self.path, self.duration_us)
    }
}

impl Video {
    /// The key favorites are kept under.
    pub fn favorite_key(&self) -> String {
        key_of(&self.path, self.duration_us)
    }
}

impl Library {
    fn favorite_key_of(&self, id: u32) -> Option<String> {
        self.track(id).map(Track::favorite_key).or_else(|| self.video(id).map(Video::favorite_key))
    }

    /// Whether the track or video `id` is a favorite.
    pub fn is_favorite(&self, id: u32) -> bool {
        !self.favorites.is_empty() && self.favorite_key_of(id).is_some_and(|k| self.favorites.contains(&k))
    }

    /// Heart or un-heart a track or video. Returns the new state, or `None` when there is no such item.
    pub fn set_favorite(&mut self, id: u32, on: bool) -> Option<bool> {
        let key = self.favorite_key_of(id)?;
        let changed = if on { self.favorites.insert(key) } else { self.favorites.remove(&key) };
        if changed {
            self.favorites_dirty = true;
            self.rev += 1;
        }
        Some(on)
    }

    /// Flip the heart of a track or video. Returns the new state.
    pub fn toggle_favorite(&mut self, id: u32) -> Option<bool> {
        let now = self.is_favorite(id);
        self.set_favorite(id, !now)
    }

    /// Heart or un-heart every one of `ids`; returns how many changed.
    pub fn set_favorites(&mut self, ids: &[u32], on: bool) -> usize {
        let mut n = 0;
        for &id in ids {
            if self.is_favorite(id) != on && self.set_favorite(id, on).is_some() {
                n += 1;
            }
        }
        n
    }

    /// The favorite songs, by title.
    pub fn favorite_tracks(&self) -> Vec<TrackId> {
        let mut v: Vec<(String, TrackId)> = self
            .all_tracks()
            .iter()
            .filter(|t| !t.unreadable && self.favorites.contains(&t.favorite_key()))
            .map(|t| (format!("{}\u{0}{:010}", natural(&fold(t.display_title())), t.id), t.id))
            .collect();
        v.sort();
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// The favorite videos, by title.
    pub fn favorite_videos(&self) -> Vec<VideoId> {
        let mut v: Vec<(String, VideoId)> = self
            .all_videos()
            .iter()
            .filter(|x| !x.unreadable && self.favorites.contains(&x.favorite_key()))
            .map(|x| (format!("{}\u{0}{:010}", natural(&fold(x.display_title())), x.id), x.id))
            .collect();
        v.sort();
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// How many favorites are kept (including files not in the library at the moment).
    pub fn favorite_count(&self) -> usize {
        self.favorites.len()
    }

    /// True if the favorites changed since [`Library::save_favorites`] last ran.
    pub fn favorites_dirty(&self) -> bool {
        self.favorites_dirty
    }

    pub(crate) fn favorites_set(&self) -> &BTreeSet<String> {
        &self.favorites
    }

    pub(crate) fn replace_favorites(&mut self, set: BTreeSet<String>) {
        self.favorites = set;
        self.favorites_dirty = false;
        self.rev += 1;
    }
}
