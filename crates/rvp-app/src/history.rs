//! Recording the play history: which library item is being heard, for how long, and how it ended. The rules (what counts as a play, how the
//! history is bounded and kept) are in `rvp_library::history`; this watches the playback and feeds them.
//!
//! A play is *recorded* the moment it counts (30 seconds heard, or half of the item); from then on it is updated, now and then while it plays
//! and for the last time when the item changes, stops or ends (`finished` says it played to the end). Heard time is real time spent playing, so
//! pausing, seeking and speed do not inflate it. With "Pause history" on, nothing new is recorded.
use super::App;
use rvp_core::Timestamp;
use rvp_host::{FrameSink, Host, Storage};
use rvp_library::counts_as_play;
use rvp_ui::MediaState;

/// How often a play in progress is brought up to date in the history, seconds of heard time.
const UPDATE_EVERY_US: i64 = 15_000_000;

/// What is being watched.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    /// The playlist item and the library item it plays.
    item: Option<(u32, u32)>,
    /// Real time spent playing it.
    heard_us: i64,
    /// Host time of the last look, while it was playing.
    last: Option<Timestamp>,
    /// The play in the history, once it counts.
    seq: Option<u64>,
    /// Heard time at the last update of the history.
    updated_at: i64,
    /// The position at the last look (a jump back after the end means it started again).
    last_pos: i64,
    /// It reached its end.
    finished: bool,
    /// The history changed in a way the tracker must save (the library knows too: `history_dirty`).
    duration_us: i64,
}

impl App {
    /// Called every tick: watch what is playing and keep the history.
    pub(crate) fn history_tick<H: Host<Video = FrameSink>>(&mut self, host: &mut H, now: Timestamp) {
        let (unix, off) = (host.clock().unix_time(), host.clock().utc_offset_secs());
        self.lib.lib.set_calendar(unix, off);
        if !self.lib.loaded {
            return;
        }
        let m = &self.model;
        let cur = self
            .playlist
            .current_id()
            .and_then(|i| self.playlist.get(i))
            .and_then(|it| it.track.map(|t| (it.id, t)));
        let playing = m.state == MediaState::Playing && m.has_media();
        let (pos, dur) = (m.position_us, m.duration_us.unwrap_or(0));
        let ended = m.state == MediaState::Ended;
        // The item changed (or playback stopped): close the one that was watched.
        let item = if m.has_media() { cur } else { None };
        if item != self.tracker.item {
            self.history_finish(host);
            self.tracker = Tracker { item, ..Tracker::default() };
        }
        if self.tracker.item.is_none() {
            return;
        }
        let t = &mut self.tracker;
        // The same item started again (repeat one): the position jumps back from the end.
        if t.last_pos > 0
            && dur > 0
            && t.last_pos >= dur - 3_000_000
            && pos < 2_000_000
            && pos < t.last_pos - 5_000_000
        {
            t.finished = true;
            let item = t.item;
            self.history_finish(host);
            self.tracker = Tracker { item, ..Tracker::default() };
        }
        let t = &mut self.tracker;
        t.duration_us = dur;
        if playing {
            if let Some(last) = t.last {
                // A long gap (a suspended machine, a stalled tick) is not listening.
                t.heard_us += (now - last).clamp(0, 2_000_000);
            }
            t.last = Some(now);
        } else {
            t.last = None;
        }
        t.last_pos = pos;
        if ended || (dur > 0 && pos >= dur - 1_500_000) {
            t.finished = true;
        }
        let heard_ms = (t.heard_us / 1000).clamp(0, u32::MAX as i64) as u32;
        let Some((_, lib_id)) = t.item else { return };
        match t.seq {
            None => {
                if !self.svc.settings.history_paused && counts_as_play(t.heard_us, dur) {
                    let at = host.clock().unix_time();
                    t.seq = self.lib.lib.record_play(lib_id, at, heard_ms, t.finished);
                    t.updated_at = t.heard_us;
                }
            }
            Some(seq) => {
                let (due, finished, heard_us) =
                    (t.heard_us - t.updated_at >= UPDATE_EVERY_US, t.finished, t.heard_us);
                if due || (finished && !self.history_flagged(seq)) {
                    self.lib.lib.update_play(seq, heard_ms, finished);
                    self.tracker.updated_at = heard_us;
                }
            }
        }
        if self.lib.lib.history_dirty() {
            self.history_save(host);
        }
    }

    /// Whether the play already says it finished.
    fn history_flagged(&self, seq: u64) -> bool {
        self.lib.lib.plays().iter().rev().find(|p| p.seq == seq).is_some_and(|p| p.finished)
    }

    /// The watched item is over: bring its play up to date for the last time.
    pub(crate) fn history_finish<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        let t = &self.tracker;
        if let Some(seq) = t.seq {
            let heard_ms = (t.heard_us / 1000).clamp(0, u32::MAX as i64) as u32;
            self.lib.lib.update_play(seq, heard_ms, t.finished);
        }
        if self.lib.lib.history_dirty() {
            self.history_save(host);
        }
    }

    pub(crate) fn history_save<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        let bytes = self.lib.lib.save_history();
        rvp_core::task::block_on(host.storage().store(rvp_library::HISTORY_KEY, &bytes));
    }
}
