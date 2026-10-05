//! What the UI shows: a plain-data snapshot the host application fills from the player each tick.
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

/// Coarse playback state for the chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaState {
    /// Nothing loaded.
    #[default]
    Idle,
    /// Opening a file.
    Opening,
    /// Loaded, paused.
    Paused,
    /// Playing, but waiting for data (start, after a seek).
    Buffering,
    /// Playing.
    Playing,
    /// Played to the end.
    Ended,
    /// The file could not be played.
    Failed,
}

impl MediaState {
    /// True while playback is running or about to run (controls auto-hide only then).
    pub fn is_active(self) -> bool {
        matches!(self, MediaState::Playing | MediaState::Buffering)
    }
}

/// An audio or subtitle track in a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackItem {
    /// Container track id.
    pub id: u32,
    /// Label, for example `English (aac)`.
    pub label: String,
}

/// A chapter mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterItem {
    /// Start, microseconds.
    pub start_us: i64,
    /// Title (may be empty).
    pub title: String,
}

/// One row of the playlist menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistEntry {
    /// Playlist item id.
    pub id: u32,
    /// Display name.
    pub label: String,
    /// The item being played.
    pub current: bool,
    /// A second line (artist and album) when the item has tags.
    pub subtitle: String,
    /// Length, microseconds (0 if unknown).
    pub duration_us: i64,
    /// The library track it came from.
    pub track: Option<u32>,
    /// Its cover picture in the library (0 if none).
    pub art: u64,
}

/// Everything the UI needs to draw and to build its menus.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UiModel {
    /// Playback state.
    pub state: MediaState,
    /// File name.
    pub title: String,
    /// Position in microseconds.
    pub position_us: i64,
    /// Duration in microseconds, if known.
    pub duration_us: Option<i64>,
    /// Volume 0.0..=1.0.
    pub volume: f32,
    /// Muted.
    pub muted: bool,
    /// Playback rate (1.0 = normal).
    pub rate: f32,
    /// Fullscreen on.
    pub fullscreen: bool,
    /// The file has a picture (not audio only).
    pub has_video: bool,
    /// Audio tracks.
    pub audio_tracks: Vec<TrackItem>,
    /// Selected audio track id.
    pub selected_audio: Option<u32>,
    /// Subtitle tracks.
    pub subtitle_tracks: Vec<TrackItem>,
    /// Selected subtitle track id (`None` = off).
    pub selected_subtitle: Option<u32>,
    /// The subtitle text on screen right now.
    pub subtitle: Option<String>,
    /// Chapter marks, in time order.
    pub chapters: Vec<ChapterItem>,
    /// The playlist (the queue), in list order. Shared so a model that is cloned every tick stays cheap.
    pub playlist: Rc<Vec<PlaylistEntry>>,
    /// Artist of what is playing (empty if unknown).
    pub artist: String,
    /// Album of what is playing (empty if unknown).
    pub album: String,
    /// Library track id of what is playing, if it came from the library.
    pub now_track: Option<u32>,
    /// Cover picture id (library thumbnail) of what is playing, 0 if none.
    pub now_art: u64,
    /// Changes whenever the queue's items, their order or the current item change.
    pub queue_rev: u64,
    /// Repeat mode: 0 off, 1 all, 2 one.
    pub repeat: u8,
    /// Shuffle on.
    pub shuffle: bool,
    /// A-B loop start, microseconds.
    pub loop_a: Option<i64>,
    /// A-B loop end, microseconds.
    pub loop_b: Option<i64>,
    /// Why playback failed, in the player's own voice.
    pub error: Option<String>,
}

impl UiModel {
    /// True once a file is open (or opening).
    pub fn has_media(&self) -> bool {
        !matches!(self.state, MediaState::Idle)
    }
}

/// `m:ss`, or `h:mm:ss` from one hour up.
pub fn format_time(us: i64) -> String {
    let total = (us.max(0) / 1_000_000) as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 { alloc::format!("{h}:{m:02}:{s:02}") } else { alloc::format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_time() {
        assert_eq!(format_time(0), "0:00");
        assert_eq!(format_time(61_900_000), "1:01");
        assert_eq!(format_time(3_725_000_000), "1:02:05");
        assert_eq!(format_time(-5), "0:00");
    }
}
