//! The thumbnails in memory: a cache with a byte budget that drops the ones used longest ago.
//!
//! Every cover thumbnail is also saved through `Storage`, so memory only has to hold the ones a view is showing. A thumbnail
//! a view asks for that is not in memory is noted ([`ThumbCache::get`]) and the app loads it from storage a few per tick;
//! once the budget is reached, loading one drops the least recently used. A thumbnail that was not saved yet is never
//! dropped (the app saves when [`ThumbCache::needs_save`] says so).
use crate::art::Thumb;
use crate::model::ArtId;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

/// The default budget: about 500 thumbnails of 144 x 144 (62 KB each).
pub const DEFAULT_THUMB_BUDGET: usize = 32 * 1024 * 1024;

/// A byte-bounded LRU of thumbnails (see the module documentation).
#[derive(Debug, Default)]
pub(crate) struct ThumbCache {
    /// In memory, with the stamp of the last use.
    map: BTreeMap<ArtId, (Thumb, Cell<u64>)>,
    clock: Cell<u64>,
    bytes: usize,
    budget: usize,
    /// Dropped from memory (still in storage).
    evicted: BTreeSet<ArtId>,
    /// Asked for by a view and not in memory (that is, in storage, or about to be).
    requested: RefCell<BTreeSet<ArtId>>,
    /// Not saved yet: never dropped.
    unsaved: BTreeSet<ArtId>,
    /// Gone for good since the caller last asked (their saved copies can be deleted).
    dropped: Vec<ArtId>,
    /// Thumbnails dropped to stay in budget, for tests and the status line.
    pub(crate) evictions: u64,
}

impl ThumbCache {
    pub(crate) fn budget(&self) -> usize {
        if self.budget == 0 { DEFAULT_THUMB_BUDGET } else { self.budget }
    }

    pub(crate) fn set_budget(&mut self, bytes: usize) {
        self.budget = bytes;
        self.trim();
    }

    /// The thumbnail, marking it used; or a note that a view wants it.
    pub(crate) fn get(&self, id: ArtId) -> Option<&Thumb> {
        match self.map.get(&id) {
            Some((t, stamp)) => {
                let now = self.clock.get() + 1;
                self.clock.set(now);
                stamp.set(now);
                Some(t)
            }
            None => {
                if id != 0 {
                    self.requested.borrow_mut().insert(id);
                }
                None
            }
        }
    }

    /// Resident, or saved and dropped.
    pub(crate) fn known(&self, id: ArtId) -> bool {
        self.map.contains_key(&id) || self.evicted.contains(&id)
    }

    /// Every thumbnail there is, in memory or not.
    pub(crate) fn ids(&self) -> Vec<ArtId> {
        self.map.keys().chain(self.evicted.iter()).copied().collect()
    }

    pub(crate) fn len_resident(&self) -> usize {
        self.map.len()
    }

    pub(crate) fn resident_bytes(&self) -> usize {
        self.bytes
    }

    fn put(&mut self, id: ArtId, thumb: Thumb) {
        self.clock.set(self.clock.get() + 1);
        self.bytes += thumb.rgb.len();
        if let Some((old, _)) = self.map.insert(id, (thumb, Cell::new(self.clock.get()))) {
            self.bytes -= old.rgb.len();
        }
        self.evicted.remove(&id);
        self.requested.get_mut().remove(&id);
    }

    /// A thumbnail made by a scan: kept, and to be saved. Returns true if it was not there before.
    pub(crate) fn insert_new(&mut self, id: ArtId, thumb: Thumb) -> bool {
        let fresh = !self.map.contains_key(&id);
        self.put(id, thumb);
        if fresh {
            self.unsaved.insert(id);
        }
        self.trim();
        fresh
    }

    /// A thumbnail read from storage (already saved).
    pub(crate) fn insert_loaded(&mut self, id: ArtId, thumb: Thumb) {
        self.put(id, thumb);
        self.trim();
    }

    /// Drop the least recently used until the budget holds (never the thumbnail used last, nor one that is not saved yet).
    fn trim(&mut self) {
        while self.bytes > self.budget() && self.map.len() > 1 {
            let newest = self.clock.get();
            let victim = self
                .map
                .iter()
                .filter(|(id, (_, stamp))| !self.unsaved.contains(id) && stamp.get() != newest)
                .min_by_key(|(_, (_, stamp))| stamp.get())
                .map(|(id, _)| *id);
            let Some(id) = victim else { break };
            if let Some((t, _)) = self.map.remove(&id) {
                self.bytes -= t.rgb.len();
                self.evicted.insert(id);
                self.evictions += 1;
            }
        }
    }

