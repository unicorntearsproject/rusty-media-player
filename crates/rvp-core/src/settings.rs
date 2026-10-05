//! The user's audio settings (crossfade and automatic level) and the little text format they are saved in.
//!
//! The format is a few `key=value` lines under a version line, so a host can keep it in a plain file or a browser's
//! local storage and a person can read it. Unknown keys and values that do not parse are ignored (the default stays),
//! and numbers are clamped to their ranges, so a hand-edited or newer file never makes loading fail.
use alloc::string::String;

/// Storage key the application keeps [`AudioSettings`] under.
pub const SETTINGS_KEY: &str = "settings/audio";

/// What the automatic level follows: each track on its own, or the album as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LevelMode {
    /// Every track is brought to the target by itself.
    #[default]
    Track,
    /// Every track of an album gets the album's gain, so the quiet and loud tracks of one record stay as the artist left
    /// them; albums are brought to the target. Tracks that are not part of a known album fall back to their own loudness.
    Album,
}

/// Shortest crossfade, seconds.
pub const CROSSFADE_MIN_SECS: u8 = 2;
/// Longest crossfade, seconds.
pub const CROSSFADE_MAX_SECS: u8 = 10;
/// Quietest target level, LUFS.
pub const TARGET_MIN_LUFS: i8 = -23;
/// Loudest target level, LUFS.
pub const TARGET_MAX_LUFS: i8 = -10;

/// Crossfade and automatic level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSettings {
    /// Fade between consecutive tracks of the queue instead of playing them back to back.
    pub crossfade: bool,
    /// Length of the fade, seconds ([`CROSSFADE_MIN_SECS`] to [`CROSSFADE_MAX_SECS`]).
    pub crossfade_secs: u8,
    /// Bring everything to the same loudness.
    pub auto_level: bool,
    /// The loudness aimed at, LUFS ([`TARGET_MIN_LUFS`] to [`TARGET_MAX_LUFS`]).
    pub target_lufs: i8,
    /// Track or album.
    pub level_mode: LevelMode,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            crossfade: false,
            crossfade_secs: 5,
            auto_level: false,
            target_lufs: -14,
            level_mode: LevelMode::Track,
        }
    }
}

impl AudioSettings {
    /// The same settings with every number inside its range.
    pub fn clamped(mut self) -> Self {
        self.crossfade_secs = self.crossfade_secs.clamp(CROSSFADE_MIN_SECS, CROSSFADE_MAX_SECS);
        self.target_lufs = self.target_lufs.clamp(TARGET_MIN_LUFS, TARGET_MAX_LUFS);
        self
    }

    /// The text to save.
    pub fn to_text(&self) -> String {
        alloc::format!(
            "rvp-audio-settings 1\ncrossfade={}\ncrossfade_secs={}\nauto_level={}\ntarget_lufs={}\nlevel_mode={}\n",
            self.crossfade as u8,
            self.crossfade_secs,
            self.auto_level as u8,
            self.target_lufs,
            match self.level_mode {
                LevelMode::Track => "track",
                LevelMode::Album => "album",
            }
        )
    }

    /// Read saved text. `None` if it is not settings at all (no version line); otherwise whatever could be understood on top
    /// of the defaults.
    pub fn from_text(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if !lines.next()?.trim().starts_with("rvp-audio-settings") {
            return None;
        }
        let mut s = Self::default();
        for line in lines {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let flag = || match v {
                "1" | "true" | "on" => Some(true),
                "0" | "false" | "off" => Some(false),
                _ => None,
            };
            match k {
                "crossfade" => s.crossfade = flag().unwrap_or(s.crossfade),
                "auto_level" => s.auto_level = flag().unwrap_or(s.auto_level),
                "crossfade_secs" => s.crossfade_secs = v.parse().unwrap_or(s.crossfade_secs),
                "target_lufs" => s.target_lufs = v.parse().unwrap_or(s.target_lufs),
                "level_mode" => {
                    s.level_mode = match v {
                        "album" => LevelMode::Album,
                        "track" => LevelMode::Track,
                        _ => s.level_mode,
                    }
                }
                _ => {}
            }
        }
        Some(s.clamped())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let s = AudioSettings {
            crossfade: true,
            crossfade_secs: 8,
            auto_level: true,
            target_lufs: -18,
            level_mode: LevelMode::Album,
        };
        assert_eq!(AudioSettings::from_text(&s.to_text()), Some(s));
        let d = AudioSettings::default();
        assert_eq!(AudioSettings::from_text(&d.to_text()), Some(d));
    }

    #[test]
    fn damaged_or_newer_text_still_loads() {
        assert_eq!(AudioSettings::from_text(""), None);
        assert_eq!(AudioSettings::from_text("something else\ncrossfade=1\n"), None);
        let text = "rvp-audio-settings 7\ncrossfade=on\ncrossfade_secs=99\ntarget_lufs=-3\nfuture=1\nauto_level=maybe\nlevel_mode=x\n";
        let s = AudioSettings::from_text(text).unwrap();
        assert!(s.crossfade && !s.auto_level);
        assert_eq!((s.crossfade_secs, s.target_lufs, s.level_mode), (10, -10, LevelMode::Track));
        let s =
            AudioSettings::from_text("rvp-audio-settings 1\ncrossfade_secs=-4\ntarget_lufs=zz\n").unwrap();
        assert_eq!((s.crossfade_secs, s.target_lufs), (5, -14));
    }
}
