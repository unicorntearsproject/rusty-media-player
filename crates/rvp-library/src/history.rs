//! Play history: what was played, when, for how long and whether it finished. A play is kept by what identifies a file (the same key as
//! the favorites: folder name, file name, length), so it survives a rescan, a move of the whole tree and a tag edit. The history is
//! bounded ([`MAX_PLAYS`], oldest dropped first) and compact on disk (a table of keys, then 13 bytes a play).
//!
//! A play *counts* once it has been heard for [`COUNT_AFTER_US`] or for half of the item, whichever comes first: skipping through a list
//! does not fill the history. The application records a play when it starts to count, then updates the same play (by its `seq`) with the
//! time heard and whether it finished when the item ends or is left.
use crate::fold::{fold, natural};
use crate::index::Library;
use crate::model::Track;
use crate::video::Video;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Storage key of the history.
pub const HISTORY_KEY: &str = "library/history";
/// Most plays kept; the oldest go first.
pub const MAX_PLAYS: usize = 10_000;
/// A play counts after this much has been heard (or half of the item, if that is less).
pub const COUNT_AFTER_US: i64 = 30_000_000;

/// Whether `heard_us` of an item `duration_us` long (0 when unknown) counts as a play.
pub fn counts_as_play(heard_us: i64, duration_us: i64) -> bool {
    heard_us >= COUNT_AFTER_US || (duration_us > 0 && heard_us.saturating_mul(2) >= duration_us)
}

/// One play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Play {
    /// Position in the order plays were made (stable while the app runs; the oldest are dropped from the front).
    pub seq: u64,
    /// What identifies the file (see [`Track::favorite_key`]).
    pub key: String,
    /// A video (else a song).
    pub video: bool,
    /// When it started to count, seconds since 1970 (0 when the host has no calendar).
    pub at: i64,
    /// How long it was heard, milliseconds.
    pub heard_ms: u32,
    /// It played to the end (else it was skipped, stopped or the app closed).
    pub finished: bool,
}

/// A row of the history view: a play, and the library item it is (if the file is in the library now).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    /// The play.
    pub play: Play,
    /// The song or video in the library, if its file is there.
    pub id: Option<u32>,
    /// How many plays of the same file the history holds.
    pub count: u32,
}

/// The state of the history inside a library.
#[derive(Debug, Default, Clone)]
pub(crate) struct History {
    pub(crate) plays: Vec<Play>,
    pub(crate) next_seq: u64,
    pub(crate) dirty: bool,
    /// `key -> (plays, last played)`, kept in step with `plays`.
    pub(crate) counts: BTreeMap<String, (u32, i64)>,
}

impl History {
    pub(crate) fn recount(&mut self) {
        self.counts.clear();
        for p in &self.plays {
            let e = self.counts.entry(p.key.clone()).or_insert((0, 0));
            e.0 += 1;
            e.1 = e.1.max(p.at);
        }
    }
}

impl Library {
    fn key_and_kind(&self, id: u32) -> Option<(String, bool)> {
        self.track(id)
            .map(|t| (Track::favorite_key(t), false))
            .or_else(|| self.video(id).map(|v| (Video::favorite_key(v), true)))
    }

    /// Record a play of the song or video `id` that started to count at `at` (seconds since 1970). Returns its `seq`, or `None` when the
    /// library has no such item.
    pub fn record_play(&mut self, id: u32, at: i64, heard_ms: u32, finished: bool) -> Option<u64> {
        let (key, video) = self.key_and_kind(id)?;
        let seq = self.history.next_seq;
        self.history.next_seq += 1;
        let e = self.history.counts.entry(key.clone()).or_insert((0, 0));
        e.0 += 1;
        e.1 = e.1.max(at);
        self.history.plays.push(Play { seq, key, video, at, heard_ms, finished });
        if self.history.plays.len() > MAX_PLAYS {
            let drop = self.history.plays.len() - MAX_PLAYS;
            self.history.plays.drain(..drop);
            self.history.recount();
        }
        self.history.dirty = true;
        self.rev += 1;
        Some(seq)
    }