    /// Forget a thumbnail for good (nothing uses it any more).
    pub(crate) fn remove(&mut self, id: ArtId) {
        if let Some((t, _)) = self.map.remove(&id) {
            self.bytes -= t.rgb.len();
        }
        self.evicted.remove(&id);
        self.unsaved.remove(&id);
        self.requested.get_mut().remove(&id);
        self.dropped.push(id);
    }

    /// Thumbnails not saved yet, as `(id, thumbnail)`; they count as saved afterwards.
    pub(crate) fn take_unsaved(&mut self) -> Vec<(ArtId, Thumb)> {
        let ids = core::mem::take(&mut self.unsaved);
        let out = ids.into_iter().filter_map(|id| self.map.get(&id).map(|(t, _)| (id, t.clone()))).collect();
        self.trim();
        out
    }

    /// True when memory is over budget only because some thumbnails are not saved yet.
    pub(crate) fn needs_save(&self) -> bool {
        self.bytes > self.budget() && !self.unsaved.is_empty()
    }

    pub(crate) fn take_dropped(&mut self) -> Vec<ArtId> {
        core::mem::take(&mut self.dropped)
    }

    /// Thumbnails views asked for and thumbnails to fill the free budget with: `fill` supplies the candidates.
    pub(crate) fn wanted(&self, fill: impl FnOnce() -> Vec<ArtId>) -> Vec<ArtId> {
        let mut out: Vec<ArtId> = self.requested.borrow().iter().copied().collect();
        // While there is room, bring in the rest as well (a small library is then in memory after a few ticks).
        let room = self.budget().saturating_sub(self.bytes);
        if room > 0 {
            let per = self.map.values().next().map_or(64 * 1024, |(t, _)| t.rgb.len().max(1));
            let slots = room / per;
            if slots > 0 {
                out.extend(fill().into_iter().filter(|a| !self.map.contains_key(a)).take(slots));
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn th(v: u8) -> Thumb {
        Thumb { w: 10, h: 10, rgb: vec![v; 300] }
    }

    #[test]
    fn drops_the_least_recently_used_but_not_unsaved_ones() {
        let mut c = ThumbCache::default();
        c.set_budget(1000); // three thumbnails
        for id in 1..=3 {
            c.insert_loaded(id, th(id as u8));
        }
        assert_eq!(c.len_resident(), 3);
        assert!(c.get(1).is_some()); // 1 is now newer than 2
        c.insert_loaded(4, th(4));
        assert_eq!((c.len_resident(), c.evictions), (3, 1));
        assert!(c.get(2).is_none(), "2 was the oldest");
        assert!(c.get(1).is_some() && c.get(3).is_some() && c.get(4).is_some());
        // The miss is noted for the loader.
        assert!(c.wanted(Vec::new).contains(&2));
        // Unsaved thumbnails stay while over budget, and can go once saved.
        let mut c = ThumbCache::default();
        c.set_budget(700);
        for id in 1..=4 {
            c.insert_new(id, th(id as u8));
        }
        assert_eq!(c.len_resident(), 4);
        assert!(c.needs_save());
        assert_eq!(c.take_unsaved().len(), 4);
        assert!(!c.needs_save());
        assert_eq!(c.len_resident(), 2, "{}", c.resident_bytes());
        assert!(c.resident_bytes() <= 700);
        // Removing for good reports the id once.
        c.remove(4);
        assert_eq!(c.take_dropped(), vec![4]);
        assert!(!c.known(4));
    }

    #[test]
    fn fills_free_budget_with_candidates_only() {
        let mut c = ThumbCache::default();
        c.set_budget(1000);
        c.insert_loaded(1, th(1));
        let w = c.wanted(|| vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(w, vec![2, 3], "room for two more");
        c.insert_loaded(2, th(2));
        c.insert_loaded(3, th(3));
        assert!(c.wanted(|| vec![4, 5]).is_empty(), "full: only what views ask for");
    }
}
