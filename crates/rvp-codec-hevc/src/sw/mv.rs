//! Motion vector prediction (8.5.3.2): merge candidates, AMVP candidates and the temporal candidate.
use super::ctu::Dec;
use super::frame::Mv;
use super::pic::MvField;
use crate::slice::SliceType;

/// Partition shapes of an inter coding unit (Table 7-10).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PartMode {
    P2Nx2N,
    P2NxN,
    PNx2N,
    PNxN,
    P2NxnU,
    P2NxnD,
    PnLx2N,
    PnRx2N,
}

fn scale(mv: Mv, td: i32, tb: i32) -> Mv {
    let td = td.clamp(-128, 127);
    let tb = tb.clamp(-128, 127);
    if td == 0 {
        return mv;
    }
    let tx = (16384 + (td.abs() >> 1)) / td;
    let dsf = ((tb * tx + 32) >> 6).clamp(-4096, 4095);
    let f = |c: i16| -> i16 {
        let p = dsf * c as i32;
        (p.signum() * ((p.abs() + 127) >> 8)).clamp(-32768, 32767) as i16
    };
    Mv { x: f(mv.x), y: f(mv.y) }
}

fn same(a: &MvField, b: &MvField) -> bool {
    a.ref_idx == b.ref_idx
        && (a.ref_idx[0] < 0 || a.mv[0] == b.mv[0])
        && (a.ref_idx[1] < 0 || a.mv[1] == b.mv[1])
}

