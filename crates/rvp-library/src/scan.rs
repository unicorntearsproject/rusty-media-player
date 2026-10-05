//! Scanning: deciding which files of a listing need reading (new, changed), reading their tags and cover art through the
//! demuxers, folding the results into the index, and picking up folder pictures (`cover.jpg`, `folder.png`).
//!
//! The pieces are small and synchronous so the application can run them from its own tick, a few at a time: [`Library::begin_scan`]
//! returns the files to read, [`read_tags`] reads one (async over a host `Source`), [`Library::apply_tags`] files the result,
//! and [`Library::finish_scan`] settles folder pictures and rebuilds the views.
use crate::art;
use crate::fold::hash64;
use crate::index::{FolderArt, Library};
use crate::model::*;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_core::{Art, Error, StreamKind};
use rvp_demux::Demuxer;
use rvp_host::{FileEntry, Source};

/// File name extensions of what the library indexes.
pub const AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "flac", "ogg", "oga", "opus", "wav", "m4a", "m4b", "aac", "mka"];
/// Names (without extension) of pictures that stand for a folder's cover, best first.
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "albumart", "album", "art"];
const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];
/// Folder pictures bigger than this are ignored.
const MAX_FOLDER_ART: u64 = 16 << 20;

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// True for a file the library indexes.
pub fn is_audio_name(name: &str) -> bool {
    AUDIO_EXTENSIONS.contains(&extension(name).as_str())
}

/// How good a folder picture a file name is: lower is better, `None` if it is not one.
fn cover_rank(file_name: &str) -> Option<usize> {
    let (stem, ext) = file_name.rsplit_once('.')?;
    if !COVER_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) {
        return None;
    }
    let stem = stem.to_ascii_lowercase();
    COVER_NAMES.iter().position(|n| *n == stem)
}

/// What the tags and headers of one file say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackTags {
    /// Title.
    pub title: String,
    /// Artist.
    pub artist: String,
    /// Album artist.
    pub album_artist: String,
    /// Album.
    pub album: String,
    /// Track number.
    pub track_no: u16,
    /// Tracks on the disc.
    pub track_total: u16,
    /// Disc number.
    pub disc_no: u16,
    /// Discs.
    pub disc_total: u16,
    /// Year.
    pub year: i32,
    /// Genre.
    pub genre: String,
    /// Length in microseconds.
    pub duration_us: i64,
    /// Codec name.
    pub codec: String,
    /// Sample rate.
    pub sample_rate: u32,
    /// Channels.
    pub channels: u16,
    /// The embedded cover picture, encoded.
    pub art: Option<Art>,
}

/// Read the tags, duration and cover art of an audio file. MP3 files are not walked to their end (see
/// `rvp_demux::open_quick`).
pub async fn read_tags<S: Source>(src: S) -> Result<TrackTags, Error> {
    let d = rvp_demux::open_quick(src).await?;
    let audio = d
        .streams()
        .iter()
        .find(|s| s.kind == StreamKind::Audio)
        .ok_or_else(|| Error::Unsupported("no audio stream".to_string()))?
        .clone();
    let m = d.metadata().clone();
    let clamp = |v: Option<u32>| v.map_or(0, |v| v.min(u16::MAX as u32) as u16);
    Ok(TrackTags {
        title: m.title.unwrap_or_default(),
        artist: m.artist.unwrap_or_default(),
        album_artist: m.album_artist.unwrap_or_default(),
        album: m.album.unwrap_or_default(),
        track_no: clamp(m.track),
        track_total: clamp(m.track_total),
        disc_no: clamp(m.disc),
        disc_total: clamp(m.disc_total),
        year: m.year.unwrap_or(0),
        genre: m.genre.unwrap_or_default(),
        duration_us: d.duration_us().or(audio.duration_us).unwrap_or(0).max(0),
        codec: audio.codec.clone(),
        sample_rate: audio.audio.map_or(0, |a| a.sample_rate),
        channels: audio.audio.map_or(0, |a| a.channels),
        art: m.art,
    })
}

/// Read a whole source (a folder picture, a playlist file), at most `max` bytes.
pub async fn read_all<S: Source>(mut src: S, max: usize) -> Result<Vec<u8>, Error> {
    let size = src.size().await.unwrap_or(0) as usize;
    if size > max {
        return Err(Error::Invalid("file is too big".to_string()));
    }
    let mut out = alloc::vec![0u8; size];
    let mut at = 0;
    while at < size {
        let n = src.read_at(at as u64, &mut out[at..]).await?;
        if n == 0 {
            break;
        }
        at += n;
    }
    out.truncate(at);
    Ok(out)
}

