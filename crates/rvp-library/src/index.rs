//! The library: roots and tracks, and the albums and artists derived from them (rebuilt after every change).
use crate::art::Thumb;
use crate::fold::{fold, hash32, natural, sort_key};
use crate::model::*;
use crate::plist::SavedPlaylist;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// What the last scan did (for the status line and for tests).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Files read for the first time.
    pub added: usize,
    /// Files read again because their size or time changed.
    pub changed: usize,
    /// Files that were gone and left the index.
    pub removed: usize,
    /// Files left alone.
    pub unchanged: usize,
    /// Tag reads that failed (damaged or unsupported files).
    pub failed: usize,
    /// Folder pictures read.
    pub folder_art_read: usize,
}

/// The folder picture of one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderArt {
    pub path: String,
    pub size: u64,
    pub mtime_ms: i64,
    /// Zero until it has been read (or when it is not a usable picture).
    pub art: ArtId,
    /// It has been read (successfully or not): do not ask again until it changes.
    pub read: bool,
    /// What the host opens it with in this session (not saved).
    pub id: String,
}

/// How to order the track list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackSort {
    /// By title.
    #[default]
    Title,
    /// By artist, then album, then track.
    Artist,
    /// By album artist, album, then track.
    Album,
    /// By length.
    Duration,
    /// By year.
    Year,
    /// Newest first (order of discovery).
    Added,
}

/// Matches for a search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    /// Matching tracks, best order first (by album, then track).
    pub tracks: Vec<TrackId>,
    /// Matching albums (indexes into [`Library::albums`]).
    pub albums: Vec<usize>,
    /// Matching artists (indexes into [`Library::artists`]).
    pub artists: Vec<usize>,
    /// Matching videos, by title.
    pub videos: Vec<crate::video::VideoId>,
}

/// Tag changes to show at once (see [`Library::patch_tags`]): `Some` replaces the field (an empty text or a 0 clears it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagPatch {
    /// Title.
    pub title: Option<String>,
    /// Artist.
    pub artist: Option<String>,
    /// Album.
    pub album: Option<String>,
    /// Album artist.
    pub album_artist: Option<String>,
    /// Genre.
    pub genre: Option<String>,
    /// Track number.
    pub track_no: Option<u16>,
    /// Tracks on the disc.
    pub track_total: Option<u16>,
    /// Disc number.
    pub disc_no: Option<u16>,
    /// Discs in the set.
    pub disc_total: Option<u16>,
    /// Year.
    pub year: Option<i32>,
}

/// The library.
#[derive(Debug, Default)]
pub struct Library {
    pub(crate) roots: Vec<Root>,
    /// Ascending by id.
    pub(crate) tracks: Vec<Track>,
    pub(crate) next_id: TrackId,
    pub(crate) folder_art: BTreeMap<(u16, String), FolderArt>,
    pub(crate) thumbs: crate::thumbs::ThumbCache,
    pub(crate) albums: Vec<Album>,
    pub(crate) artists: Vec<Artist>,
    album_ix: BTreeMap<u32, usize>,
    artist_ix: BTreeMap<u32, usize>,
    track_album: BTreeMap<TrackId, usize>,
    /// Folded searchable text per visible track, same order as `visible`.
    hay: Vec<String>,
    /// Indexes into `tracks` of the tracks that are shown.
    visible: Vec<usize>,
    /// Folded file name -> tracks, for resolving playlist entries.
    pub(crate) by_file: BTreeMap<String, Vec<TrackId>>,
    pub(crate) playlists: Vec<SavedPlaylist>,
    pub(crate) next_playlist_id: u32,
    /// The videos, ascending by id (see `videos.rs`).
    pub(crate) videos: Vec<crate::video::Video>,
    /// The videos changed since they were last saved.
    pub(crate) videos_dirty: bool,
    pub(crate) rev: u64,
    /// The index (not the pictures) changed since it was last saved.
    pub(crate) dirty: bool,
    pub(crate) playlists_dirty: bool,
    /// Keys of the favorite files (see `favorites.rs`).
    pub(crate) favorites: BTreeSet<String>,
    pub(crate) favorites_dirty: bool,
    /// What the last scan did.
    pub report: ScanReport,
}

