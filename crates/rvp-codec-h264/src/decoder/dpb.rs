//! Reference picture lists (8.2.4) and reference picture marking (8.2.5), for frames.
use super::picture::{Picture, RefState};
use crate::params::{DecRefPicMarking, Mmco, SliceHeader, SliceType, Sps};
use alloc::vec::Vec;

/// `FrameNumWrap` of a short-term picture relative to the current `frame_num`.
#[inline]
pub fn frame_num_wrap(p: &Picture, cur_frame_num: u32, max_frame_num: u32) -> i32 {
    if p.frame_num > cur_frame_num { p.frame_num as i32 - max_frame_num as i32 } else { p.frame_num as i32 }
}

/// A reference picture list: indices into the DPB, `None` for "no reference picture".
pub type RefList = Vec<Option<usize>>;

/// Build `RefPicList0` and `RefPicList1` for a slice (initialisation then modification).
pub fn build_ref_lists(dpb: &[Picture], h: &SliceHeader, sps: &Sps, cur_poc: i32) -> [RefList; 2] {
    let max_frame_num = 1u32 << sps.log2_max_frame_num;
    let cur_fn = h.frame_num;
    let mut l0: Vec<usize> = Vec::new();
    let mut l1: Vec<usize> = Vec::new();
    let mut longs: Vec<usize> = (0..dpb.len()).filter(|&i| dpb[i].ref_state == RefState::Long).collect();
    longs.sort_by_key(|&i| dpb[i].long_term_idx);
    match h.slice_type {
        SliceType::P | SliceType::Sp => {
            let mut shorts: Vec<usize> =
                (0..dpb.len()).filter(|&i| dpb[i].ref_state == RefState::Short).collect();
            shorts.sort_by_key(|&i| core::cmp::Reverse(frame_num_wrap(&dpb[i], cur_fn, max_frame_num)));
            l0.extend(shorts);
            l0.extend(longs.iter().copied());
        }
        SliceType::B => {
            let shorts: Vec<usize> = (0..dpb.len())
                .filter(|&i| dpb[i].ref_state == RefState::Short && !dpb[i].non_existing)
                .collect();
            let mut before: Vec<usize> = shorts.iter().copied().filter(|&i| dpb[i].poc < cur_poc).collect();
            let mut after: Vec<usize> = shorts.iter().copied().filter(|&i| dpb[i].poc > cur_poc).collect();
            before.sort_by_key(|&i| core::cmp::Reverse(dpb[i].poc));
            after.sort_by_key(|&i| dpb[i].poc);
            l0.extend(before.iter().copied());
            l0.extend(after.iter().copied());
            l0.extend(longs.iter().copied());
            l1.extend(after.iter().copied());
            l1.extend(before.iter().copied());
            l1.extend(longs.iter().copied());
            if l1.len() > 1 && l1 == l0 {
                l1.swap(0, 1);
            }
        }
        _ => {}
    }
    let mut out: [RefList; 2] = [Vec::new(), Vec::new()];
    let lists = [l0, l1];
    for list in 0..2 {
        let n = if list == 0 { h.num_ref_idx_l0_active } else { h.num_ref_idx_l1_active } as usize;
        let mut v: RefList = lists[list].iter().map(|&i| Some(i)).collect();
        v.truncate(n);
        v.resize(n, None);
        if let Some(ops) = &h.ref_list_mod[list] {
            modify_list(&mut v, ops, dpb, cur_fn, max_frame_num, n);
        }
        out[list] = v;
    }
    out
}

fn modify_list(
    list: &mut RefList,
    ops: &[crate::params::RefListMod],
    dpb: &[Picture],
    cur_fn: u32,
    max_frame_num: u32,
    n: usize,
) {
    let max_pic_num = max_frame_num as i32;
    let curr_pic_num = cur_fn as i32;
    let mut pred = curr_pic_num;
    let mut ref_idx = 0usize;
    list.push(None); // temporary extra entry
    for op in ops {
        if ref_idx >= n {
            break;
        }
        let target: Option<usize>;
        let pic_num_f = |idx: Option<usize>| -> i32 {
            match idx {
                Some(i) if dpb[i].ref_state == RefState::Short => {
                    frame_num_wrap(&dpb[i], cur_fn, max_frame_num)
                }
                _ => max_pic_num,
            }
        };
        let wanted_num: i32;
        if op.idc < 2 {
            let abs_diff = op.value as i64 + 1;
            let no_wrap = if op.idc == 0 {
                let v = pred as i64 - abs_diff;
                if v < 0 { v + max_pic_num as i64 } else { v }
            } else {
                let v = pred as i64 + abs_diff;
                if v >= max_pic_num as i64 { v - max_pic_num as i64 } else { v }
            } as i32;
            pred = no_wrap;
            wanted_num = if no_wrap > curr_pic_num { no_wrap - max_pic_num } else { no_wrap };
            target = (0..dpb.len()).find(|&i| {
                dpb[i].ref_state == RefState::Short
                    && frame_num_wrap(&dpb[i], cur_fn, max_frame_num) == wanted_num
            });
        } else {
            wanted_num = op.value as i32;
            target = (0..dpb.len())
                .find(|&i| dpb[i].ref_state == RefState::Long && dpb[i].long_term_idx == op.value);
        }
        let Some(t) = target else { continue };
        // Insert at ref_idx, shifting the rest up, then remove the later duplicate.
        for c in (ref_idx + 1..=n).rev() {
            list[c] = list[c - 1];
        }
        list[ref_idx] = Some(t);
        ref_idx += 1;
        let mut nidx = ref_idx;
        for c in ref_idx..=n {
            let dup = if op.idc < 2 {
                pic_num_f(list[c]) == wanted_num
            } else {
                match list[c] {
                    Some(i) => {
                        dpb[i].ref_state == RefState::Long && dpb[i].long_term_idx == wanted_num as u32
                    }
                    None => false,
                }
            };
            if !dup {
                list[nidx] = list[c];
                nidx += 1;
            }
        }
    }
    list.truncate(n);
}

