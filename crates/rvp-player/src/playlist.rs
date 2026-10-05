//! The playlist: an ordered list of items with a current item, next/previous, repeat and shuffle.
//!
//! Items carry a host-defined `source` string (a path, a stashed file id, a URL): the playlist never opens
//! anything, the application resolves the string through the host when an item is played.
use alloc::string::String;
use alloc::vec::Vec;

/// One entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Stable id, never reused within a playlist.
    pub id: u32,
    /// Display name (usually the file name).
    pub name: String,
    /// Host-defined identifier the host can open (`OpenRequest::Id`).
    pub source: String,
    /// The library track this item came from, if any (the playlist does not interpret it).
    pub track: Option<u32>,
}

/// What happens at the end of the list or of an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    /// Stop after the last item.
    #[default]
    Off,
    /// Start over after the last item.
    All,
    /// Play the current item again and again (an explicit next/previous still moves on).
    One,
}

impl Repeat {
    /// Off, then All, then One, then Off.
    pub fn cycled(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

/// The list, its play order and the current position in it.
#[derive(Debug, Clone, Default)]
pub struct Playlist {
    items: Vec<Item>,
    /// Item ids in the order they play: the list order, or a shuffled permutation.
    order: Vec<u32>,
    /// Index into `order` of the current item.
    pos: Option<usize>,
    repeat: Repeat,
    shuffle: bool,
    next_id: u32,
    rng: u64,
    /// Counts up whenever the items or their order change (not when only the current item moves), so a view can tell
    /// whether it must rebuild.
    rev: u32,
    /// Shuffled with repeat all, standing on the last item: the order of the next round, shuffled so that the item that
    /// ends this round does not start the next one.
    wrap_order: Option<Vec<u32>>,
}

impl Playlist {
    /// An empty playlist.
    pub fn new() -> Self {
        Self { rng: 0x9e37_79b9_7f4a_7c15, ..Self::default() }
    }

    fn rand(&mut self, n: usize) -> usize {
        // xorshift64*; shuffling does not need more.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % n.max(1)
    }

    /// Items in list order.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The item with `id`.
    pub fn get(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Index of `id` in list order.
    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    /// The current item.
    pub fn current(&self) -> Option<&Item> {
        self.current_id().and_then(|id| self.get(id))
    }

    /// The current item's id.
    pub fn current_id(&self) -> Option<u32> {
        self.pos.and_then(|p| self.order.get(p)).copied()
    }

    /// Repeat mode.
    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    /// Set the repeat mode.
    pub fn set_repeat(&mut self, r: Repeat) {
        self.repeat = r;
        self.prepare_wrap();
    }

    /// True when shuffled.
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    /// Point item `id` at a different source (a library folder was listed again and its files have new ids). Not a change of the list.
    pub fn set_source(&mut self, id: u32, source: &str) {
        if let Some(i) = self.items.iter_mut().find(|i| i.id == id) {
            i.source = String::from(source);
        }
    }

    /// Changes whenever the list or its order does.
    pub fn revision(&self) -> u32 {
        self.rev
    }

    /// Append an item and return its id.
    pub fn add(&mut self, name: &str, source: &str) -> u32 {
        self.add_track(name, source, None)
    }

    /// Append an item that came from library track `track`.
    pub fn add_track(&mut self, name: &str, source: &str, track: Option<u32>) -> u32 {
        self.rev = self.rev.wrapping_add(1);
        self.wrap_order = None;
        self.next_id += 1;
        let id = self.next_id;
        self.items.push(Item { id, name: String::from(name), source: String::from(source), track });
        if self.shuffle {
            // Somewhere after the current item.
            let from = self.pos.map_or(0, |p| p + 1);
            let at = from + self.rand(self.order.len() - from + 1);
            self.order.insert(at.min(self.order.len()), id);
        } else {
            self.order.push(id);
        }
        id
    }

    /// Insert an item right after item `after` (`None`: after the current item, or at the start of an empty list) in both
    /// the list and the play order, so it plays next ("play next"). Returns its id.
    pub fn insert_after(&mut self, after: Option<u32>, name: &str, source: &str, track: Option<u32>) -> u32 {
        let anchor = after.or_else(|| self.current_id());
        let Some(a) = anchor.filter(|a| self.index_of(*a).is_some()) else {
            // Nothing to follow: at the front.
            self.rev = self.rev.wrapping_add(1);
            self.wrap_order = None;
            self.next_id += 1;
            let id = self.next_id;
            self.items.insert(0, Item { id, name: String::from(name), source: String::from(source), track });
            self.order.insert(0, id);
            if let Some(p) = &mut self.pos {
                *p += 1;
            }
            return id;
        };
        self.rev = self.rev.wrapping_add(1);
        self.wrap_order = None;
        self.next_id += 1;
        let id = self.next_id;
        let li = self.index_of(a).map_or(self.items.len(), |i| i + 1);
        self.items.insert(li, Item { id, name: String::from(name), source: String::from(source), track });
        let oi = self.order.iter().position(|&o| o == a).map_or(self.order.len(), |i| i + 1);
        self.order.insert(oi, id);
        if let Some(p) = &mut self.pos {
            if oi <= *p {
                *p += 1;
            }
        }
        self.prepare_wrap();
        id
    }

    /// Move item `id` to right after item `after` (`None`: after the current item) in both the list and the play order
    /// ("play this next"). Does nothing if either is missing or they are the same item.
    pub fn move_after(&mut self, id: u32, after: Option<u32>) {
        let anchor = after.or_else(|| self.current_id());
        let Some(a) = anchor.filter(|a| *a != id) else { return };
        let (Some(i), Some(_)) = (self.index_of(id), self.index_of(a)) else { return };
        self.rev = self.rev.wrapping_add(1);
        self.wrap_order = None;
        let cur = self.current_id();
        let item = self.items.remove(i);
        let ai = self.index_of(a).map_or(self.items.len(), |x| x + 1);
        self.items.insert(ai, item);
        if let Some(o) = self.order.iter().position(|&x| x == id) {
            self.order.remove(o);
        }
        let oi = self.order.iter().position(|&x| x == a).map_or(self.order.len(), |x| x + 1);
        self.order.insert(oi, id);
        self.pos = cur.and_then(|c| self.order.iter().position(|&x| x == c));
        self.prepare_wrap();
    }

    /// Make `id` the current item.
    pub fn set_current(&mut self, id: u32) -> bool {
        match self.order.iter().position(|&o| o == id) {
            Some(p) => {
                self.pos = Some(p);
                self.prepare_wrap();
                true
            }
            None => false,
        }
    }

    /// With shuffle and repeat all, on the last item: decide the order of the next round now, so that what plays after this
    /// item is known (the player queues it ahead) and is never the item itself.
    fn prepare_wrap(&mut self) {
        self.wrap_order = None;
        let n = self.order.len();
        if self.shuffle && self.repeat == Repeat::All && n > 1 && self.pos == Some(n - 1) {
            let last = self.order[n - 1];
            let mut ids = self.order.clone();
            for i in (1..n).rev() {
                let j = self.rand(i + 1);
                ids.swap(i, j);
            }
            if ids[0] == last {
                let j = 1 + self.rand(n - 1);
                ids.swap(0, j);
            }
            self.wrap_order = Some(ids);
        }
    }

    /// Start the next round: the prepared order replaces the old one.
    fn take_wrap(&mut self) {
        if let Some(o) = self.wrap_order.take() {
            self.order = o;
        }
    }

    /// Remove an item. Returns true if it was the current one (the item after it, if any, is now current).
    pub fn remove(&mut self, id: u32) -> bool {
        let Some(i) = self.index_of(id) else { return false };
        self.rev = self.rev.wrapping_add(1);
        self.wrap_order = None;
        self.items.remove(i);
        let Some(o) = self.order.iter().position(|&x| x == id) else { return false };
        self.order.remove(o);
        let was_current = match self.pos {
            Some(p) if o < p => {
                self.pos = Some(p - 1);
                false
            }
            Some(p) if o == p => {
                if p >= self.order.len() {
                    self.pos = if self.order.is_empty() { None } else { Some(0) };
                    // Past the end: nothing is current unless repeating.
                    if self.repeat == Repeat::Off {
                        self.pos = None;
                    }
                }
                true
            }
            _ => false,
        };
        self.prepare_wrap();
        was_current
    }

    /// Remove everything.
    pub fn clear(&mut self) {
        self.rev = self.rev.wrapping_add(1);
        self.wrap_order = None;
        self.items.clear();
        self.order.clear();
        self.pos = None;
    }

    /// Move the item `id` by `delta` places in the list (negative = earlier). The play order follows when not
    /// shuffled.
    pub fn move_item(&mut self, id: u32, delta: i32) {
        let Some(i) = self.index_of(id) else { return };
        let j = (i as i32 + delta).clamp(0, self.items.len() as i32 - 1) as usize;
        if i == j {
            return;
        }
        self.rev = self.rev.wrapping_add(1);
        self.wrap_order = None;
        let it = self.items.remove(i);
        self.items.insert(j, it);
        if !self.shuffle {
            let cur = self.current_id();
            self.order = self.items.iter().map(|i| i.id).collect();
            self.pos = cur.and_then(|c| self.order.iter().position(|&o| o == c));
        }
    }

    /// Turn shuffle on or off. Turning it on keeps the current item first; turning it off restores list order
    /// (the current item stays current).
    pub fn set_shuffle(&mut self, on: bool, seed: u64) {
        if on == self.shuffle {
            return;
        }
        self.rev = self.rev.wrapping_add(1);
        self.wrap_order = None;
        self.shuffle = on;
        let cur = self.current_id();
        if on {
            self.rng ^= seed | 1;
            let mut ids: Vec<u32> = self.items.iter().map(|i| i.id).filter(|i| Some(*i) != cur).collect();
            for i in (1..ids.len()).rev() {
                let j = self.rand(i + 1);
                ids.swap(i, j);
            }
            self.order.clear();
            if let Some(c) = cur {
                self.order.push(c);
                self.pos = Some(0);
            }
            self.order.extend(ids);
        } else {
            self.order = self.items.iter().map(|i| i.id).collect();
            self.pos = cur.and_then(|c| self.order.iter().position(|&o| o == c));
        }
        self.prepare_wrap();
    }

    /// The item that will play after the current one finishes by itself (repeat rules apply), without moving.
    pub fn peek_next(&self) -> Option<u32> {
        let p = self.pos?;
        match self.repeat {
            Repeat::One => self.order.get(p).copied(),
            Repeat::All if self.shuffle && p + 1 >= self.order.len() && self.order.len() > 1 => self
                .wrap_order
                .as_ref()
                .and_then(|o| o.first().copied())
                .or_else(|| self.order.first().copied()),
            Repeat::All => self.order.get((p + 1) % self.order.len().max(1)).copied(),
            Repeat::Off => self.order.get(p + 1).copied(),
        }
    }

    /// Move on to what [`Playlist::peek_next`] named (an item finished by itself). Returns it.
    pub fn advance(&mut self) -> Option<u32> {
        let id = self.peek_next()?;
        if self.pos.is_some_and(|p| p + 1 >= self.order.len()) {
            self.take_wrap();
        }
        self.set_current(id);
        Some(id)
    }

    /// The user asked for the next item: the one after the current, wrapping only with repeat All (repeat One
    /// does not stop it). With nothing current, the first.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<u32> {
        let Some(p) = self.pos else {
            let id = *self.order.first()?;
            self.pos = Some(0);
            return Some(id);
        };
        let n = self.order.len();
        let np = if p + 1 < n {
            p + 1
        } else if self.repeat == Repeat::All {
            self.take_wrap();
            0
        } else {
            return None;
        };
        self.pos = Some(np);
        self.prepare_wrap();
        self.order.get(np).copied()
    }

    /// The user asked for the previous item: the one before, wrapping with repeat All; at the very start the
    /// current item again (start it over).
    pub fn prev(&mut self) -> Option<u32> {
        let p = self.pos?;
        let np = if p > 0 {
            p - 1
        } else if self.repeat == Repeat::All {
            self.order.len() - 1
        } else {
            0
        };
        self.pos = Some(np);
        self.prepare_wrap();
        self.order.get(np).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    fn list(n: usize) -> Playlist {
        let mut p = Playlist::new();
        for i in 0..n {
            p.add(&format!("t{i}"), &format!("src{i}"));
        }
        p
    }

    fn names(p: &Playlist) -> Vec<&str> {
        p.items().iter().map(|i| i.name.as_str()).collect()
    }

    #[test]
    fn plays_in_order_and_stops_at_the_end() {
        let mut p = list(3);
        assert_eq!(p.current(), None);
        assert_eq!(p.next(), Some(1));
        assert_eq!(p.peek_next(), Some(2));
        assert_eq!(p.advance(), Some(2));
        assert_eq!(p.advance(), Some(3));
        assert_eq!(p.peek_next(), None);
        assert_eq!(p.advance(), None);
        assert_eq!(p.current_id(), Some(3), "stays on the last item");
        assert_eq!(p.next(), None);
        assert_eq!(p.prev(), Some(2));
        assert_eq!(p.prev(), Some(1));
        assert_eq!(p.prev(), Some(1), "at the start, previous restarts the item");
    }

    #[test]
    fn repeat_modes() {
        let mut p = list(2);
        p.set_current(2);
        p.set_repeat(Repeat::All);
        assert_eq!(p.peek_next(), Some(1));
        assert_eq!(p.next(), Some(1));
        assert_eq!(p.prev(), Some(2), "previous wraps with repeat all");
        p.set_repeat(Repeat::One);
        assert_eq!(p.peek_next(), Some(2));
        assert_eq!(p.advance(), Some(2));
        assert_eq!(p.next(), None, "explicit next wraps only with repeat all");
        p.set_repeat(Repeat::All);
        assert_eq!(p.next(), Some(1));
        assert_eq!(Repeat::Off.cycled().cycled().cycled(), Repeat::Off);
    }

    #[test]
    fn add_remove_move() {
        let mut p = list(4);
        p.set_current(2);
        p.move_item(4, -3);
        assert_eq!(names(&p), ["t3", "t0", "t1", "t2"]);
        assert_eq!(p.current_id(), Some(2), "the current item stays current when the list is reordered");
        assert_eq!(p.peek_next(), Some(3));
        assert!(!p.remove(1), "removing an item before the current one keeps the current one");
        assert_eq!(p.current_id(), Some(2));
        assert!(p.remove(2), "removing the current item reports it");
        assert_eq!(p.current_id(), Some(3), "the next item took its place");
        assert!(p.remove(3));
        assert_eq!(p.current_id(), None, "ran off the end with repeat off");
        p.clear();
        assert!(p.is_empty() && p.peek_next().is_none());
        assert_eq!(p.add("x", "y"), 5, "ids are never reused");
    }

    #[test]
    fn play_next_inserts_after_the_current_item() {
        let mut p = list(3);
        p.set_current(1);
        let a = p.insert_after(None, "a", "sa", Some(77));
        let b = p.insert_after(Some(a), "b", "sb", None);
        assert_eq!(names(&p), ["t0", "a", "b", "t1", "t2"]);
        assert_eq!(p.get(a).unwrap().track, Some(77));
        assert_eq!(p.current_id(), Some(1));
        assert_eq!(p.peek_next(), Some(a));
        assert_eq!(p.advance(), Some(a));
        assert_eq!(p.advance(), Some(b));
        // In an empty list it becomes the first item; an item inserted before the current one keeps it current.
        let mut q = Playlist::new();
        let x = q.insert_after(None, "x", "sx", None);
        q.set_current(x);
        let y = q.insert_after(Some(x), "y", "sy", None);
        assert_eq!(names(&q), ["x", "y"]);
        assert_eq!((q.current_id(), q.peek_next()), (Some(x), Some(y)));
        // While shuffled it joins the play order right after the current item too.
        let mut s = list(10);
        s.set_current(4);
        s.set_shuffle(true, 5);
        let n = s.insert_after(None, "n", "sn", None);
        assert_eq!(s.peek_next(), Some(n));
        assert_eq!(s.current_id(), Some(4));
    }

    #[test]
    fn an_item_can_be_moved_to_play_next() {
        let mut p = list(5);
        p.set_current(2);
        p.move_after(5, None);
        assert_eq!(names(&p), ["t0", "t1", "t4", "t2", "t3"]);
        assert_eq!(p.current_id(), Some(2));
        assert_eq!(p.peek_next(), Some(5));
        p.move_after(1, Some(5));
        assert_eq!(p.peek_next(), Some(5));
        assert_eq!(p.advance(), Some(5));
        assert_eq!(p.advance(), Some(1));
        p.move_after(3, Some(3)); // itself: nothing
    }

    #[test]
    fn revision_counts_changes_to_the_list_only() {
        let mut p = list(3);
        let r = p.revision();
        p.set_current(2);
        p.next();
        assert_eq!(p.revision(), r, "moving the current item is not a change of the list");
        p.move_item(1, 1);
        assert_ne!(p.revision(), r);
        let r = p.revision();
        p.set_shuffle(true, 3);
        assert_ne!(p.revision(), r);
    }

    #[test]
    fn a_shuffled_round_never_ends_and_restarts_with_the_same_item() {
        for seed in 0..40u64 {
            let mut p = list(5);
            p.set_repeat(Repeat::All);
            p.set_current(1);
            p.set_shuffle(true, seed);
            let mut seq = alloc::vec![p.current_id().unwrap()];
            for _ in 0..30 {
                // What is announced as next is what plays.
                let peek = p.peek_next().unwrap();
                let got = p.advance().unwrap();
                assert_eq!(peek, got);
                seq.push(got);
            }
            for w in seq.windows(2) {
                assert_ne!(w[0], w[1], "seed {seed}: the same item twice in a row: {seq:?}");
            }
            // Every round of five plays all five.
            for round in seq.chunks_exact(5) {
                let mut r = round.to_vec();
                r.sort();
                assert_eq!(r, [1, 2, 3, 4, 5], "seed {seed}: {seq:?}");
            }
        }
    }

    #[test]
    fn shuffle_plays_everything_once_with_the_current_item_first() {
        let mut p = list(20);
        p.set_current(7);
        p.set_shuffle(true, 12345);
        assert!(p.shuffle());
        assert_eq!(p.current_id(), Some(7));
        let mut seen = alloc::vec![7];
        while let Some(id) = p.advance() {
            seen.push(id);
        }
        assert_eq!(seen.len(), 20);
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(sorted, (1..=20).collect::<Vec<_>>(), "every item exactly once");
        assert_ne!(seen, (7..=20).chain(1..7).collect::<Vec<_>>(), "and not in list order");
        // Turning it off keeps the current item and goes back to list order.
        p.set_shuffle(false, 0);
        assert_eq!(p.current_id(), Some(seen[19]));
        let cur = p.current_id().unwrap();
        assert_eq!(p.peek_next(), if cur == 20 { None } else { Some(cur + 1) });
        // An item added while shuffled joins the remaining play order.
        p.set_shuffle(true, 99);
        let id = p.add("late", "late");
        let mut rest = Vec::new();
        while let Some(i) = p.advance() {
            rest.push(i);
        }
        assert!(rest.contains(&id));
    }
}
