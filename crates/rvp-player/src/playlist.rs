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
    }

    /// True when shuffled.
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    /// Append an item and return its id.
    pub fn add(&mut self, name: &str, source: &str) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        self.items.push(Item { id, name: String::from(name), source: String::from(source) });
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

    /// Make `id` the current item.
    pub fn set_current(&mut self, id: u32) -> bool {
        match self.order.iter().position(|&o| o == id) {
            Some(p) => {
                self.pos = Some(p);
                true
            }
            None => false,
        }
    }

    /// Remove an item. Returns true if it was the current one (the item after it, if any, is now current).
    pub fn remove(&mut self, id: u32) -> bool {
        let Some(i) = self.index_of(id) else { return false };
        self.items.remove(i);
        let Some(o) = self.order.iter().position(|&x| x == id) else { return false };
        self.order.remove(o);
        match self.pos {
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
        }
    }

    /// Remove everything.
    pub fn clear(&mut self) {
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
    }

    /// The item that will play after the current one finishes by itself (repeat rules apply), without moving.
    pub fn peek_next(&self) -> Option<u32> {
        let p = self.pos?;
        match self.repeat {
            Repeat::One => self.order.get(p).copied(),
            Repeat::All => self.order.get((p + 1) % self.order.len().max(1)).copied(),
            Repeat::Off => self.order.get(p + 1).copied(),
        }
    }

    /// Move on to what [`Playlist::peek_next`] named (an item finished by itself). Returns it.
    pub fn advance(&mut self) -> Option<u32> {
        let id = self.peek_next()?;
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
            0
        } else {
            return None;
        };
        self.pos = Some(np);
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
