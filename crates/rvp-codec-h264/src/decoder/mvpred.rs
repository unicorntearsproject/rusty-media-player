//! Motion vector prediction (8.4.1.3) and the neighbour motion access it needs.
use super::mbinfo::blk_order;
use super::slice::SliceDecoder;

/// Motion data of a neighbouring partition.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Nb {
    /// The partition exists (is available, whether or not it is inter coded).
    pub avail: bool,
    /// Reference index, `-1` if unavailable, intra or not using the list.
    pub ref_idx: i32,
    /// Motion vector (zero when `ref_idx < 0`).
    pub mv: [i32; 2],
}

const UNAVAILABLE: Nb = Nb { avail: false, ref_idx: -1, mv: [0, 0] };

#[inline]
fn median(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).min(a.min(b).max(c))
}

impl SliceDecoder<'_> {
    /// Motion data at 4x4 block `(bx, by)` relative to the current macroblock (may be -1 or 4 for neighbouring
    /// macroblocks), for list `list`. `cur_order` is the decoding order of the current partition's first 4x4 block;
    /// blocks inside the macroblock that are not decoded before it are unavailable.
    pub(crate) fn nb_motion(&self, list: usize, bx: i32, by: i32, cur_order: usize) -> Nb {
        let inside = (0..4).contains(&bx) && (0..4).contains(&by);
        let mb = if inside {
            if blk_order(bx as usize, by as usize) >= cur_order {
                return UNAVAILABLE;
            }
            self.mb_addr
        } else if by < 0 {
            let m = if bx < 0 {
                self.nd
            } else if bx > 3 {
                self.nc
            } else {
                self.nb
            };
            match m {
                Some(a) => a,
                None => return UNAVAILABLE,
            }
        } else if bx < 0 && by < 4 {
            match self.na {
                Some(a) => a,
                None => return UNAVAILABLE,
            }
        } else {
            return UNAVAILABLE;
        };
        // Position in the picture's 4x4 grid.
        let w4 = self.mbw * 4;
        let (gx, gy) = if inside {
            (self.mb_x * 4 + bx as usize, self.mb_y * 4 + by as usize)
        } else {
            let (mx, my) = (mb % self.mbw, mb / self.mbw);
            (mx * 4 + (bx.rem_euclid(4)) as usize, my * 4 + (by.rem_euclid(4)) as usize)
        };
        let r = self.cur.ref_idx[list][(gy >> 1) * (self.mbw * 2) + (gx >> 1)] as i32;
        if r < 0 {
            return Nb { avail: true, ref_idx: -1, mv: [0, 0] };
        }
        let mv = self.cur.mv[list][gy * w4 + gx];
        Nb { avail: true, ref_idx: r, mv: [mv[0] as i32, mv[1] as i32] }
    }

    /// Predict the motion vector of the partition at `(bx, by)` of size `w` x `h` (4x4 units) for `list` with
    /// reference index `ref_idx`. `dir` selects the directional rules: 1 for 16x8, 2 for 8x16, 0 otherwise.
    pub(crate) fn pred_mv(
        &self,
        list: usize,
        ref_idx: i32,
        bx: usize,
        by: usize,
        w: usize,
        _h: usize,
        dir: u8,
    ) -> [i32; 2] {
        let order = blk_order(bx, by);
        let (bxi, byi) = (bx as i32, by as i32);
        let a = self.nb_motion(list, bxi - 1, byi, order);
        let b = self.nb_motion(list, bxi, byi - 1, order);
        let mut c = self.nb_motion(list, bxi + w as i32, byi - 1, order);
        if !c.avail {
            c = self.nb_motion(list, bxi - 1, byi - 1, order);
        }
        match dir {
            1 => {
                if by == 0 && b.ref_idx == ref_idx {
                    return b.mv;
                }
                if by > 0 && a.ref_idx == ref_idx {
                    return a.mv;
                }
            }
            2 => {
                if bx == 0 && a.ref_idx == ref_idx {
                    return a.mv;
                }
                if bx > 0 && c.ref_idx == ref_idx {
                    return c.mv;
                }
            }
            _ => {}
        }
        self.median_pred(a, b, c, ref_idx)
    }

    /// The median prediction of 8.4.1.3.1.
    pub(crate) fn median_pred(&self, a: Nb, mut b: Nb, mut c: Nb, ref_idx: i32) -> [i32; 2] {
        if !b.avail && !c.avail && a.avail {
            b = a;
            c = a;
        }
        let matches = [a.ref_idx == ref_idx, b.ref_idx == ref_idx, c.ref_idx == ref_idx];
        let n = matches.iter().filter(|&&m| m).count();
        if n == 1 {
            return if matches[0] {
                a.mv
            } else if matches[1] {
                b.mv
            } else {
                c.mv
            };
        }
        [median(a.mv[0], b.mv[0], c.mv[0]), median(a.mv[1], b.mv[1], c.mv[1])]
    }

    /// Motion vector of a P_Skip macroblock (8.4.1.1).
    pub(crate) fn pskip_mv(&self) -> [i32; 2] {
        let a = self.nb_motion(0, -1, 0, 0);
        let b = self.nb_motion(0, 0, -1, 0);
        if !a.avail || !b.avail || (a.ref_idx == 0 && a.mv == [0, 0]) || (b.ref_idx == 0 && b.mv == [0, 0]) {
            return [0, 0];
        }
        self.pred_mv(0, 0, 0, 0, 4, 4, 0)
    }
}