/// State shared by the marking operations.
pub struct MarkCtx {
    /// `max_num_ref_frames` (at least 1).
    pub max_refs: usize,
    /// `frame_num` of the current picture.
    pub frame_num: u32,
    /// `MaxFrameNum`.
    pub max_frame_num: u32,
}

/// Mark all reference pictures unused.
pub fn clear_all_refs(dpb: &mut [Picture]) {
    for p in dpb.iter_mut() {
        p.ref_state = RefState::Unused;
    }
}

/// The sliding window process (8.2.5.3): free a slot if the reference set is full.
pub fn sliding_window(dpb: &mut [Picture], ctx: &MarkCtx) {
    let used = dpb.iter().filter(|p| p.is_ref()).count();
    if used >= ctx.max_refs {
        let oldest = (0..dpb.len())
            .filter(|&i| dpb[i].ref_state == RefState::Short)
            .min_by_key(|&i| frame_num_wrap(&dpb[i], ctx.frame_num, ctx.max_frame_num));
        if let Some(i) = oldest {
            dpb[i].ref_state = RefState::Unused;
        }
    }
}

/// Apply the reference marking of the current picture (8.2.5.1). Returns `true` if a `ClearAll` operation
/// (MMCO 5) was executed. `cur` is the picture just decoded; its marking is set here.
pub fn mark_current(
    dpb: &mut [Picture],
    cur: &mut Picture,
    marking: &DecRefPicMarking,
    idr: bool,
    ctx: &MarkCtx,
    max_long_term_idx: &mut i32,
) -> bool {
    let mut mmco5 = false;
    cur.ref_state = RefState::Short;
    if idr {
        clear_all_refs(dpb);
        if marking.long_term_reference {
            cur.ref_state = RefState::Long;
            cur.long_term_idx = 0;
            *max_long_term_idx = 0;
        } else {
            *max_long_term_idx = -1;
        }
        return false;
    }
    if !marking.adaptive {
        sliding_window(dpb, ctx);
        return false;
    }
    let curr_pic_num = ctx.frame_num as i32;
    for op in &marking.ops {
        match *op {
            Mmco::ShortTermUnused { difference_of_pic_nums_minus1 } => {
                let x = curr_pic_num - (difference_of_pic_nums_minus1 as i32 + 1);
                if let Some(p) = dpb.iter_mut().find(|p| {
                    p.ref_state == RefState::Short && frame_num_wrap(p, ctx.frame_num, ctx.max_frame_num) == x
                }) {
                    p.ref_state = RefState::Unused;
                }
            }
            Mmco::LongTermUnused { long_term_pic_num } => {
                if let Some(p) = dpb
                    .iter_mut()
                    .find(|p| p.ref_state == RefState::Long && p.long_term_idx == long_term_pic_num)
                {
                    p.ref_state = RefState::Unused;
                }
            }
            Mmco::AssignLongTerm { difference_of_pic_nums_minus1, long_term_frame_idx } => {
                let x = curr_pic_num - (difference_of_pic_nums_minus1 as i32 + 1);
                // Free the index if another frame holds it.
                for p in dpb.iter_mut() {
                    if p.ref_state == RefState::Long && p.long_term_idx == long_term_frame_idx {
                        p.ref_state = RefState::Unused;
                    }
                }
                if let Some(p) = dpb.iter_mut().find(|p| {
                    p.ref_state == RefState::Short && frame_num_wrap(p, ctx.frame_num, ctx.max_frame_num) == x
                }) {
                    p.ref_state = RefState::Long;
                    p.long_term_idx = long_term_frame_idx;
                }
            }
            Mmco::MaxLongTermIdx { max_long_term_frame_idx_plus1 } => {
                *max_long_term_idx = max_long_term_frame_idx_plus1 as i32 - 1;
                for p in dpb.iter_mut() {
                    if p.ref_state == RefState::Long && p.long_term_idx as i32 > *max_long_term_idx {
                        p.ref_state = RefState::Unused;
                    }
                }
            }
            Mmco::ClearAll => {
                clear_all_refs(dpb);
                *max_long_term_idx = -1;
                mmco5 = true;
            }
            Mmco::CurrentLongTerm { long_term_frame_idx } => {
                for p in dpb.iter_mut() {
                    if p.ref_state == RefState::Long && p.long_term_idx == long_term_frame_idx {
                        p.ref_state = RefState::Unused;
                    }
                }
                cur.ref_state = RefState::Long;
                cur.long_term_idx = long_term_frame_idx;
            }
        }
    }
    mmco5
}