/// What [`Library::begin_scan`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanPlan {
    /// Index of the root in [`Library::roots`].
    pub root: u16,
    /// Files to read: new ones and ones whose size or time changed.
    pub read: Vec<FileEntry>,
}

impl Library {
    /// Start folding a listing of root `root_id` into the index. Files that are unchanged are left alone (they only learn
    /// how to be opened in this session), tracks whose file is gone are dropped, and the files to read are returned.
    pub fn begin_scan(&mut self, root_id: &str, name: &str, files: &[FileEntry]) -> ScanPlan {
        let ri = match self.roots.iter().position(|r| r.id == root_id) {
            Some(i) => i,
            None => {
                self.roots.push(Root { id: root_id.to_string(), name: name.to_string(), connected: true });
                self.roots.len() - 1
            }
        };
        self.roots[ri].connected = true;
        self.roots[ri].name = name.to_string();
        let ri16 = ri as u16;
        self.report = Default::default();
        // The tracks of this root, by path.
        let mut known: BTreeMap<String, usize> = BTreeMap::new();
        for (i, t) in self.tracks.iter().enumerate() {
            if t.root == ri16 {
                known.insert(t.path.clone(), i);
            }
        }
        let mut read = Vec::new();
        let mut pictures: BTreeMap<String, (usize, &FileEntry)> = BTreeMap::new();
        for f in files {
            let file_name = f.path.rsplit('/').next().unwrap_or(&f.path);
            if is_audio_name(file_name) {
                match known.remove(&f.path) {
                    Some(i) => {
                        let t = &mut self.tracks[i];
                        t.src = f.id.clone();
                        if t.size != f.size || t.mtime_ms != f.mtime_ms {
                            read.push(f.clone());
                        } else {
                            self.report.unchanged += 1;
                        }
                    }
                    None => read.push(f.clone()),
                }
            } else if let Some(rank) = cover_rank(file_name) {
                if f.size <= MAX_FOLDER_ART {
                    let dir = f.path.rsplit_once('/').map_or("", |(d, _)| d).to_string();
                    if pictures.get(&dir).is_none_or(|(r, _)| rank < *r) {
                        pictures.insert(dir, (rank, f));
                    }
                }
            }
        }
        // Tracks that were not listed are gone.
        let gone: Vec<usize> = known.into_values().collect();
        if !gone.is_empty() {
            self.report.removed = gone.len();
            let gone: alloc::collections::BTreeSet<usize> = gone.into_iter().collect();
            let mut i = 0;
            self.tracks.retain(|_| {
                let keep = !gone.contains(&i);
                i += 1;
                keep
            });
            self.dirty = true;
        }
        // Folder pictures: keep what is unchanged (with its picture), note what must be read.
        let mut next: BTreeMap<(u16, String), FolderArt> = BTreeMap::new();
        for (dir, (_, f)) in pictures {
            let old = self.folder_art.remove(&(ri16, dir.clone()));
            let fa = match old {
                Some(o) if o.path == f.path && o.size == f.size && o.mtime_ms == f.mtime_ms => {
                    FolderArt { id: f.id.clone(), ..o }
                }
                _ => FolderArt {
                    path: f.path.clone(),
                    size: f.size,
                    mtime_ms: f.mtime_ms,
                    art: 0,
                    read: false,
                    id: f.id.clone(),
                },
            };
            next.insert((ri16, dir), fa);
        }
        self.folder_art.retain(|(r, _), _| *r != ri16);
        self.folder_art.extend(next);
        ScanPlan { root: ri16, read }
    }

    /// File the result of reading `entry` (a file of root `root`).
    pub fn apply_tags(&mut self, root: u16, entry: &FileEntry, result: Result<TrackTags, Error>) {
        let existing = self.tracks.iter().position(|t| t.root == root && t.path == entry.path);
        let id = match existing {
            Some(i) => self.tracks[i].id,
            None => {
                let id = self.next_id.max(1);
                self.next_id = id + 1;
                id
            }
        };
        let mut t = Track {
            id,
            root,
            path: entry.path.clone(),
            size: entry.size,
            mtime_ms: entry.mtime_ms,
            title: String::new(),
            artist: String::new(),
            album_artist: String::new(),
            album: String::new(),
            track_no: 0,
            track_total: 0,
            disc_no: 0,
            disc_total: 0,
            year: 0,
            genre: String::new(),
            duration_us: 0,
            art: 0,
            art_embedded: false,
            codec: String::new(),
            sample_rate: 0,
            channels: 0,
            unreadable: false,
            src: entry.id.clone(),
        };
        match result {
            Ok(tags) => {
                t.title = tags.title;
                t.artist = tags.artist;
                t.album_artist = tags.album_artist;
                t.album = tags.album;
                t.track_no = tags.track_no;
                t.track_total = tags.track_total;
                t.disc_no = tags.disc_no;
                t.disc_total = tags.disc_total;
                t.year = tags.year;
                t.genre = tags.genre;
                t.duration_us = tags.duration_us;
                t.codec = tags.codec;
                t.sample_rate = tags.sample_rate;
                t.channels = tags.channels;
                if let Some(a) = tags.art {
                    let aid = self.keep_picture(&a.data);
                    if aid != 0 {
                        t.art = aid;
                        t.art_embedded = true;
                    }
                }
            }
            Err(_) => {
                t.unreadable = true;
                self.report.failed += 1;
            }
        }
        match existing {
            Some(i) => {
                self.tracks[i] = t;
                self.report.changed += 1;
            }
            None => {
                self.tracks.push(t);
                self.report.added += 1;
            }
        }
        self.dirty = true;
    }