impl Library {
    /// An empty library.
    pub fn new() -> Self {
        Self { next_id: 1, next_playlist_id: 1, ..Self::default() }
    }

    /// Show what a tag edit just wrote at once, without waiting for the folder to be read again: the fields of the track change, the
    /// albums and artists are regrouped and the search text is rebuilt. (The next scan reads the file itself and confirms it.)
    pub fn patch_tags(&mut self, id: TrackId, p: &TagPatch) -> bool {
        let Ok(i) = self.tracks.binary_search_by_key(&id, |t| t.id) else { return false };
        let t = &mut self.tracks[i];
        let text = |slot: &mut String, v: &Option<String>| {
            if let Some(v) = v {
                *slot = v.clone();
            }
        };
        text(&mut t.title, &p.title);
        text(&mut t.artist, &p.artist);
        text(&mut t.album, &p.album);
        text(&mut t.album_artist, &p.album_artist);
        text(&mut t.genre, &p.genre);
        if let Some(n) = p.track_no {
            t.track_no = n;
        }
        if let Some(n) = p.track_total {
            t.track_total = n;
        }
        if let Some(n) = p.disc_no {
            t.disc_no = n;
        }
        if let Some(n) = p.disc_total {
            t.disc_total = n;
        }
        if let Some(y) = p.year {
            t.year = y;
        }
        self.dirty = true;
        self.rebuild();
        true
    }

    /// Counts up whenever something the views show changed.
    pub fn revision(&self) -> u64 {
        self.rev
    }

    /// Folders the library was built from.
    pub fn roots(&self) -> &[Root] {
        &self.roots
    }

    /// Every remembered file, including the ones that could not be read.
    pub fn all_tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Number of tracks that are shown.
    pub fn track_count(&self) -> usize {
        self.visible.len()
    }

