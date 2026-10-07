//! The decoded picture buffer: picture order count (8.3.1), reference picture sets (8.3.2), reference picture lists (8.3.4) and output
//! order (C.5.2). Generic over what a decoded picture is (`T`: a GPU surface, a frame buffer).
use crate::slice::{SliceHeader, SliceType};
use alloc::vec::Vec;

/// How a picture in the buffer is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marking {
    /// Not used for reference.
    Unused,
    /// Used for short-term reference.
    ShortTerm,
    /// Used for long-term reference.
    LongTerm,
}

/// A picture in the buffer.
#[derive(Debug, Clone)]
pub struct DpbEntry<T> {
    /// `PicOrderCntVal`.
    pub poc: i32,
    /// Reference marking.
    pub marking: Marking,
    /// "Needed for output".
    pub needed_for_output: bool,
    /// `PicLatencyCount`.
    pub latency: u32,
    /// The decoded picture.
    pub payload: T,
    /// The presentation time the container gave the picture.
    pub pts: i64,
}

/// What `PicOrderCntVal` of the previous picture of temporal layer 0 was (the state 8.3.1 needs).
#[derive(Debug, Clone, Copy, Default)]
pub struct PocState {
    prev_lsb: u32,
    prev_msb: i32,
}

impl PocState {
    /// Derive `PicOrderCntVal` for a picture with `poc_lsb`, given `max_lsb`; `irap_no_rasl_output` when the picture is an IRAP with
    /// `NoRaslOutputFlag` (its MSB is 0).
    pub fn poc(&self, poc_lsb: u32, max_lsb: u32, irap_no_rasl_output: bool) -> i32 {
        let msb = if irap_no_rasl_output {
            0
        } else if poc_lsb < self.prev_lsb && self.prev_lsb - poc_lsb >= max_lsb / 2 {
            self.prev_msb + max_lsb as i32
        } else if poc_lsb > self.prev_lsb && poc_lsb - self.prev_lsb > max_lsb / 2 {
            self.prev_msb - max_lsb as i32
        } else {
            self.prev_msb
        };
        msb + poc_lsb as i32
    }

    /// Remember a picture of temporal layer 0 that is not a RASL, RADL or sub-layer non-reference picture (`prevTid0Pic`).
    pub fn remember(&mut self, poc: i32, max_lsb: u32) {
        let lsb = poc.rem_euclid(max_lsb as i32) as u32;
        self.prev_lsb = lsb;
        self.prev_msb = poc - lsb as i32;
    }
}

/// The reference picture set of the current picture, as POCs (8.3.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rps {
    /// `PocStCurrBefore`.
    pub st_curr_before: Vec<i32>,
    /// `PocStCurrAfter`.
    pub st_curr_after: Vec<i32>,
    /// `PocStFoll`.
    pub st_foll: Vec<i32>,
    /// `PocLtCurr` with its `CurrDeltaPocMsbPresentFlag` (when false only the low bits of the POC identify the picture).
    pub lt_curr: Vec<(i32, bool)>,
    /// `PocLtFoll` with `FollDeltaPocMsbPresentFlag`.
    pub lt_foll: Vec<(i32, bool)>,
}

/// Derive the RPS of a picture with POC `poc` from its first slice header (8-5 and 8-6).
pub fn derive_rps(h: &SliceHeader, poc: i32, max_lsb: u32) -> Rps {
    let mut rps = Rps::default();
    if h.nal.is_idr() {
        return rps;
    }
    let s = &h.st_rps;
    for (i, d) in s.delta_poc_s0.iter().enumerate() {
        if s.used_s0[i] {
            rps.st_curr_before.push(poc + d);
        } else {
            rps.st_foll.push(poc + d);
        }
    }
    for (i, d) in s.delta_poc_s1.iter().enumerate() {
        if s.used_s1[i] {
            rps.st_curr_after.push(poc + d);
        } else {
            rps.st_foll.push(poc + d);
        }
    }
    for lt in &h.long_term {
        let mut p = lt.poc_lsb as i32;
        if lt.msb_present {
            p += poc - (lt.delta_poc_msb_cycle as i32) * max_lsb as i32 - poc.rem_euclid(max_lsb as i32);
        }
        if lt.used_by_curr {
            rps.lt_curr.push((p, lt.msb_present));
        } else {
            rps.lt_foll.push((p, lt.msb_present));
        }
    }
    rps
}

/// Where a reference list entry comes from: an index into [`RefSet::pics`].
pub type RefIdx = usize;

/// The pictures a picture refers to, found in the buffer, and the lists built from them.
#[derive(Debug, Clone, Default)]
pub struct RefSet {
    /// The pictures of the RPS that are in the buffer: (index into the buffer, POC, long-term).
    pub pics: Vec<(usize, i32, bool)>,
    /// Indices into `pics` for `RefPicSetStCurrBefore`.
    pub st_curr_before: Vec<RefIdx>,
    /// `RefPicSetStCurrAfter`.
    pub st_curr_after: Vec<RefIdx>,
    /// `RefPicSetLtCurr`.
    pub lt_curr: Vec<RefIdx>,
    /// How many references the RPS names that the buffer does not have.
    pub missing: usize,
}

