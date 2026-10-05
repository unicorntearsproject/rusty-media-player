//! Restoring the queue and the playback position after a restart.
//!
//! What is saved, through the host's [`Storage`]: the queue (names, library track ids, and the file ids when the host's ids
//! survive a restart, see [`Host::stable_ids`]), which item is current, shuffle and repeat, and, in a second small value
//! written every few seconds, the position in the current item. At the next start, when nothing was opened from outside,
//! the queue comes back, the current item is opened *paused* and moved to where it was. A library track whose folder is not
//! readable yet (a browser asks again) waits until the library is listed and then does the same.
use crate::{App, RESUME_SAVE_EVERY_US};
use alloc::string::String;
use alloc::vec::Vec;
use rvp_core::Timestamp;
use rvp_host::{FrameSink, Host, Storage};
use rvp_player::Repeat;

/// Where the queue is kept (under `library/` so the browser puts it in IndexedDB with the rest of the big values).
pub const QUEUE_KEY: &str = "library/queue";
/// Where the position in the current item is kept.
pub const POSITION_KEY: &str = "session/position";

const QUEUE_MAGIC: &[u8; 4] = b"RVQ1";
const MAX_ITEMS: usize = 200_000;
const MAX_STR: usize = 8192;

/// One saved queue entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedItem {
    /// Display name.
    pub name: String,
    /// Host id to open it with (empty when the host's ids do not last).
    pub source: String,
    /// Library track id, if the entry came from the library.
    pub track: Option<u32>,
}

/// The saved queue.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SavedQueue {
    /// Entries in list order.
    pub items: Vec<SavedItem>,
    /// Index of the current one.
    pub current: Option<usize>,
    /// Shuffle was on.
    pub shuffle: bool,
    /// Repeat mode: 0 off, 1 all, 2 one.
    pub repeat: u8,
}