    /// Update a play made by [`Library::record_play`] with how long it was heard in the end and whether it finished.
    pub fn update_play(&mut self, seq: u64, heard_ms: u32, finished: bool) {
        if let Some(p) = self.history.plays.iter_mut().rev().find(|p| p.seq == seq) {
            if p.heard_ms != heard_ms || p.finished != finished {
                p.heard_ms = heard_ms;
                p.finished = finished;
                self.history.dirty = true;
            }
        }
    }

    /// How many times the song or video `id` was played.
    pub fn play_count(&self, id: u32) -> u32 {
        self.key_and_kind(id).and_then(|(k, _)| self.history.counts.get(&k).map(|c| c.0)).unwrap_or(0)
    }

    /// When the song or video `id` was last played (seconds since 1970), if ever.
    pub fn last_played(&self, id: u32) -> Option<i64> {
        self.key_and_kind(id).and_then(|(k, _)| self.history.counts.get(&k).map(|c| c.1))
    }

    /// How many plays the history holds.
    pub fn history_len(&self) -> usize {
        self.history.plays.len()
    }

    /// The plays, oldest first.
    pub fn plays(&self) -> &[Play] {
        &self.history.plays
    }

    /// The rows of the history of songs (`video` false) or videos, newest first.
    pub fn history_rows(&self, video: bool) -> Vec<HistoryRow> {
        let mut ids: BTreeMap<String, u32> = BTreeMap::new();
        if video {
            for v in self.all_videos().iter().filter(|v| !v.unreadable) {
                ids.insert(v.favorite_key(), v.id);
            }
        } else {
            for t in self.all_tracks().iter().filter(|t| !t.unreadable) {
                ids.insert(t.favorite_key(), t.id);
            }
        }
        self.history
            .plays
            .iter()
            .rev()
            .filter(|p| p.video == video)
            .map(|p| HistoryRow {
                id: ids.get(&p.key).copied(),
                count: self.history.counts.get(&p.key).map_or(1, |c| c.0),
                play: p.clone(),
            })
            .collect()
    }

    /// Forget one play. Returns whether there was one.
    pub fn remove_play(&mut self, seq: u64) -> bool {
        let before = self.history.plays.len();
        self.history.plays.retain(|p| p.seq != seq);
        let removed = self.history.plays.len() != before;
        if removed {
            self.history.recount();
            self.history.dirty = true;
            self.rev += 1;
        }
        removed
    }

    /// Forget every play of the file `id` is. Returns how many.
    pub fn remove_plays_of(&mut self, id: u32) -> usize {
        let Some((key, _)) = self.key_and_kind(id) else { return 0 };
        let before = self.history.plays.len();
        self.history.plays.retain(|p| p.key != key);
        let n = before - self.history.plays.len();
        if n > 0 {
            self.history.recount();
            self.history.dirty = true;
            self.rev += 1;
        }
        n
    }

    /// Forget everything.
    pub fn clear_history(&mut self) {
        if !self.history.plays.is_empty() {
            self.history.plays.clear();
            self.history.counts.clear();
            self.history.dirty = true;
            self.rev += 1;
        }
    }

    /// Tell the library the host's calendar: now, and the local offset from UTC (both seconds).
    pub fn set_calendar(&mut self, now: i64, utc_offset: i32) {
        self.calendar = (now, utc_offset);
    }

    /// How many local days ago `at` was (0 today, 1 yesterday), or `None` when the host has no calendar or `at` is unknown.
    pub fn days_ago(&self, at: i64) -> Option<i64> {
        let (now, off) = self.calendar;
        if now <= 0 || at <= 0 {
            return None;
        }
        let day = |t: i64| (t + off as i64).div_euclid(86_400);
        Some((day(now) - day(at)).max(0))
    }

