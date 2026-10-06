//! Writing tags into audio files, without touching the audio.
//!
//! One function does the work: [`edit_tags`] takes the whole file and an [`Edit`] (the fields to set or clear; anything not named stays
//! as it is) and returns the whole new file. It never changes a byte of the audio, keeps every tag, frame, atom and picture it does
//! not edit, and refuses a file it cannot follow rather than guess. Writing it out safely (a temporary file and an atomic rename) is the
//! host's job.
//!
//! Formats: MP3 (ID3v2.4; an ID3v2.2 or v2.3 tag is upgraded), FLAC, Ogg Vorbis and Opus (Vorbis comments, the cover as a
//! `METADATA_BLOCK_PICTURE`) and MP4 / M4A (`ilst` atoms, with the chunk offsets moved when the movie box grows).
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod b64;
mod flac;
mod id3;
mod mp4;
mod ogg;
mod vorbis;

use alloc::string::String;
use alloc::vec::Vec;

/// The biggest file that is edited (it is held in memory, twice).
pub const MAX_FILE: usize = 1 << 30;

/// What to do with one field.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Change<T> {
    /// Leave it as the file has it.
    #[default]
    Keep,
    /// Remove it.
    Clear,
    /// Write this.
    Set(T),
}

impl<T> Change<T> {
    /// True for [`Change::Keep`].
    pub fn is_keep(&self) -> bool {
        matches!(self, Change::Keep)
    }
}

/// A cover picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cover {
    /// `image/jpeg` or `image/png`.
    pub mime: String,
    /// The encoded picture.
    pub data: Vec<u8>,
}

/// The fields that can be edited. Numbers of 0 are written as "not set".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Edit {
    /// Title.
    pub title: Change<String>,
    /// Artist.
    pub artist: Change<String>,
    /// Album.
    pub album: Change<String>,
    /// Album artist.
    pub album_artist: Change<String>,
    /// Track number (within the disc).
    pub track_no: Change<u32>,
    /// Tracks on the disc.
    pub track_total: Change<u32>,
    /// Disc number.
    pub disc_no: Change<u32>,
    /// Discs in the set.
    pub disc_total: Change<u32>,
    /// Year.
    pub year: Change<i32>,
    /// Genre, as text.
    pub genre: Change<String>,
    /// The cover: replaced, or removed (every picture in the file goes).
    pub cover: Change<Cover>,
}

impl Edit {
    /// True when nothing is to change.
    pub fn is_empty(&self) -> bool {
        self.title.is_keep()
            && self.artist.is_keep()
            && self.album.is_keep()
            && self.album_artist.is_keep()
            && self.track_no.is_keep()
            && self.track_total.is_keep()
            && self.disc_no.is_keep()
            && self.disc_total.is_keep()
            && self.year.is_keep()
            && self.genre.is_keep()
            && self.cover.is_keep()
    }
}

/// Why a file was not edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// This kind of file (or this way of making it) is not handled; the text says what.
    Unsupported(&'static str),
    /// The file is damaged in a way that makes editing it unsafe.
    Corrupt(&'static str),
    /// Too big to hold in memory.
    TooLarge,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Unsupported(s) => write!(f, "this file cannot be edited: {s}"),
            Error::Corrupt(s) => write!(f, "the file looks damaged ({s}), so it was left alone"),
            Error::TooLarge => write!(f, "the file is too big to edit"),
        }
    }
}

/// The kinds of file whose tags can be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// MP3 (ID3v2).
    Mp3,
    /// FLAC.
    Flac,
    /// Ogg Vorbis or Ogg Opus.
    Ogg,
    /// MP4 and M4A.
    Mp4,
}

/// The format of a file, from its name and its first bytes (the bytes win; the name only breaks the tie for a bare MP3 stream).
pub fn format_of(name: &str, head: &[u8]) -> Option<Format> {
    if head.starts_with(b"fLaC") {
        return Some(Format::Flac);
    }
    if head.starts_with(b"OggS") {
        return Some(Format::Ogg);
    }
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return Some(Format::Mp4);
    }
    if head.starts_with(b"ID3") {
        // An ID3 tag in front of FLAC (some taggers do that): the name says which.
        let lower = name.to_ascii_lowercase();
        return if lower.ends_with(".flac") { Some(Format::Flac) } else { Some(Format::Mp3) };
    }
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".mp3") && head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0 {
        return Some(Format::Mp3);
    }
    None
}

/// The file with `edit` applied. The audio is byte for byte what it was.
pub fn edit_tags(format: Format, file: &[u8], edit: &Edit) -> Result<Vec<u8>, Error> {
    if file.len() > MAX_FILE {
        return Err(Error::TooLarge);
    }
    if let Change::Set(c) = &edit.cover {
        if c.data.is_empty() || !(c.mime == "image/jpeg" || c.mime == "image/png") {
            return Err(Error::Unsupported("the cover must be a JPEG or a PNG picture"));
        }
    }
    match format {
        Format::Mp3 => id3::edit(file, edit),
        Format::Flac => flac::edit(file, edit),
        Format::Ogg => ogg::edit(file, edit),
        Format::Mp4 => mp4::edit(file, edit),
    }
}

/// The track or disc number and total after an edit, `None` for "not set": what is there now, changed by what the edit says.
pub(crate) fn merged_pair(now: (u32, u32), no: &Change<u32>, total: &Change<u32>) -> Option<(u32, u32)> {
    let pick = |cur: u32, c: &Change<u32>| match c {
        Change::Keep => cur,
        Change::Clear => 0,
        Change::Set(v) => *v,
    };
    let (n, t) = (pick(now.0, no), pick(now.1, total));
    (n != 0 || t != 0).then_some((n, t))
}

/// `3` or `3/12` (a total without a number is `0/12`).
pub(crate) fn pair_text((n, t): (u32, u32)) -> String {
    use alloc::string::ToString;
    if t == 0 { n.to_string() } else { alloc::format!("{n}/{t}") }
}

/// `"3/12"` or `"3"` as (number, total), 0 for what is missing.
pub(crate) fn parse_pair(s: &str) -> (u32, u32) {
    let mut it = s.split('/');
    let n = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let t = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    (n, t)
}