    /// Decode an encoded picture to a thumbnail (once per distinct picture) and return its id (0 if it cannot be decoded).
    fn keep_picture(&mut self, data: &[u8]) -> ArtId {
        let id = hash64(data);
        if id == 0 {
            return 1;
        }
        if self.thumbs.known(id) {
            return id;
        }
        match art::thumbnail(data) {
            Some(t) => {
                self.insert_thumb(id, t);
                id
            }
            None => 0,
        }
    }

    /// Folder pictures that still have to be read: `(root, directory, id to open, size)`. Only directories that have a track
    /// without an embedded picture are worth the read.
    pub fn pending_folder_art(&self) -> Vec<(u16, String, String, u64)> {
        let mut out = Vec::new();
        for ((root, dir), fa) in &self.folder_art {
            if fa.read || fa.id.is_empty() {
                continue;
            }
            let needed = self.tracks.iter().any(|t| {
                t.root == *root
                    && !t.art_embedded
                    && !t.unreadable
                    && self.folder_art_dir(t) == Some(dir.as_str())
            });
            if needed {
                out.push((*root, dir.clone(), fa.id.clone(), fa.size));
            }
        }
        out
    }

    /// The directory (of the track's own and its parents) whose folder picture serves the track.
    fn folder_art_dir(&self, t: &Track) -> Option<&str> {
        let mut dir = t.dir();
        loop {
            if let Some(((_, d), _)) = self.folder_art.get_key_value(&(t.root, dir.to_string())) {
                return Some(d.as_str());
            }
            if dir.is_empty() {
                return None;
            }
            dir = dir.rsplit_once('/').map_or("", |(p, _)| p);
        }
    }

    /// Hand over the bytes of a folder picture `pending_folder_art` named (`None` if it could not be read).
    pub fn set_folder_art(&mut self, root: u16, dir: &str, bytes: Option<&[u8]>) {
        let id = bytes.map_or(0, |b| self.keep_picture(b));
        if let Some(fa) = self.folder_art.get_mut(&(root, dir.to_string())) {
            fa.art = id;
            fa.read = true;
            self.report.folder_art_read += 1;
            self.dirty = true;
        }
    }

    /// Everything that belongs to the scan is in: give tracks without a picture of their own the folder's, forget pictures
    /// nobody uses and rebuild the albums.
    pub fn finish_scan(&mut self) {
        let mut set: Vec<(usize, ArtId)> = Vec::new();
        for (i, t) in self.tracks.iter().enumerate() {
            if t.art_embedded || t.unreadable {
                continue;
            }
            let fa = self
                .folder_art_dir(t)
                .and_then(|d| self.folder_art.get(&(t.root, d.to_string())))
                .map_or(0, |f| f.art);
            if fa != t.art {
                set.push((i, fa));
            }
        }
        for (i, a) in set {
            self.tracks[i].art = a;
            self.dirty = true;
        }
        let used = self.used_art();
        let unused: Vec<ArtId> = self.thumbs.ids().into_iter().filter(|k| !used.contains(k)).collect();
        for k in unused {
            self.thumbs.remove(k);
        }
        self.rebuild();
    }

    /// Roots the host cannot read this session (their tracks stay in the index but cannot be played).
    pub fn set_connected(&mut self, connected: &[String]) {
        for r in &mut self.roots {
            r.connected = connected.contains(&r.id);
        }
        for t in &mut self.tracks {
            if !self.roots.get(t.root as usize).is_some_and(|r| r.connected) {
                t.src.clear();
            }
        }
    }