impl SavedQueue {
    /// The queue as bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(64 + self.items.len() * 64);
        o.extend_from_slice(QUEUE_MAGIC);
        o.push(self.shuffle as u8);
        o.push(self.repeat);
        o.extend_from_slice(&self.current.map_or(u32::MAX, |c| c as u32).to_le_bytes());
        o.extend_from_slice(&(self.items.len() as u32).to_le_bytes());
        for i in &self.items {
            for s in [&i.name, &i.source] {
                let b = s.as_bytes();
                let n = b.len().min(MAX_STR);
                o.extend_from_slice(&(n as u16).to_le_bytes());
                o.extend_from_slice(&b[..n]);
            }
            o.extend_from_slice(&i.track.unwrap_or(0).to_le_bytes());
        }
        o
    }

    /// The queue from bytes made by [`SavedQueue::encode`]; `None` for anything else (damaged, other version).
    pub fn decode(b: &[u8]) -> Option<Self> {
        let mut r = Reader(b);
        if r.take(4)? != QUEUE_MAGIC {
            return None;
        }
        let shuffle = r.take(1)?[0] != 0;
        let repeat = r.take(1)?[0].min(2);
        let current = match r.u32()? {
            u32::MAX => None,
            c => Some(c as usize),
        };
        let n = r.u32()? as usize;
        if n > MAX_ITEMS || n.saturating_mul(6) > b.len() {
            return None;
        }
        let mut items = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.str()?;
            let source = r.str()?;
            let track = match r.u32()? {
                0 => None,
                t => Some(t),
            };
            items.push(SavedItem { name, source, track });
        }
        let current = current.filter(|&c| c < items.len());
        Some(Self { items, current, shuffle, repeat })
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn str(&mut self) -> Option<String> {
        let n = u16::from_le_bytes(self.take(2)?.try_into().ok()?) as usize;
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

/// The position blob: index of the current item in list order, and the position in milliseconds.
fn encode_position(index: usize, pos_us: Timestamp) -> Vec<u8> {
    let mut o = Vec::with_capacity(12);
    o.extend_from_slice(&(index as u32).to_le_bytes());
    o.extend_from_slice(&(pos_us / 1000).to_le_bytes());
    o
}

fn decode_position(b: &[u8]) -> Option<(usize, Timestamp)> {
    let raw: [u8; 12] = b.try_into().ok()?;
    let index = u32::from_le_bytes(raw[..4].try_into().ok()?) as usize;
    let ms = i64::from_le_bytes(raw[4..].try_into().ok()?);
    Some((index, ms.saturating_mul(1000)))
}

/// What identifies the saved queue: list revision, current item, shuffle, repeat.
pub(crate) type QueueSig = (u32, Option<u32>, bool, Repeat);

/// The restore bookkeeping of the app.
#[derive(Default)]
pub(crate) struct RestoreState {
    /// The first tick's restore has run (or was not wanted).
    pub done: bool,
    /// The item to open paused, and where, once its source can be opened.
    pub pending: Option<(u32, Timestamp)>,
    /// Set while the restore itself starts the item, so it does not cancel itself.
    pub starting: bool,
    /// Where to seek once the opened item's length is known.
    pub forced_pos: Option<Timestamp>,
    /// What was last saved.
    pub saved_sig: Option<QueueSig>,
    /// When the position was last saved.
    pub last_pos_save: Timestamp,
}

impl App {
    fn queue_sig(&self) -> QueueSig {
        (
            self.playlist.revision(),
            self.playlist.current_id(),
            self.playlist.shuffle(),
            self.playlist.repeat(),
        )
    }

    /// The queue as it is now, for saving. Entries the host could not open again are left out.
    fn saved_queue<H: Host<Video = FrameSink>>(&self, host: &H) -> SavedQueue {
        let stable = host.stable_ids();
        let cur = self.playlist.current_id();
        let mut q = SavedQueue {
            shuffle: self.playlist.shuffle(),
            repeat: match self.playlist.repeat() {
                Repeat::Off => 0,
                Repeat::All => 1,
                Repeat::One => 2,
            },
            ..SavedQueue::default()
        };
        for i in self.playlist.items() {
            let in_library = i.track.is_some_and(|t| self.lib.lib.track(t).is_some());
            if !in_library && !(stable && !i.source.is_empty()) {
                continue; // a dropped file in a browser: nothing to open it with next time
            }
            if Some(i.id) == cur {
                q.current = Some(q.items.len());
            }
            q.items.push(SavedItem {
                name: i.name.clone(),
                source: if stable { i.source.clone() } else { String::new() },
                track: i.track.filter(|_| in_library),
            });
        }
        q
    }

    /// Index of the current item in the saved list (the same filter as [`App::saved_queue`]).
    fn saved_index<H: Host<Video = FrameSink>>(&self, host: &H) -> Option<usize> {
        self.saved_queue(host).current
    }

    fn write_position<H: Host<Video = FrameSink>>(&mut self, host: &mut H, now: Timestamp) {
        let Some(cur) = self.playlist.current_id() else { return };
        let Some(s) = self.session.as_ref().filter(|s| s.tag() == cur) else { return };
        let pos = if s.state() == rvp_player::SessionState::Ended { 0 } else { s.position_us(now) };
        let Some(index) = self.saved_index(host) else { return };
        self.restore.last_pos_save = now;
        rvp_core::task::block_on(host.storage().store(POSITION_KEY, &encode_position(index, pos)));
    }

    /// Keep the saved queue current: the queue itself when it changed, the position every few seconds, and `force` right now
    /// (the host is about to go away).
    pub(crate) fn save_queue<H: Host<Video = FrameSink>>(
        &mut self,
        host: &mut H,
        now: Timestamp,
        force: bool,
    ) {
        if !self.restore.done || self.restore.pending.is_some() {
            return;
        }
        let sig = self.queue_sig();
        if self.restore.saved_sig != Some(sig) {
            self.restore.saved_sig = Some(sig);
            let bytes = if self.playlist.is_empty() { Vec::new() } else { self.saved_queue(host).encode() };
            rvp_core::task::block_on(host.storage().store(QUEUE_KEY, &bytes));
            if self.playlist.is_empty() {
                rvp_core::task::block_on(host.storage().store(POSITION_KEY, &[]));
            } else {
                self.write_position(host, now);
            }
            return;
        }
        let playing = self.model.state.is_active();
        if force || (playing && now - self.restore.last_pos_save >= RESUME_SAVE_EVERY_US) {
            self.write_position(host, now);
        }
    }

    /// Bring the saved queue back (once, at the first tick after the library is loaded) and open its current item
    /// paused as soon as it can be opened.
    pub(crate) fn restore_tick<H>(&mut self, host: &mut H, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        if !self.restore.done && self.lib.loaded {
            self.restore.done = true;
            if self.playlist.is_empty() && self.session.is_none() {
                self.restore_queue(host);
            }
            // With an empty or unusable saved queue this saves nothing; with something opened from outside it saves that.
            self.restore.saved_sig =
                if self.playlist.is_empty() { Some(self.queue_sig()) } else { self.restore.saved_sig };
        }
        let Some((id, pos)) = self.restore.pending else { return };
        let Some(item) = self.playlist.get(id) else {
            self.restore.pending = None;
            return;
        };
        if item.source.is_empty() {
            return; // wait for the library listing
        }
        self.restore.pending = None;
        self.restore.starting = true;
        self.restore.forced_pos = Some(pos);
        self.play_item(host, id);
        self.restore.starting = false;
        if let Some(s) = &mut self.session {
            s.pause(now);
        }
        self.refresh_model(now);
    }

    fn restore_queue<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        let Some(bytes) = rvp_core::task::block_on(host.storage().load(QUEUE_KEY)) else { return };
        let Some(q) = SavedQueue::decode(&bytes) else { return };
        if q.items.is_empty() {
            return;
        }
        let stable = host.stable_ids();
        let mut ids = Vec::with_capacity(q.items.len());
        for it in &q.items {
            let track = it.track.filter(|&t| self.lib.lib.track(t).is_some());
            let lib_src =
                track.and_then(|t| self.lib.lib.track(t)).map(|t| t.src.clone()).filter(|s| !s.is_empty());
            let source = lib_src.unwrap_or_else(|| if stable { it.source.clone() } else { String::new() });
            if source.is_empty() && track.is_none() {
                ids.push(None);
                continue;
            }
            ids.push(Some(self.playlist.add_track(&it.name, &source, track)));
        }
        if self.playlist.is_empty() {
            return;
        }
        self.playlist.set_repeat(match q.repeat {
            1 => Repeat::All,
            2 => Repeat::One,
            _ => Repeat::Off,
        });
        // The position saved last, if it belongs to this queue.
        let pos =
            rvp_core::task::block_on(host.storage().load(POSITION_KEY)).and_then(|b| decode_position(&b));
        let cur = q.current.and_then(|c| ids.get(c).copied().flatten());
        if let Some(id) = cur {
            self.playlist.set_current(id);
            let pos = pos.filter(|(i, _)| Some(*i) == q.current).map_or(0, |(_, p)| p);
            self.restore.pending = Some((id, pos));
        }
        if q.shuffle {
            self.playlist.set_shuffle(true, host.clock().now_us() as u64);
        }
        self.lib.auto_mode = true;
        self.restore.saved_sig = Some(self.queue_sig());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SavedQueue {
        SavedQueue {
            items: alloc::vec![
                SavedItem { name: "a.flac".into(), source: "/m/a.flac".into(), track: Some(3) },
                SavedItem { name: "Жук \u{65e5}.mkv".into(), source: String::new(), track: None },
            ],
            current: Some(1),
            shuffle: true,
            repeat: 2,
        }
    }

    #[test]
    fn the_queue_round_trips() {
        let q = sample();
        assert_eq!(SavedQueue::decode(&q.encode()), Some(q));
    }

    #[test]
    fn damaged_queues_are_refused() {
        let b = sample().encode();
        for n in 0..b.len() {
            assert!(SavedQueue::decode(&b[..n]).is_none(), "cut at {n}");
        }
        let mut bad = b.clone();
        bad[0] = b'X';
        assert!(SavedQueue::decode(&bad).is_none());
        // A huge count with no data behind it.
        let mut huge = b[..4].to_vec();
        huge.extend_from_slice(&[0, 0]);
        huge.extend_from_slice(&0u32.to_le_bytes());
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(SavedQueue::decode(&huge).is_none());
        // Garbage never panics.
        let mut x = 0x1234_5678u32;
        for _ in 0..2000 {
            let v: Vec<u8> = (0..40)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            let _ = SavedQueue::decode(&v);
        }
    }

    #[test]
    fn positions_round_trip() {
        assert_eq!(decode_position(&encode_position(7, 123_456_789)), Some((7, 123_456_000)));
        assert_eq!(decode_position(&[1, 2, 3]), None);
    }
}