impl Dec<'_> {
    /// 6.4.2: whether the prediction block covering luma (`xn`, `yn`) is available to the one being decoded, and inter coded.
    #[allow(clippy::too_many_arguments)]
    fn pb_available(
        &self,
        cb: (i32, i32, i32),
        pb: (i32, i32, i32, i32),
        part_idx: usize,
        xn: i32,
        yn: i32,
    ) -> bool {
        let (xcb, ycb, ncbs) = cb;
        let (xpb, ypb, w, h) = pb;
        let same_cb = xcb <= xn && ycb <= yn && xcb + ncbs > xn && ycb + ncbs > yn;
        let avail = if same_cb {
            !(w << 1 == ncbs && h << 1 == ncbs && part_idx == 1 && ycb + h <= yn && xcb + w > xn)
        } else {
            self.pic.available(xpb, ypb, xn, yn)
        };
        avail && self.cell_mode(xn, yn) == 2
    }

    fn mvf_at(&self, x: i32, y: i32) -> MvField {
        self.pic.mvf[(y as usize >> 2) * self.pic.w4 + (x as usize >> 2)]
    }

    /// The motion of a merge candidate: the vectors and references of the candidate with the POCs filled in.
    fn finish(&self, mut m: MvField) -> MvField {
        for l in 0..2 {
            if m.ref_idx[l] >= 0 {
                m.ref_poc[l] = self.ref_poc[l][m.ref_idx[l] as usize];
            } else {
                m.mv[l] = Mv::default();
                m.ref_poc[l] = 0;
            }
        }
        m
    }

    /// 8.5.3.2.9: the temporal vector for list `x` and reference index `ref_idx` of the block at (`xpb`, `ypb`).
    fn temporal_mv(&self, xpb: i32, ypb: i32, w: i32, h: i32, ref_idx: usize, x: usize) -> Option<Mv> {
        let col_i = self.col_frame?;
        let col = self.refs[col_i];
        let try_pos = |xc: i32, yc: i32| -> Option<Mv> {
            let (xc, yc) = ((xc >> 4) << 4, (yc >> 4) << 4);
            let m = col.motion[(yc as usize >> 2) * col.w4 + (xc as usize >> 2)];
            if m.flags & 3 == 0 {
                return None;
            }
            // Which of the collocated block's vectors to use.
            let list_col = if m.flags & 1 == 0 {
                1
            } else if m.flags & 2 == 0 {
                0
            } else if self.no_backward_pred {
                x
            } else {
                // collocated_from_l0_flag selects the list (N = collocated_from_l0_flag).
                self.hdr.collocated_from_l0 as usize
            };
            let col_lt = m.flags & (4 << list_col) != 0;
            let cur_lt = self.ref_lt[x][ref_idx];
            if col_lt != cur_lt {
                return None;
            }
            let mv = m.mv[list_col];
            let col_diff = col.poc - m.ref_poc[list_col];
            let cur_diff = self.cur_poc - self.ref_poc[x][ref_idx];
            if cur_lt || col_diff == cur_diff || col_diff == 0 {
                Some(mv)
            } else {
                Some(scale(mv, col_diff, cur_diff))
            }
        };
        let (xbr, ybr) = (xpb + w, ypb + h);
        if (ypb >> self.pic.ctb_log2) == (ybr >> self.pic.ctb_log2)
            && ybr < self.pic.height as i32
            && xbr < self.pic.width as i32
        {
            if let Some(v) = try_pos(xbr, ybr) {
                return Some(v);
            }
        }
        try_pos(xpb + (w >> 1), ypb + (h >> 1))
    }

    /// 8.5.3.2.2: the merge candidate `merge_idx` of the prediction block.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn merge_motion(
        &self,
        cb: (i32, i32, i32),
        pb: (i32, i32, i32, i32),
        part_idx: usize,
        part: PartMode,
        merge_idx: usize,
    ) -> MvField {
        let (xcb, ycb, ncbs) = cb;
        let orig = (pb.2, pb.3);
        let par = self.pps.log2_parallel_merge_level_minus2 as u32 + 2;
        // With a parallel merge level above 4x4, all prediction blocks of an 8x8 coding unit share the list of the 2Nx2N unit.
        let (pb, part_idx, part) = if par > 2 && ncbs == 8 {
            ((xcb, ycb, ncbs, ncbs), 0, PartMode::P2Nx2N)
        } else {
            (pb, part_idx, part)
        };
        let (xpb, ypb, w, h) = pb;
        let max_cand = 5 - self.hdr.five_minus_max_num_merge_cand as usize;
        let mut list: [MvField; 6] = [MvField::NONE; 6];
        let mut n = 0usize;
        let mer = |xn: i32, yn: i32| (xpb >> par) == (xn >> par) && (ypb >> par) == (yn >> par);
        let get = |xn: i32, yn: i32| -> Option<MvField> {
            if mer(xn, yn) || !self.pb_available(cb, pb, part_idx, xn, yn) {
                None
            } else {
                Some(self.mvf_at(xn, yn))
            }
        };
        // Spatial candidates (8.5.3.2.3). The comparisons look at the neighbours as they are before any of them was dropped.
        let a1 = if matches!(part, PartMode::PNx2N | PartMode::PnLx2N | PartMode::PnRx2N) && part_idx == 1 {
            None
        } else {
            get(xpb - 1, ypb + h - 1)
        };
        let b1 = if matches!(part, PartMode::P2NxN | PartMode::P2NxnU | PartMode::P2NxnD) && part_idx == 1 {
            None
        } else {
            get(xpb + w - 1, ypb - 1)
        };
        if let Some(m) = a1 {
            list[n] = m;
            n += 1;
        }
        if let Some(m) = b1.filter(|b| !a1.is_some_and(|a| same(&a, b))) {
            list[n] = m;
            n += 1;
        }
        if let Some(m) = get(xpb + w, ypb - 1).filter(|b| !b1.is_some_and(|x| same(&x, b))) {
            list[n] = m;
            n += 1;
        }
        if let Some(m) = get(xpb - 1, ypb + h).filter(|a| !a1.is_some_and(|x| same(&x, a))) {
            list[n] = m;
            n += 1;
        }
        if n < 4 {
            if let Some(m) = get(xpb - 1, ypb - 1)
                .filter(|b| !a1.is_some_and(|x| same(&x, b)) && !b1.is_some_and(|x| same(&x, b)))
            {
                list[n] = m;
                n += 1;
            }
        }
        // Temporal candidate.
        {
            if self.hdr.temporal_mvp && n < 5 {
                let t0 = self.temporal_mv(xpb, ypb, w, h, 0, 0);
                let t1 = if self.hdr.slice_type == SliceType::B {
                    self.temporal_mv(xpb, ypb, w, h, 0, 1)
                } else {
                    None
                };
                if t0.is_some() || t1.is_some() {
                    let mut m = MvField::NONE;
                    if let Some(v) = t0 {
                        m.mv[0] = v;
                        m.ref_idx[0] = 0;
                    }
                    if let Some(v) = t1 {
                        m.mv[1] = v;
                        m.ref_idx[1] = 0;
                    }
                    list[n] = m;
                    n += 1;
                }
            }
        }
        let n = n.min(max_cand);
        let mut n = n;
        // Combined bi-predictive candidates.
        if self.hdr.slice_type == SliceType::B && n > 1 && n < max_cand {
            const L0: [usize; 12] = [0, 1, 0, 2, 1, 2, 0, 3, 1, 3, 2, 3];
            const L1: [usize; 12] = [1, 0, 2, 0, 2, 1, 3, 0, 3, 1, 3, 2];
            let orig_n = n;
            for comb in 0..orig_n * (orig_n - 1) {
                let (c0, c1) = (list[L0[comb]], list[L1[comb]]);
                if c0.ref_idx[0] >= 0 && c1.ref_idx[1] >= 0 {
                    let p0 = self.ref_poc[0][c0.ref_idx[0] as usize];
                    let p1 = self.ref_poc[1][c1.ref_idx[1] as usize];
                    if p0 != p1 || c0.mv[0] != c1.mv[1] {
                        list[n] = MvField {
                            mv: [c0.mv[0], c1.mv[1]],
                            ref_idx: [c0.ref_idx[0], c1.ref_idx[1]],
                            ref_poc: [0, 0],
                        };
                        n += 1;
                        if n == max_cand {
                            break;
                        }
                    }
                }
            }
        }
        // Zero candidates.
        let num_ref = if self.hdr.slice_type == SliceType::P {
            self.hdr.num_ref_idx[0] as usize
        } else {
            self.hdr.num_ref_idx[0].min(self.hdr.num_ref_idx[1]) as usize
        };
        let mut zero_idx = 0usize;
        while n < max_cand {
            let r = if zero_idx < num_ref { zero_idx as i8 } else { 0 };
            list[n] = MvField {
                mv: [Mv::default(); 2],
                ref_idx: [r, if self.hdr.slice_type == SliceType::B { r } else { -1 }],
                ref_poc: [0, 0],
            };
            n += 1;
            zero_idx += 1;
        }
        let mut m = list[merge_idx.min(5)];
        if m.ref_idx[0] >= 0 && m.ref_idx[1] >= 0 && orig.0 + orig.1 == 12 {
            m.ref_idx[1] = -1;
        }
        self.finish(m)
    }

    /// 8.5.3.2.6: the motion vector predictor `mvp_flag` for list `x` and `ref_idx` of the prediction block.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn amvp(
        &self,
        cb: (i32, i32, i32),
        pb: (i32, i32, i32, i32),
        part_idx: usize,
        x: usize,
        ref_idx: usize,
        mvp_flag: usize,
    ) -> Mv {
        let (xpb, ypb, w, h) = pb;
        let y = 1 - x;
        let target_poc = self.ref_poc[x][ref_idx];
        let target_lt = self.ref_lt[x][ref_idx];
        let avail = |xn: i32, yn: i32| self.pb_available(cb, pb, part_idx, xn, yn);
        // Candidate with the same reference picture (no scaling).
        let direct = |m: &MvField| -> Option<Mv> {
            if m.ref_idx[x] >= 0 && self.ref_poc[x][m.ref_idx[x] as usize] == target_poc {
                Some(m.mv[x])
            } else if m.ref_idx[y] >= 0 && self.ref_poc[y][m.ref_idx[y] as usize] == target_poc {
                Some(m.mv[y])
            } else {
                None
            }
        };
        // Candidate with another picture, scaled by the distance when both are short-term.
        let scaled = |m: &MvField| -> Option<Mv> {
            for l in [x, y] {
                if m.ref_idx[l] >= 0 {
                    let ri = m.ref_idx[l] as usize;
                    if self.ref_lt[l][ri] == target_lt {
                        let mv = m.mv[l];
                        let rp = self.ref_poc[l][ri];
                        if !self.ref_lt[l][ri] && !target_lt && rp != target_poc {
                            return Some(scale(mv, self.cur_poc - rp, self.cur_poc - target_poc));
                        }
                        return Some(mv);
                    }
                }
            }
            None
        };
        let a_pos = [(xpb - 1, ypb + h), (xpb - 1, ypb + h - 1)];
        let b_pos = [(xpb + w, ypb - 1), (xpb + w - 1, ypb - 1), (xpb - 1, ypb - 1)];
        let a_avail: [bool; 2] = [avail(a_pos[0].0, a_pos[0].1), avail(a_pos[1].0, a_pos[1].1)];
        let is_scaled = a_avail[0] || a_avail[1];
        let mut a: Option<Mv> = None;
        for k in 0..2 {
            if a.is_none() && a_avail[k] {
                a = direct(&self.mvf_at(a_pos[k].0, a_pos[k].1));
            }
        }
        for k in 0..2 {
            if a.is_none() && a_avail[k] {
                a = scaled(&self.mvf_at(a_pos[k].0, a_pos[k].1));
            }
        }
        let b_avail: [bool; 3] =
            [avail(b_pos[0].0, b_pos[0].1), avail(b_pos[1].0, b_pos[1].1), avail(b_pos[2].0, b_pos[2].1)];
        let mut b: Option<Mv> = None;
        for k in 0..3 {
            if b.is_none() && b_avail[k] {
                b = direct(&self.mvf_at(b_pos[k].0, b_pos[k].1));
            }
        }
        if !is_scaled && b.is_some() {
            a = b;
        }
        if !is_scaled {
            b = None;
            for k in 0..3 {
                if b.is_none() && b_avail[k] {
                    b = scaled(&self.mvf_at(b_pos[k].0, b_pos[k].1));
                }
            }
        }
        let mut cands: [Mv; 3] = [Mv::default(); 3];
        let mut n = 0;
        if let Some(v) = a {
            cands[n] = v;
            n += 1;
        }
        if let Some(v) = b {
            if !(a == Some(v)) {
                cands[n] = v;
                n += 1;
            }
        }
        if n < 2 && self.hdr.temporal_mvp {
            if let Some(v) = self.temporal_mv(xpb, ypb, w, h, ref_idx, x) {
                cands[n] = v;
                n += 1;
            }
        }
        let _ = n;
        cands[mvp_flag.min(2)]
    }
}