/// The decoded picture buffer.
#[derive(Debug, Clone)]
pub struct Dpb<T> {
    /// The pictures, in the order they were decoded.
    pub entries: Vec<DpbEntry<T>>,
}

impl<T> Default for Dpb<T> {
    fn default() -> Self {
        Self { entries: Vec::new() }
    }
}

impl<T> Dpb<T> {
    /// An empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply the RPS of the current picture (8.3.2): mark the pictures it keeps (short- or long-term), release the others, and find
    /// the ones the current picture predicts from.
    pub fn apply_rps(&mut self, rps: &Rps, max_lsb: u32, is_irap_no_rasl_output: bool) -> RefSet {
        let mut set = RefSet::default();
        if is_irap_no_rasl_output {
            // All reference pictures are marked as unused.
            for e in &mut self.entries {
                e.marking = Marking::Unused;
            }
        }
        let find_lt = |entries: &[DpbEntry<T>], poc: i32, msb: bool| {
            entries.iter().position(|e| {
                e.marking != Marking::Unused && if msb { e.poc == poc } else { e.poc.rem_euclid(max_lsb as i32) == poc.rem_euclid(max_lsb as i32) }
            })
        };
        // Long-term first: a picture that is long-term now stays out of the short-term search.
        let mut keep = alloc::vec![false; self.entries.len()];
        let mut lt_found: Vec<(bool, Option<usize>, i32)> = Vec::new();
        for (list, curr) in [(&rps.lt_curr, true), (&rps.lt_foll, false)] {
            for &(poc, msb) in list.iter() {
                let idx = find_lt(&self.entries, poc, msb);
                if let Some(i) = idx {
                    keep[i] = true;
                }
                lt_found.push((curr, idx, poc));
            }
        }
        let st = |entries: &[DpbEntry<T>], poc: i32| entries.iter().position(|e| e.marking == Marking::ShortTerm && e.poc == poc);
        let mut st_found: Vec<(u8, Option<usize>, i32)> = Vec::new();
        for (kind, list) in [(0u8, &rps.st_curr_before), (1, &rps.st_curr_after), (2, &rps.st_foll)] {
            for &poc in list.iter() {
                let idx = st(&self.entries, poc).filter(|i| !keep[*i]);
                if let Some(i) = idx {
                    keep[i] = true;
                }
                st_found.push((kind, idx, poc));
            }
        }
        // Marking: the long-term ones become long-term; whatever the RPS does not name is released.
        for (_, idx, _) in &lt_found {
            if let Some(i) = idx {
                self.entries[*i].marking = Marking::LongTerm;
            }
        }
        for (i, e) in self.entries.iter_mut().enumerate() {
            if !keep[i] {
                e.marking = Marking::Unused;
            }
        }
        // The sets of the current picture.
        let slot_of = |set: &mut RefSet, i: usize| -> RefIdx {
            if let Some(p) = set.pics.iter().position(|(bi, _, _)| *bi == i) {
                return p;
            }
            set.pics.push((i, self.entries[i].poc, self.entries[i].marking == Marking::LongTerm));
            set.pics.len() - 1
        };
        for (kind, idx, _) in st_found {
            match (kind, idx) {
                (0, Some(i)) => {
                    let s = slot_of(&mut set, i);
                    set.st_curr_before.push(s);
                }
                (1, Some(i)) => {
                    let s = slot_of(&mut set, i);
                    set.st_curr_after.push(s);
                }
                (2, Some(i)) => {
                    slot_of(&mut set, i);
                }
                (2, None) => {}
                _ => set.missing += 1,
            }
        }
        for (curr, idx, _) in lt_found {
            match (curr, idx) {
                (true, Some(i)) => {
                    let s = slot_of(&mut set, i);
                    set.lt_curr.push(s);
                }
                (false, Some(i)) => {
                    slot_of(&mut set, i);
                }
                (true, None) => set.missing += 1,
                (false, None) => {}
            }
        }
        set
    }

    /// Release the pictures that are neither needed for output nor used for reference.
    pub fn sweep(&mut self) {
        self.entries.retain(|e| e.needed_for_output || e.marking != Marking::Unused);
    }

    /// Pictures waiting for output.
    pub fn waiting(&self) -> usize {
        self.entries.iter().filter(|e| e.needed_for_output).count()
    }

    /// "Bumping" (C.5.2.4): output the waiting picture with the smallest POC. Returns its index in the buffer.
    pub fn bump_index(&self) -> Option<usize> {
        self.entries.iter().enumerate().filter(|(_, e)| e.needed_for_output).min_by_key(|(_, e)| e.poc).map(|(i, _)| i)
    }

    /// Whether the buffer must bump before the current picture (C.5.2.2): too many pictures wait, the latency limit is reached, or the
    /// buffer is full.
    pub fn must_bump(&self, max_reorder: usize, max_latency_plus1: u32, dpb_size: usize) -> bool {
        let waiting = self.waiting();
        if waiting == 0 {
            return false;
        }
        let latency_hit = max_latency_plus1 != 0 && {
            let limit = max_reorder as u32 + max_latency_plus1 - 1;
            self.entries.iter().any(|e| e.needed_for_output && e.latency >= limit)
        };
        waiting > max_reorder || latency_hit || self.entries.len() >= dpb_size
    }

