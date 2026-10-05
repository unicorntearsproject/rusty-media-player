//! The plain data of the library: roots, tracks, and the albums and artists derived from them.
use alloc::string::String;
use alloc::vec::Vec;

/// Id of a track: assigned when the file is first seen, kept across rescans and restarts.
pub type TrackId = u32;
/// Id of a cover picture: the 64-bit hash of its encoded bytes; 0 means none.
pub type ArtId = u64;

/// What to show when a tag is missing.
pub const UNKNOWN_ARTIST: &str = "Unknown Artist";
/// What to show when the album tag is missing.
pub const UNKNOWN_ALBUM: &str = "Unknown Album";
/// The album artist of a compilation.
pub const VARIOUS_ARTISTS: &str = "Various Artists";

/// A folder the library was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// Stable id (the host's).
    pub id: String,
    /// Name to show.
    pub name: String,
    /// Readable in this session (the host listed it since the app started).
    pub connected: bool,
}

/// What the library knows about how loud a track is, for the automatic level.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrackLoudness {
    /// Integrated loudness, LUFS: what the file's own ReplayGain or R128 tags say, or else what decoding the file measured.
    pub lufs: Option<f32>,
    /// `lufs` was measured by decoding the file (not read from tags).
    pub measured: bool,
    /// The loudness of the album as the file's tags state it, LUFS.
    pub album_lufs: Option<f32>,
    /// A measurement was tried and found nothing to measure (silence, or a file that does not decode): not tried again until the
    /// file changes.
    pub tried: bool,
}

/// One audio file.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    /// Stable id.
    pub id: TrackId,
    /// Index into [`crate::Library::roots`].
    pub root: u16,
    /// Path below the root, `/` separated.
    pub path: String,
    /// File size when it was read.
    pub size: u64,
    /// Modification time when it was read, milliseconds since the Unix epoch.
    pub mtime_ms: i64,
    /// Title tag (empty if missing).
    pub title: String,
    /// Artist tag (empty if missing).
    pub artist: String,
    /// Album artist tag (empty if missing).
    pub album_artist: String,
    /// Album tag (empty if missing).
    pub album: String,
    /// Track number (0 if missing).
    pub track_no: u16,
    /// Tracks on the disc, if the tag says (0 if not).
    pub track_total: u16,
    /// Disc number (0 if missing).
    pub disc_no: u16,
    /// Number of discs (0 if not said).
    pub disc_total: u16,
    /// Year (0 if missing).
    pub year: i32,
    /// Genre tag (empty if missing).
    pub genre: String,
    /// Length, microseconds (0 if unknown).
    pub duration_us: i64,
    /// Cover picture, embedded or from the folder; 0 if none.
    pub art: ArtId,
    /// The picture came from the file itself (not from a `cover.jpg` next to it).
    pub art_embedded: bool,
    /// Codec name (`mp3`, `flac`, ...).
    pub codec: String,
    /// Sample rate, Hz.
    pub sample_rate: u32,
    /// Channels.
    pub channels: u16,
    /// The file could not be read as audio; it is remembered (so it is not read again) but not shown.
    pub unreadable: bool,
    /// Loudness, from tags or measured.
    pub loudness: TrackLoudness,
    /// What the host opens this file with in this session; empty when its root is not connected.
    pub src: String,
}

impl Track {
    /// The file name without its directory.
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// The directory below the root (empty at the top).
    pub fn dir(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(d, _)| d)
    }

    /// The title to show: the tag, or the file name without its extension.
    pub fn display_title(&self) -> &str {
        if !self.title.is_empty() {
            return &self.title;
        }
        let f = self.file_name();
        match f.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 => stem,
            _ => f,
        }
    }

    /// The artist to show.
    pub fn display_artist(&self) -> &str {
        if self.artist.is_empty() { UNKNOWN_ARTIST } else { &self.artist }
    }

    /// The album to show.
    pub fn display_album(&self) -> &str {
        if self.album.is_empty() { UNKNOWN_ALBUM } else { &self.album }
    }
}

/// An album: the tracks that share an album name and album artist (or folder, when no album artist is tagged).
#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    /// Stable id (a hash of the grouping key).
    pub id: u32,
    /// Title to show.
    pub title: String,
    /// Album artist to show ("Various Artists" for a compilation).
    pub artist: String,
    /// Id of the artist entry.
    pub artist_id: u32,
    /// First year found (0 if none).
    pub year: i32,
    /// Genre of the first track with one.
    pub genre: String,
    /// Tracks in play order: disc, track number, then file name.
    pub tracks: Vec<TrackId>,
    /// Cover picture of the album (the first track's that has one).
    pub art: ArtId,
    /// Total length, microseconds.
    pub duration_us: i64,
    /// Number of discs (1 unless tags say more).
    pub discs: u16,
    /// Sort key of the album title.
    pub(crate) sort: String,
}

/// An artist (an album artist: a compilation's artist is "Various Artists").
#[derive(Debug, Clone, PartialEq)]
pub struct Artist {
    /// Stable id (a hash of the folded name).
    pub id: u32,
    /// Name to show.
    pub name: String,
    /// Indexes into [`crate::Library::albums`], by year then title.
    pub albums: Vec<usize>,
    /// Tracks over all albums.
    pub track_count: usize,
    /// Cover of the first album that has one.
    pub art: ArtId,
    pub(crate) sort: String,
}