    /// Ids of pictures that were dropped since the last call (their saved copies can go).
    pub fn take_dropped_art(&mut self) -> Vec<ArtId> {
        self.thumbs.take_dropped()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvp_host::mock::MemSource;

    fn entry(path: &str, size: u64, mtime: i64) -> FileEntry {
        FileEntry { id: alloc::format!("id:{path}"), path: path.to_string(), size, mtime_ms: mtime }
    }

    fn tags(title: &str, album: &str) -> TrackTags {
        TrackTags {
            title: title.into(),
            album: album.into(),
            artist: "A".into(),
            duration_us: 1_000_000,
            codec: "mp3".into(),
            ..Default::default()
        }
    }

    #[test]
    fn picks_audio_and_cover_names() {
        assert!(
            is_audio_name("a.MP3")
                && is_audio_name("x.m4a")
                && !is_audio_name("a.mp4")
                && !is_audio_name("notes.txt")
        );
        assert!(cover_rank("Cover.JPG").unwrap() < cover_rank("front.png").unwrap());
        assert_eq!(cover_rank("photo.jpg"), None);
        assert_eq!(cover_rank("cover.gif"), None);
    }

    #[test]
    fn a_rescan_reads_only_what_changed() {
        let mut l = Library::new();
        let first = [
            entry("d/a.mp3", 10, 1),
            entry("d/b.mp3", 20, 1),
            entry("d/c.flac", 30, 1),
            entry("d/readme.txt", 5, 1),
        ];
        let plan = l.begin_scan("r", "Music", &first);
        assert_eq!(plan.read.len(), 3);
        for e in &plan.read {
            l.apply_tags(plan.root, e, Ok(tags(&e.path, "Alb")));
        }
        l.finish_scan();
        assert_eq!((l.report.added, l.report.changed, l.report.removed), (3, 0, 0));
        assert_eq!(l.track_count(), 3);
        let id_b = l.all_tracks().iter().find(|t| t.path == "d/b.mp3").unwrap().id;

        // b changes, c is deleted, e is new, a is untouched (with a new session id).
        let second = [
            FileEntry { id: "new-a".into(), ..entry("d/a.mp3", 10, 1) },
            entry("d/b.mp3", 21, 2),
            entry("d/e.ogg", 40, 1),
        ];
        let plan = l.begin_scan("r", "Music", &second);
        let mut paths: Vec<&str> = plan.read.iter().map(|e| e.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["d/b.mp3", "d/e.ogg"], "only the changed and the new file are read");
        for e in &plan.read {
            l.apply_tags(plan.root, e, Ok(tags(&e.path, "Alb")));
        }
        l.finish_scan();
        assert_eq!((l.report.added, l.report.changed, l.report.removed, l.report.unchanged), (1, 1, 1, 1));
        assert_eq!(l.track_count(), 3);
        assert_eq!(
            l.all_tracks().iter().find(|t| t.path == "d/b.mp3").unwrap().id,
            id_b,
            "ids survive a rescan"
        );
        assert_eq!(l.all_tracks().iter().find(|t| t.path == "d/a.mp3").unwrap().src, "new-a");
        assert!(l.all_tracks().iter().all(|t| t.path != "d/c.flac"));
    }

    #[test]
    fn unreadable_files_are_not_read_again() {
        let mut l = Library::new();
        let files = [entry("bad.mp3", 3, 1)];
        let plan = l.begin_scan("r", "M", &files);
        l.apply_tags(plan.root, &plan.read[0], Err(Error::Invalid("x".into())));
        l.finish_scan();
        assert_eq!((l.report.failed, l.track_count()), (1, 0));
        assert!(l.begin_scan("r", "M", &files).read.is_empty());
    }

    #[test]
    fn folder_pictures_serve_tracks_without_their_own() {
        let mut l = Library::new();
        let files = [
            entry("alb/01.mp3", 10, 1),
            entry("alb/cover.jpg", 99, 1),
            entry("alb/photo.jpg", 5, 1),
            entry("other/02.mp3", 1, 1),
        ];
        let plan = l.begin_scan("r", "M", &files);
        for e in &plan.read {
            l.apply_tags(plan.root, e, Ok(tags("t", "x")));
        }
        let pending = l.pending_folder_art();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1, "alb");
        // Not a decodable picture: the folder is marked read and the tracks stay without art.
        l.set_folder_art(pending[0].0, "alb", Some(b"junk"));
        l.finish_scan();
        assert!(l.pending_folder_art().is_empty());
        assert!(l.all_tracks().iter().all(|t| t.art == 0));
        // The next scan with the same picture does not ask for it again.
        let plan = l.begin_scan("r", "M", &files);
        assert!(plan.read.is_empty());
        assert!(l.pending_folder_art().is_empty());
    }

    #[test]
    fn reads_tags_from_a_non_audio_source_as_an_error() {
        let src = MemSource::new(alloc::vec![0u8; 64]);
        let r = rvp_core::task::block_on(read_tags(src));
        assert!(r.is_err());
    }
}
