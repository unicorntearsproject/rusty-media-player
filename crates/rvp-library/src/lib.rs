//! The music library: what a scan finds (tracks with tags and cover art), the albums and artists derived from it, search,
//! saved playlists with M3U, M3U8 and PLS import and export, and how all of it is saved.
//!
//! Host-neutral and `no_std`: directory walking comes from the host (`rvp_host::Library`), reading files from
//! `rvp_host::Source` through `rvp-demux`, saving through `rvp_host::Storage`. The application (`rvp-app`) drives the
//! scan from its tick; the UI (`rvp-ui`) draws from [`Library`].
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod art;
pub mod fold;
mod index;
mod model;
mod persist;
mod plist;
mod scan;
mod scanner;

pub use art::{Image, THUMB_SIDE, Thumb};
pub use index::{Library, ScanReport, Search, TrackSort};
pub use model::{Album, ArtId, Artist, Root, Track, TrackId, UNKNOWN_ALBUM, UNKNOWN_ARTIST, VARIOUS_ARTISTS};
pub use persist::{INDEX_KEY, PLAYLISTS_KEY, art_key, decode_thumb, encode_thumb};
pub use plist::{ListFormat, PlEntry, SavedPlaylist};
pub use scan::{AUDIO_EXTENSIONS, ScanPlan, TrackTags, is_audio_name, read_all, read_tags};
pub use scanner::{ScanEvent, ScanStatus, Scanner};