    /// The local calendar date and time of `at`: (year, month, day, hour, minute).
    pub fn local_date(&self, at: i64) -> (i32, u32, u32, u32, u32) {
        let t = at + self.calendar.1 as i64;
        let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
        // Civil-from-days (proleptic Gregorian).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
        (y, m, d, (secs / 3600) as u32, (secs % 3600 / 60) as u32)
    }

    /// The history changed since it was last saved.
    pub fn history_dirty(&self) -> bool {
        self.history.dirty
    }

    /// The sort key of a song or video by its plays: the most played first when descending, ties by title.
    pub(crate) fn plays_sort_key(&self, id: u32, title: &str) -> String {
        let (n, _) =
            self.key_and_kind(id).and_then(|(k, _)| self.history.counts.get(&k).copied()).unwrap_or((0, 0));
        format!("{n:010}\u{0}{}\u{0}{id:010}", natural(&fold(title)))
    }

    /// The sort key by when last played (never played sorts first when ascending).
    pub(crate) fn last_played_sort_key(&self, id: u32, title: &str) -> String {
        let (_, at) =
            self.key_and_kind(id).and_then(|(k, _)| self.history.counts.get(&k).copied()).unwrap_or((0, 0));
        format!("{:020}\u{0}{}\u{0}{id:010}", at.max(0), natural(&fold(title)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::tests::{lib_with, track};
    use alloc::vec;

    fn lib() -> Library {
        lib_with(vec![
            track(1, "Artist/Album/01 One.mp3", "Artist", "Album", "One", 1),
            track(2, "Artist/Album/02 Two.mp3", "Artist", "Album", "Two", 2),
            track(3, "Other/Rec/01 One.mp3", "Other", "Rec", "One", 1),
        ])
    }

    #[test]
    fn a_play_counts_after_thirty_seconds_or_half_of_the_item() {
        assert!(!counts_as_play(29_000_000, 300_000_000));
        assert!(counts_as_play(30_000_000, 300_000_000));
        assert!(counts_as_play(10_000_000, 20_000_000), "half of a short clip");
        assert!(!counts_as_play(9_000_000, 20_000_000));
        assert!(counts_as_play(30_000_000, 0), "an unknown length: the 30 seconds");
        assert!(!counts_as_play(0, 0));
    }

    #[test]
    fn plays_are_recorded_counted_updated_and_forgotten() {
        let mut l = lib();
        let a = l.record_play(1, 1_000, 31_000, false).unwrap();
        l.record_play(1, 2_000, 200_000, true).unwrap();
        l.record_play(2, 3_000, 45_000, false).unwrap();
        assert_eq!(l.record_play(99, 4_000, 1, false), None, "no such item");
        assert_eq!((l.play_count(1), l.last_played(1)), (2, Some(2_000)));
        assert_eq!(
            (l.play_count(2), l.play_count(3)),
            (1, 0),
            "the same title in another folder is another file"
        );
        assert_eq!(l.last_played(3), None);
        // The first play is updated in place: heard longer, and it finished.
        l.update_play(a, 180_000, true);
        let rows = l.history_rows(false);
        assert_eq!(rows.iter().map(|r| r.play.at).collect::<Vec<_>>(), [3_000, 2_000, 1_000], "newest first");
        assert_eq!(rows[2].play.heard_ms, 180_000);
        assert!(rows[2].play.finished);
        assert_eq!((rows[1].count, rows[1].id), (2, Some(1)));
        assert!(l.history_rows(true).is_empty());
        // Forget one play, then every play of a file, then all.
        assert!(l.remove_play(rows[1].play.seq));
        assert!(!l.remove_play(rows[1].play.seq));
        assert_eq!(l.play_count(1), 1);
        assert_eq!(l.remove_plays_of(1), 1);
        assert_eq!(l.play_count(1), 0);
        assert_eq!(l.history_len(), 1);
        l.clear_history();
        assert_eq!(l.history_len(), 0);
        assert!(l.history_dirty());
    }

    #[test]
    fn the_history_is_bounded_and_drops_the_oldest() {
        let mut l = lib();
        for i in 0..(MAX_PLAYS as i64 + 25) {
            l.record_play(1 + (i % 2) as u32, i, 1_000, false);
        }
        assert_eq!(l.history_len(), MAX_PLAYS);
        assert_eq!(l.plays()[0].at, 25, "the first 25 went");
        assert_eq!(l.play_count(1) + l.play_count(2), MAX_PLAYS as u32);
    }

    #[test]
    fn the_history_round_trips_compactly_and_survives_a_rescan_by_the_files_identity() {
        let mut l = lib();
        for i in 0..200 {
            l.record_play(1 + (i % 3) as u32, 1_700_000_000 + i, 40_000 + i as u32, i % 2 == 0);
        }
        let bytes = l.save_history();
        assert!(!l.history_dirty());
        assert!(bytes.len() < 4 + 1 + 4 + 3 * 60 + 4 + 200 * 13, "{} bytes", bytes.len());
        // A new library with the same files under other ids (a rescan, a move of the tree) reads it back.
        let mut m = lib_with(vec![
            track(10, "Artist/Album/01 One.mp3", "Artist", "Album", "One", 1),
            track(11, "Artist/Album/02 Two.mp3", "Artist", "Album", "Two", 2),
            track(12, "Other/Rec/01 One.mp3", "Other", "Rec", "One", 1),
        ]);
        m.load_history(&bytes).unwrap();
        assert_eq!(m.history_len(), 200);
        assert_eq!(m.play_count(10) + m.play_count(11) + m.play_count(12), 200);
        assert_eq!(m.history_rows(false)[0].id, Some(11));
        // Bytes that are not a history, cut short, or pointing at a missing key are refused without changing anything.
        assert!(m.load_history(b"nope").is_err());
        assert!(m.load_history(&bytes[..bytes.len() - 5]).is_err());
        let mut bad = bytes.clone();
        bad[10] = 0xff;
        assert!(m.load_history(&bad).is_err() || m.history_len() == 200);
        assert_eq!(m.history_len(), 200);
    }

    #[test]
    fn songs_sort_by_plays_and_by_when_last_played() {
        let mut l = lib();
        l.record_play(2, 100, 40_000, true);
        l.record_play(2, 300, 40_000, true);
        l.record_play(3, 200, 40_000, true);
        let by_plays = l.sorted_tracks(crate::TrackSort::Plays, false);
        assert_eq!(by_plays[..2], [2, 3]);
        let by_last = l.sorted_tracks(crate::TrackSort::LastPlayed, false);
        assert_eq!(by_last[..2], [2, 3], "2 was played at 300");
        let oldest_first = l.sorted_tracks(crate::TrackSort::LastPlayed, true);
        assert_eq!(oldest_first[0], 1, "never played sorts first");
    }
}

#[cfg(test)]
mod calendar_tests {
    use crate::index::Library;

    #[test]
    fn dates_and_days_follow_the_local_offset() {
        let mut l = Library::default();
        // 2026-10-07 23:30 UTC is already the 8th at +02:00.
        let at = 1_791_415_800;
        l.set_calendar(at, 7200);
        assert_eq!(l.local_date(at), (2026, 10, 8, 1, 30));
        assert_eq!(l.days_ago(at), Some(0));
        assert_eq!(l.days_ago(at - 3600), Some(0));
        assert_eq!(l.days_ago(at - 2 * 3600), Some(1));
        assert_eq!(l.days_ago(at - 86_400), Some(1));
        l.set_calendar(at, -8 * 3600);
        assert_eq!(l.local_date(at), (2026, 10, 7, 15, 30));
        assert_eq!(l.local_date(86_400 * 59 + 12 * 3600), (1970, 3, 1, 4, 0));
        assert_eq!(l.days_ago(0), None);
    }

    #[test]
    fn no_calendar_means_no_days() {
        let l = Library::default();
        assert_eq!(l.days_ago(1_000_000), None);
    }
}