    /// The track with `id`.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.binary_search_by_key(&id, |t| t.id).ok().map(|i| &self.tracks[i])
    }

    /// Albums, by artist, year and title.
    pub fn albums(&self) -> &[Album] {
        &self.albums
    }

    /// The album with `id`.
    pub fn album(&self, id: u32) -> Option<&Album> {
        self.album_ix.get(&id).map(|&i| &self.albums[i])
    }

    /// Artists by name.
    pub fn artists(&self) -> &[Artist] {
        &self.artists
    }

    /// The artist with `id`.
    pub fn artist(&self, id: u32) -> Option<&Artist> {
        self.artist_ix.get(&id).map(|&i| &self.artists[i])
    }

    /// The album a track belongs to.
    pub fn album_of(&self, id: TrackId) -> Option<&Album> {
        self.track_album.get(&id).map(|&i| &self.albums[i])
    }

    /// The loudness of an album as a whole, LUFS: what the tags of its tracks state, or, when every track's loudness is known, the
    /// duration-weighted mean of their energies (what the album would measure as one programme, without the gating of the joins).
    /// `None` while any track is still without a figure.
    pub fn album_loudness(&self, album: u32) -> Option<f32> {
        let a = self.album(album)?;
        let tracks: Vec<&Track> = a.tracks.iter().filter_map(|&id| self.track(id)).collect();
        if let Some(l) = tracks.iter().find_map(|t| t.loudness.album_lufs) {
            return Some(l);
        }
        let mut parts = Vec::with_capacity(tracks.len());
        for t in &tracks {
            parts.push((t.loudness.lufs?, t.duration_us.max(1)));
        }
        rvp_core::loudness::combine_lufs(&parts)
    }

    /// What the player is told about how loud track `id` is: its own figure and its album's.
    pub fn loudness_hint(&self, id: TrackId) -> Option<rvp_core::LoudnessTags> {
        let t = self.track(id)?;
        let album = self.album_of(id).and_then(|a| self.album_loudness(a.id));
        (t.loudness.lufs.is_some() || album.is_some()).then(|| rvp_core::LoudnessTags {
            track_lufs: t.loudness.lufs,
            album_lufs: album,
            ..Default::default()
        })
    }

    /// The thumbnail of picture `art`.
    ///
    /// Thumbnails are kept in a cache with a byte budget ([`Library::set_thumb_budget`]): one that is not in memory returns
    /// `None` and shows up in [`Library::wanted_art`] to be loaded again from storage.
    pub fn thumb(&self, art: ArtId) -> Option<&Thumb> {
        self.thumbs.get(art)
    }

    /// Keep a thumbnail (it will be saved with the next [`Library::take_unsaved_art`]).
    pub fn insert_thumb(&mut self, art: ArtId, thumb: Thumb) {
        self.thumbs.insert_new(art, thumb);
    }

    /// How much memory the thumbnails may use, in bytes (0 for the default, 32 MiB). Least recently used ones are dropped
    /// from memory first; they stay in storage and come back when a view asks for them.
    pub fn set_thumb_budget(&mut self, bytes: usize) {
        self.thumbs.set_budget(bytes);
    }

    /// Bytes of thumbnails in memory now.
    pub fn thumb_bytes(&self) -> usize {
        self.thumbs.resident_bytes()
    }

    /// Thumbnails in memory now.
    pub fn thumbs_resident(&self) -> usize {
        self.thumbs.len_resident()
    }

    /// Thumbnails dropped from memory to stay in budget since the library was created.
    pub fn thumb_evictions(&self) -> u64 {
        self.thumbs.evictions
    }

    /// True when thumbnails are over budget only because some are not saved yet: save them (the host can then drop them).
    pub fn thumbs_need_save(&self) -> bool {
        self.thumbs.needs_save()
    }

    /// Total length of everything shown, microseconds.
    pub fn total_duration_us(&self) -> i64 {
        self.visible.iter().map(|&i| self.tracks[i].duration_us).sum()
    }

    /// All shown tracks in the given order.
    pub fn sorted_tracks(&self, by: TrackSort, ascending: bool) -> Vec<TrackId> {
        let mut keyed: Vec<(String, TrackId)> = self
            .visible
            .iter()
            .map(|&i| {
                let t = &self.tracks[i];
                let key = match by {
                    TrackSort::Title => format!("{}\u{0}{}", natural(&fold(t.display_title())), t.id),
                    TrackSort::Artist => format!(
                        "{}\u{0}{}\u{0}{:05}{:05}\u{0}{}",
                        sort_key(t.display_artist()),
                        natural(&fold(t.display_album())),
                        t.disc_no,
                        t.track_no,
                        natural(&fold(t.display_title()))
                    ),
                    TrackSort::Album => format!(
                        "{}\u{0}{}\u{0}{:05}{:05}\u{0}{}",
                        natural(&fold(t.display_album())),
                        sort_key(t.display_artist()),
                        t.disc_no,
                        t.track_no,
                        natural(&fold(t.display_title()))
                    ),
                    TrackSort::Duration => format!("{:020}\u{0}{}", t.duration_us.max(0), t.id),
                    TrackSort::Year => {
                        format!("{:06}\u{0}{}", t.year.max(0), natural(&fold(t.display_title())))
                    }
                    TrackSort::Added => format!("{:010}", t.id),
                };
                (key, t.id)
            })
            .collect();
        keyed.sort();
        if !ascending {
            keyed.reverse();
        }
        keyed.into_iter().map(|(_, id)| id).collect()
    }

    /// Albums and tracks whose text contains every word of `query` (case and accents ignored).
    pub fn search(&self, query: &str) -> Search {
        let q = fold(query);
        let words: Vec<&str> = q.split_whitespace().collect();
        if words.is_empty() {
            return Search::default();
        }
        let has = |hay: &str| words.iter().all(|w| hay.contains(w));
        let mut tracks: Vec<TrackId> = Vec::new();
        for (n, &i) in self.visible.iter().enumerate() {
            if has(&self.hay[n]) {
                tracks.push(self.tracks[i].id);
            }
        }
        let albums = (0..self.albums.len())
            .filter(|&i| has(&format!("{} {}", fold(&self.albums[i].title), fold(&self.albums[i].artist))))
            .collect();
        let artists = (0..self.artists.len()).filter(|&i| has(&fold(&self.artists[i].name))).collect();
        let videos = self.search_videos(&words);
        Search { tracks, albums, artists, videos }
    }

    /// Recompute the albums, artists and search text after the tracks changed.
    pub fn rebuild(&mut self) {
        self.rev += 1;
        self.visible = (0..self.tracks.len()).filter(|&i| !self.tracks[i].unreadable).collect();
        self.hay = self
            .visible
            .iter()
            .map(|&i| {
                let t = &self.tracks[i];
                fold(&format!(
                    "{} {} {} {} {}",
                    t.display_title(),
                    t.display_artist(),
                    t.album_artist,
                    t.display_album(),
                    t.genre
                ))
            })
            .collect();
        self.by_file.clear();
        for &i in &self.visible {
            let t = &self.tracks[i];
            self.by_file.entry(fold(t.file_name())).or_default().push(t.id);
        }

        // Group tracks into albums.
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for &i in &self.visible {
            let t = &self.tracks[i];
            let key = if t.album.is_empty() {
                let who = if !t.album_artist.is_empty() { &t.album_artist } else { &t.artist };
                format!("\u{0}unknown\u{0}{}", fold(who))
            } else if !t.album_artist.is_empty() {
                format!("{}\u{0}aa:{}", fold(&t.album), fold(&t.album_artist))
            } else {
                format!("{}\u{0}dir:{}:{}", fold(&t.album), t.root, fold(t.dir()))
            };
            groups.entry(key).or_default().push(i);
        }
        let mut albums: Vec<Album> = Vec::with_capacity(groups.len());
        for (key, mut idx) in groups {
            idx.sort_by(|&a, &b| {
                let (x, y) = (&self.tracks[a], &self.tracks[b]);
                let kx = (
                    x.disc_no.max(1),
                    if x.track_no == 0 { u16::MAX } else { x.track_no },
                    natural(&fold(x.file_name())),
                );
                let ky = (
                    y.disc_no.max(1),
                    if y.track_no == 0 { u16::MAX } else { y.track_no },
                    natural(&fold(y.file_name())),
                );
                kx.cmp(&ky)
            });
            let first = &self.tracks[idx[0]];
            let tagged_aa = idx.iter().map(|&i| &self.tracks[i]).find(|t| !t.album_artist.is_empty());
            let artist: String = if let Some(t) = tagged_aa {
                t.album_artist.clone()
            } else {
                let mut names = idx.iter().map(|&i| fold(self.tracks[i].display_artist()));
                let head = names.next().unwrap_or_default();
                if names.all(|n| n == head) { first.display_artist().into() } else { VARIOUS_ARTISTS.into() }
            };
            let title: String =
                if first.album.is_empty() { UNKNOWN_ALBUM.into() } else { first.album.clone() };
            let year = idx.iter().map(|&i| self.tracks[i].year).find(|&y| y != 0).unwrap_or(0);
            let genre =
                idx.iter().map(|&i| self.tracks[i].genre.clone()).find(|g| !g.is_empty()).unwrap_or_default();
            let art = idx.iter().map(|&i| self.tracks[i].art).find(|&a| a != 0).unwrap_or(0);
            let discs = idx
                .iter()
                .map(|&i| self.tracks[i].disc_no.max(self.tracks[i].disc_total))
                .max()
                .unwrap_or(1)
                .max(1);
            albums.push(Album {
                id: hash32(&key),
                sort: natural(&fold(&title)),
                title,
                artist_id: hash32(&fold(&artist)),
                artist,
                year,
                genre,
                duration_us: idx.iter().map(|&i| self.tracks[i].duration_us).sum(),
                tracks: idx.iter().map(|&i| self.tracks[i].id).collect(),
                art,
                discs,
            });
        }
        albums.sort_by_cached_key(|a| {
            (sort_key(&a.artist), if a.year == 0 { i32::MAX } else { a.year }, a.sort.clone(), a.id)
        });
        let mut artists: Vec<Artist> = Vec::new();
        let mut artist_of: BTreeMap<u32, usize> = BTreeMap::new();
        for (ai, a) in albums.iter().enumerate() {
            let k = *artist_of.entry(a.artist_id).or_insert_with(|| {
                artists.push(Artist {
                    id: a.artist_id,
                    sort: sort_key(&a.artist),
                    name: a.artist.clone(),
                    albums: Vec::new(),
                    track_count: 0,
                    art: 0,
                });
                artists.len() - 1
            });
            artists[k].albums.push(ai);
            artists[k].track_count += a.tracks.len();
            if artists[k].art == 0 {
                artists[k].art = a.art;
            }
        }
        artists.sort_by(|a, b| a.sort.cmp(&b.sort).then(a.id.cmp(&b.id)));
        self.album_ix = albums.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
        self.track_album =
            albums.iter().enumerate().flat_map(|(i, a)| a.tracks.iter().map(move |&t| (t, i))).collect();
        self.albums = albums;
        self.artist_ix = artists.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
        self.artists = artists;
        self.resolve_playlists();
    }

    /// Ids of every picture the tracks use (the pictures worth keeping).
    pub fn used_art(&self) -> BTreeSet<ArtId> {
        self.tracks.iter().map(|t| t.art).chain(self.poster_art()).filter(|&a| a != 0).collect()
    }

    /// Thumbnails that were added since the last call, as `(id, thumbnail)`, to be saved.
    pub fn take_unsaved_art(&mut self) -> Vec<(ArtId, Thumb)> {
        self.thumbs.take_unsaved()
    }

    /// True if the index changed since [`Library::save_index`] last ran.
    pub fn index_dirty(&self) -> bool {
        self.dirty
    }

    /// True if the playlists changed since they were last saved.
    pub fn playlists_dirty(&self) -> bool {
        self.playlists_dirty
    }

    /// Forget a root and everything found under it.
    pub fn remove_root(&mut self, root_id: &str) {
        let Some(ri) = self.roots.iter().position(|r| r.id == root_id) else { return };
        self.tracks.retain(|t| t.root as usize != ri);
        for t in &mut self.tracks {
            if t.root as usize > ri {
                t.root -= 1;
            }
        }
        let old = core::mem::take(&mut self.folder_art);
        for ((r, d), v) in old {
            match (r as usize).cmp(&ri) {
                core::cmp::Ordering::Less => {
                    self.folder_art.insert((r, d), v);
                }
                core::cmp::Ordering::Greater => {
                    self.folder_art.insert((r - 1, d), v);
                }
                core::cmp::Ordering::Equal => {}
            }
        }
        self.remove_root_videos(ri);
        self.roots.remove(ri);
        self.dirty = true;
        self.rebuild();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use alloc::vec;

    pub(crate) fn track(id: u32, path: &str, artist: &str, album: &str, title: &str, no: u16) -> Track {
        Track {
            id,
            root: 0,
            path: path.into(),
            size: 1,
            mtime_ms: 1,
            title: title.into(),
            artist: artist.into(),
            album_artist: String::new(),
            album: album.into(),
            track_no: no,
            track_total: 0,
            disc_no: 0,
            disc_total: 0,
            year: 0,
            genre: String::new(),
            duration_us: 1_000_000,
            art: 0,
            art_embedded: false,
            codec: "mp3".into(),
            sample_rate: 44_100,
            channels: 2,
            unreadable: false,
            loudness: Default::default(),
            src: String::new(),
        }
    }

    pub(crate) fn lib_with(tracks: Vec<Track>) -> Library {
        let mut l = Library::new();
        l.roots.push(Root { id: "r".into(), name: "r".into(), connected: true });
        l.next_id = tracks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        l.tracks = tracks;
        l.rebuild();
        l
    }

    #[test]
    fn groups_albums_and_artists() {
        let l = lib_with(vec![
            track(1, "a/x/02 b.mp3", "The Owls", "Night", "B", 2),
            track(2, "a/x/01 a.mp3", "The Owls", "Night", "A", 1),
            track(3, "b/y/1.mp3", "Zed", "Night", "Z", 1),
            track(4, "c/loose.mp3", "", "", "", 0),
        ]);
        assert_eq!(
            l.albums().len(),
            3,
            "same album name in different folders and by different artists stays apart"
        );
        let night = l.albums().iter().find(|a| a.artist == "The Owls").unwrap();
        assert_eq!(night.tracks, [2, 1], "track order");
        let names: Vec<&str> = l.artists().iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["The Owls", "Unknown Artist", "Zed"], "'The' is ignored when sorting");
        let loose = l.albums().iter().find(|a| a.artist == UNKNOWN_ARTIST).unwrap();
        assert_eq!(loose.title, UNKNOWN_ALBUM);
        assert_eq!(l.track(4).unwrap().display_title(), "loose");
    }

    #[test]
    fn compilations_become_various_artists() {
        let l = lib_with(vec![
            track(1, "m/01.mp3", "A", "Mix", "x", 1),
            track(2, "m/02.mp3", "B", "Mix", "y", 2),
        ]);
        assert_eq!(l.albums().len(), 1);
        assert_eq!(l.albums()[0].artist, VARIOUS_ARTISTS);
    }

    #[test]
    fn search_folds_case_and_accents_and_needs_every_word() {
        let l = lib_with(vec![
            track(1, "a/1.mp3", "Zo\u{eb} P\u{e9}rez", "Coraz\u{f3}n", "Luz de Ne\u{f3}n", 1),
            track(2, "a/2.mp3", "Zo\u{eb} P\u{e9}rez", "Coraz\u{f3}n", "Mar", 2),
        ]);
        assert_eq!(l.search("zoe perez neon").tracks, [1]);
        assert_eq!(l.search("CORAZON").tracks, [1, 2]);
        assert_eq!(l.search("CORAZON").albums.len(), 1);
        assert_eq!(l.search("zoe").artists.len(), 1);
        assert!(l.search("   ").tracks.is_empty());
        assert!(l.search("nothing here").tracks.is_empty());
    }

    #[test]
    fn sorts_the_track_list() {
        let mut t1 = track(1, "a/1.mp3", "B", "X", "Beta", 1);
        t1.duration_us = 3;
        let mut t2 = track(2, "a/2.mp3", "A", "Y", "alpha", 1);
        t2.duration_us = 9;
        let mut t3 = track(3, "a/3.mp3", "C", "Z", "Track 10", 1);
        t3.duration_us = 1;
        let mut t4 = track(4, "a/4.mp3", "C", "Z", "Track 2", 2);
        t4.duration_us = 5;
        let l = lib_with(vec![t1, t2, t3, t4]);
        assert_eq!(l.sorted_tracks(TrackSort::Title, true), [2, 1, 4, 3], "alpha, Beta, Track 2, Track 10");
        assert_eq!(l.sorted_tracks(TrackSort::Artist, true), [2, 1, 3, 4]);
        assert_eq!(l.sorted_tracks(TrackSort::Duration, false), [2, 4, 1, 3]);
        assert_eq!(l.sorted_tracks(TrackSort::Added, false), [4, 3, 2, 1]);
    }

    #[test]
    fn unreadable_files_are_remembered_but_hidden() {
        let mut bad = track(2, "a/bad.mp3", "", "", "", 0);
        bad.unreadable = true;
        let l = lib_with(vec![track(1, "a/ok.mp3", "A", "B", "C", 1), bad]);
        assert_eq!(l.track_count(), 1);
        assert_eq!(l.all_tracks().len(), 2);
        assert_eq!(l.albums().len(), 1);
    }
}