    /// After the current picture was added (C.5.2.3): count latency for the waiting pictures that follow it in output order.
    pub fn count_latency(&mut self, cur_poc: i32) {
        for e in &mut self.entries {
            if e.needed_for_output && e.poc > cur_poc {
                e.latency += 1;
            }
        }
    }
}

/// Build `RefPicList0` and `RefPicList1` of a slice (8.3.4) as indices into [`RefSet::pics`]. A list is empty for the slices that do not
/// use it; an RPS with no usable picture gives empty lists (the caller treats the picture as undecodable).
pub fn build_ref_lists(h: &SliceHeader, set: &RefSet) -> [Vec<RefIdx>; 2] {
    let mut out: [Vec<RefIdx>; 2] = [Vec::new(), Vec::new()];
    if h.slice_type == SliceType::I {
        return out;
    }
    let total = set.st_curr_before.len() + set.st_curr_after.len() + set.lt_curr.len();
    if total == 0 {
        return out;
    }
    for l in 0..if h.slice_type == SliceType::B { 2 } else { 1 } {
        let n = h.num_ref_idx[l] as usize;
        let temp_len = n.max(total);
        let (first, second) = if l == 0 { (&set.st_curr_before, &set.st_curr_after) } else { (&set.st_curr_after, &set.st_curr_before) };
        let mut temp: Vec<RefIdx> = Vec::with_capacity(temp_len);
        while temp.len() < temp_len {
            for &i in first.iter().chain(second.iter()).chain(set.lt_curr.iter()) {
                if temp.len() < temp_len {
                    temp.push(i);
                }
            }
        }
        out[l] = (0..n)
            .map(|i| match &h.list_entry[l] {
                Some(e) => temp[e[i] as usize],
                None => temp[i],
            })
            .collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poc_wraps_with_the_msb() {
        let mut s = PocState::default();
        let max = 16;
        assert_eq!(s.poc(0, max, true), 0);
        s.remember(0, max);
        let mut prev = 0;
        for p in 1..40 {
            let lsb = (p % 16) as u32;
            let poc = s.poc(lsb, max, false);
            assert_eq!(poc, p, "lsb {lsb} after {prev}");
            s.remember(poc, max);
            prev = p;
        }
        // A step back across a wrap (a B picture before the anchor).
        let poc = s.poc((37 % 16) as u32, max, false);
        assert_eq!(poc, 37);
    }

    fn entry(poc: i32, marking: Marking, out: bool) -> DpbEntry<u8> {
        DpbEntry { poc, marking, needed_for_output: out, latency: 0, payload: poc as u8, pts: poc as i64 }
    }

    #[test]
    fn the_rps_keeps_what_it_names_and_releases_the_rest() {
        let mut d: Dpb<u8> = Dpb::new();
        for p in [0, 4, 2, 8] {
            d.entries.push(entry(p, Marking::ShortTerm, false));
        }
        d.entries.push(entry(-30, Marking::ShortTerm, false));
        let rps = Rps { st_curr_before: alloc::vec![4, 2], st_curr_after: alloc::vec![8], st_foll: alloc::vec![0], lt_curr: alloc::vec![(-30, true)], lt_foll: alloc::vec![] };
        let set = d.apply_rps(&rps, 64, false);
        assert_eq!(set.missing, 0);
        assert_eq!(set.pics.len(), 5);
        assert!(d.entries.iter().all(|e| e.marking != Marking::Unused));
        assert_eq!(d.entries[4].marking, Marking::LongTerm);
        // A name the buffer does not have is counted as missing; a picture no longer named is released.
        let rps = Rps { st_curr_before: alloc::vec![4, 3], ..Rps::default() };
        let set = d.apply_rps(&rps, 64, false);
        assert_eq!(set.missing, 1);
        assert_eq!(set.st_curr_before.len(), 1);
        assert_eq!(d.entries.iter().filter(|e| e.marking != Marking::Unused).count(), 1);
        d.sweep();
        assert_eq!(d.entries.len(), 1);
    }

    #[test]
    fn output_order_follows_poc_with_reorder_and_latency_limits() {
        let mut d: Dpb<u8> = Dpb::new();
        for p in [8, 4, 2] {
            d.entries.push(entry(p, Marking::ShortTerm, true));
        }
        assert!(!d.must_bump(3, 0, 16));
        assert!(d.must_bump(2, 0, 16), "three wait and two are allowed");
        assert_eq!(d.entries[d.bump_index().unwrap()].poc, 2);
        // The buffer being full bumps too.
        assert!(d.must_bump(8, 0, 3));
        // Latency: a picture that has waited long enough goes.
        d.entries[0].latency = 5;
        assert!(!d.must_bump(8, 3, 16), "limit is reorder + latency - 1 = 10 here, so 5 is not enough");
        d.entries[0].latency = 10;
        assert!(d.must_bump(8, 3, 16));
    }
}
