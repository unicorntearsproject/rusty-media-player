//! Inter macroblock parsing, motion storage and motion compensation.
use super::entropy::Entropy;
use super::inter::{self, PSTRIDE, Weights};
use super::mbinfo::*;
use super::slice::{RefInfo, SliceDecoder};
use crate::error::{Error, Result};
use crate::params::SliceType;

/// Prediction list usage of a partition: bit 0 list 0, bit 1 list 1.
type Pm = u8;

/// (shape, prediction modes) of the B macroblock types 1 to 21: shape 0 is 16x16, 1 is 16x8, 2 is 8x16.
fn b_type(raw: u32) -> (u8, [Pm; 2]) {
    match raw {
        1..=3 => (0, [raw as u8, 0]),
        _ => {
            const COMBO: [[Pm; 2]; 9] =
                [[1, 1], [2, 2], [1, 2], [2, 1], [1, 3], [2, 3], [3, 1], [3, 2], [3, 3]];
            let c = ((raw - 4) / 2) as usize;
            (if raw % 2 == 0 { 1 } else { 2 }, COMBO[c])
        }
    }
}

/// (sub-partition shape, prediction mode) of a sub-macroblock type: shape 0 is 8x8, 1 is 8x4, 2 is 4x8, 3 is 4x4.
fn sub_type(is_b: bool, raw: u32) -> (u8, Pm) {
    if !is_b {
        return (raw as u8, 1);
    }
    match raw {
        1..=3 => (0, raw as u8),
        4 => (1, 1),
        5 => (2, 1),
        6 => (1, 2),
        7 => (2, 2),
        8 => (1, 3),
        9 => (2, 3),
        10 => (3, 1),
        11 => (3, 2),
        _ => (3, 3),
    }
}

/// The sub-partitions `(x, y, w, h)` in 4x4 units relative to the 8x8 block.
fn sub_parts(shape: u8) -> &'static [(usize, usize, usize, usize)] {
    match shape {
        0 => &[(0, 0, 2, 2)],
        1 => &[(0, 0, 2, 1), (0, 1, 2, 1)],
        2 => &[(0, 0, 1, 2), (1, 0, 1, 2)],
        _ => &[(0, 0, 1, 1), (1, 0, 1, 1), (0, 1, 1, 1), (1, 1, 1, 1)],
    }
}

impl SliceDecoder<'_> {
    /// Number of active references in `list`.
    #[inline]
    pub(crate) fn num_ref(&self, list: usize) -> usize {
        self.refs[list].len()
    }

    /// Store a reference index (and the id of the referenced frame) for the 8x8 blocks covered by the partition.
    pub(crate) fn store_ref(&mut self, list: usize, bx: usize, by: usize, w: usize, h: usize, ref_idx: i32) {
        let w8 = self.mbw * 2;
        let id = if ref_idx >= 0 { self.refs[list][ref_idx as usize].uid } else { -1 };
        for y in by / 2..(by + h).div_ceil(2) {
            for x in bx / 2..(bx + w).div_ceil(2) {
                let i = (self.mb_y * 2 + y) * w8 + self.mb_x * 2 + x;
                self.cur.ref_idx[list][i] = ref_idx as i8;
                self.cur.ref_id[list][i] = id;
            }
        }
    }

    /// Store a motion vector for the 4x4 blocks covered by the partition.
    pub(crate) fn store_mv(&mut self, list: usize, bx: usize, by: usize, w: usize, h: usize, mv: [i32; 2]) {
        let w4 = self.mbw * 4;
        let v = [mv[0] as i16, mv[1] as i16];
        for y in by..by + h {
            for x in bx..bx + w {
                self.cur.mv[list][(self.mb_y * 4 + y) * w4 + self.mb_x * 4 + x] = v;
            }
        }
    }

    /// The reference index stored for the 8x8 block containing 4x4 block `(bx, by)` of the current macroblock.
    #[inline]
    fn ref_at(&self, list: usize, bx: usize, by: usize) -> i32 {
        self.cur.ref_idx[list][(self.mb_y * 2 + by / 2) * (self.mbw * 2) + self.mb_x * 2 + bx / 2] as i32
    }

    /// Saturating absolute `mvd` store for CABAC contexts.
    pub(crate) fn store_mvd(&mut self, list: usize, bx: usize, by: usize, w: usize, h: usize, mvd: [i32; 2]) {
        let a = [mvd[0].unsigned_abs().min(255) as u8, mvd[1].unsigned_abs().min(255) as u8];
        let m = &mut self.mbs[self.mb_addr];
        for y in by..by + h {
            for x in bx..bx + w {
                m.mvd[list][y * 4 + x] = a;
            }
        }
    }

    /// Decode a non-skipped inter macroblock whose raw `mb_type` (in the slice's numbering) is `raw`.
    pub(crate) fn decode_inter_mb<E: Entropy>(&mut self, ent: &mut E, raw: u32) -> Result<()> {
        let is_b = self.slice_type == SliceType::B;
        let addr = self.mb_addr;
        let mut no_sub_lt8 = true;
        let mut direct16 = false;
        let mut p8x8ref0 = false;
        let (shape, pm): (u8, [Pm; 2]) = if is_b {
            match raw {
                0 => {
                    direct16 = true;
                    (0, [0, 0])
                }
                22 => (3, [0, 0]),
                _ => b_type(raw),
            }
        } else {
            match raw {
                0 => (0, [1, 0]),
                1 => (1, [1, 1]),
                2 => (2, [1, 1]),
                3 => (3, [1, 0]),
                _ => {
                    p8x8ref0 = true;
                    (3, [1, 0])
                }
            }
        };
        if direct16 {
            self.mbs[addr].flags |= F_DIRECT;
            self.mbs[addr].direct8 = 0xF;
            self.direct_mb(0xF)?;
            no_sub_lt8 = self.sps.direct_8x8_inference;
        } else if shape == 3 {
            let mut st = [(0u8, 0 as Pm); 4];
            let mut dmask = 0u8;
            for i in 0..4 {
                let raw_sub = ent.sub_mb_type(self)?;
                if raw_sub >= if is_b { 13 } else { 4 } {
                    return Err(Error::Invalid("sub_mb_type out of range"));
                }
                st[i] = sub_type(is_b, raw_sub);
                if is_b && raw_sub == 0 {
                    dmask |= 1 << i;
                }
            }
            self.mbs[addr].direct8 = dmask;
            for list in 0..2 {
                for i in 0..4 {
                    if dmask >> i & 1 != 0 || st[i].1 & (1 << list) == 0 {
                        continue;
                    }
                    let num = self.num_ref(list);
                    let r = if num > 1 && !p8x8ref0 {
                        ent.ref_idx(self, list, i & 1, i >> 1, num)? as i32
                    } else {
                        0
                    };
                    if r as usize >= num {
                        return Err(Error::Invalid("ref_idx out of range"));
                    }
                    self.store_ref(list, (i & 1) * 2, (i >> 1) * 2, 2, 2, r);
                }
            }
            if dmask != 0 {
                self.direct_mb(dmask)?;
            }
            for list in 0..2 {
                for i in 0..4 {
                    if dmask >> i & 1 != 0 || st[i].1 & (1 << list) == 0 {
                        continue;
                    }
                    let (qx, qy) = ((i & 1) * 2, (i >> 1) * 2);
                    let r = self.ref_at(list, qx, qy);
                    for &(sx, sy, sw, sh) in sub_parts(st[i].0) {
                        let (bx, by) = (qx + sx, qy + sy);
                        let mvd = [ent.mvd(self, list, bx, by, 0)?, ent.mvd(self, list, bx, by, 1)?];
                        let p = self.pred_mv(list, r, bx, by, sw, sh, 0);
                        self.store_mv(
                            list,
                            bx,
                            by,
                            sw,
                            sh,
                            [p[0].wrapping_add(mvd[0]), p[1].wrapping_add(mvd[1])],
                        );
                        self.store_mvd(list, bx, by, sw, sh, mvd);
                    }
                }
            }
            for i in 0..4 {
                if dmask >> i & 1 != 0 {
                    if !self.sps.direct_8x8_inference {
                        no_sub_lt8 = false;
                    }
                } else if st[i].0 != 0 {
                    no_sub_lt8 = false;
                }
            }
        } else {
            let parts: &[(usize, usize, usize, usize)] = match shape {
                0 => &[(0, 0, 4, 4)],
                1 => &[(0, 0, 4, 2), (0, 2, 4, 2)],
                _ => &[(0, 0, 2, 4), (2, 0, 2, 4)],
            };
            let dir = shape; // 1: 16x8, 2: 8x16
            let nparts = parts.len();
            for list in 0..2 {
                for (p, &(bx, by, w, h)) in parts.iter().enumerate().take(nparts) {
                    if pm[if nparts == 1 { 0 } else { p }] & (1 << list) == 0 {
                        continue;
                    }
                    let num = self.num_ref(list);
                    let r = if num > 1 { ent.ref_idx(self, list, bx / 2, by / 2, num)? as i32 } else { 0 };
                    if r as usize >= num {
                        return Err(Error::Invalid("ref_idx out of range"));
                    }
                    self.store_ref(list, bx, by, w, h, r);
                }
            }
            for list in 0..2 {
                for (p, &(bx, by, w, h)) in parts.iter().enumerate() {
                    if pm[p] & (1 << list) == 0 {
                        continue;
                    }
                    let r = self.ref_at(list, bx, by);
                    let mvd = [ent.mvd(self, list, bx, by, 0)?, ent.mvd(self, list, bx, by, 1)?];
                    let pr = self.pred_mv(list, r, bx, by, w, h, dir);
                    self.store_mv(
                        list,
                        bx,
                        by,
                        w,
                        h,
                        [pr[0].wrapping_add(mvd[0]), pr[1].wrapping_add(mvd[1])],
                    );
                    self.store_mvd(list, bx, by, w, h, mvd);
                }
            }
        }
        // Residual.
        let cbp = ent.coded_block_pattern(self, false)?;
        self.mbs[addr].cbp = cbp;
        let mut t8 = false;
        if cbp & 15 != 0 && self.pps.transform_8x8_mode && no_sub_lt8 {
            t8 = ent.transform_8x8_flag(self)?;
            if t8 {
                self.mbs[addr].flags |= F_T8X8;
            }
        }
        if cbp != 0 {
            self.read_qp_delta(ent)?;
            self.read_residual(ent, false, cbp, t8, false)?;
        } else {
            self.prev_dqp_nonzero = false;
        }
        self.finish_qp();
        self.mc_mb()?;
        self.add_inter_residual(t8);
        Ok(())
    }

    /// A skipped macroblock: P_Skip, or B_Skip (direct prediction), without residual.
    pub(crate) fn decode_skip_mb(&mut self) -> Result<()> {
        let addr = self.mb_addr;
        self.mbs[addr].flags = F_SKIP;
        if self.slice_type == SliceType::B {
            self.mbs[addr].flags |= F_DIRECT;
            self.mbs[addr].direct8 = 0xF;
            self.direct_mb(0xF)?;
        } else {
            let mv = self.pskip_mv();
            self.store_ref(0, 0, 0, 4, 4, 0);
            self.store_mv(0, 0, 0, 4, 4, mv);
        }
        self.prev_dqp_nonzero = false;
        self.finish_qp();
        self.mc_mb()
    }

    // ---------------------------------------------------------------------------------------------------
    // Motion compensation

    /// True if all 4x4 blocks of the rectangle share the same motion.
    fn uniform(&self, bx: usize, by: usize, w: usize, h: usize) -> bool {
        let w4 = self.mbw * 4;
        let key = |x: usize, y: usize| {
            let (gx, gy) = (self.mb_x * 4 + x, self.mb_y * 4 + y);
            let i8 = (gy >> 1) * (self.mbw * 2) + (gx >> 1);
            (
                self.cur.ref_idx[0][i8],
                self.cur.ref_idx[1][i8],
                self.cur.mv[0][gy * w4 + gx],
                self.cur.mv[1][gy * w4 + gx],
            )
        };
        let k0 = key(bx, by);
        (by..by + h).all(|y| (bx..bx + w).all(|x| key(x, y) == k0))
    }

    /// Motion-compensate the whole macroblock into the current picture.
    pub(crate) fn mc_mb(&mut self) -> Result<()> {
        if self.uniform(0, 0, 4, 4) {
            self.mc_block(0, 0, 4, 4);
            return Ok(());
        }
        for q in 0..4 {
            let (qx, qy) = ((q & 1) * 2, (q >> 1) * 2);
            if self.uniform(qx, qy, 2, 2) {
                self.mc_block(qx, qy, 2, 2);
            } else {
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    self.mc_block(qx + dx, qy + dy, 1, 1);
                }
            }
        }
        Ok(())
    }

    /// Weighted-prediction parameters for component `comp` (0 luma, 1 Cb, 2 Cr) and the references in use.
    fn weights(&self, comp: usize, r: [i32; 2]) -> Option<Weights> {
        match self.weight_mode {
            1 => {
                let t = self.hdr.pred_weight_table.as_ref()?;
                let mut wt = Weights {
                    log_wd: if comp == 0 { t.luma_log2_denom } else { t.chroma_log2_denom },
                    w: [0; 2],
                    o: [0; 2],
                };
                for l in 0..2 {
                    if r[l] >= 0 {
                        let e = t.entries[l].get(r[l] as usize)?;
                        if comp == 0 {
                            wt.w[l] = e.luma_weight;
                            wt.o[l] = e.luma_offset;
                        } else {
                            wt.w[l] = e.chroma_weight[comp - 1];
                            wt.o[l] = e.chroma_offset[comp - 1];
                        }
                    }
                }
                Some(wt)
            }
            2 if r[0] >= 0 && r[1] >= 0 => {
                let w1 = self.implicit_w1(r[0] as usize, r[1] as usize);
                Some(Weights { log_wd: 5, w: [64 - w1, w1], o: [0, 0] })
            }
            _ => None,
        }
    }

    /// Implicit bi-prediction weight `w1` for reference indices `(i0, i1)` (8.4.2.3.1).
    fn implicit_w1(&self, i0: usize, i1: usize) -> i32 {
        let (a, b): (&RefInfo, &RefInfo) = (&self.refs[0][i0], &self.refs[1][i1]);
        if a.long || b.long {
            return 32;
        }
        let tb = (self.cur_poc - a.poc).clamp(-128, 127);
        let td = (b.poc - a.poc).clamp(-128, 127);
        if td == 0 {
            return 32;
        }
        let tx = (16384 + (td / 2).abs()) / td;
        let dsf = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
        let w1 = dsf >> 2;
        if !(-64..=128).contains(&w1) { 32 } else { w1 }
    }

    /// Motion-compensate one block of `w` x `h` 4x4 units at `(bx, by)` of the current macroblock.
    fn mc_block(&mut self, bx: usize, by: usize, w4: usize, h4: usize) {
        let w4g = self.mbw * 4;
        let (gx, gy) = (self.mb_x * 4 + bx, self.mb_y * 4 + by);
        let i8 = (gy >> 1) * (self.mbw * 2) + (gx >> 1);
        let r = [self.cur.ref_idx[0][i8] as i32, self.cur.ref_idx[1][i8] as i32];
        let mvs = [self.cur.mv[0][gy * w4g + gx], self.cur.mv[1][gy * w4g + gx]];
        let (x, y) = ((gx * 4) as i32, (gy * 4) as i32);
        let (w, h) = (w4 * 4, h4 * 4);
        let dpb = self.dpb;
        let (pw, ph) = ((self.mbw * 16) as i32, (self.mbh * 16) as i32);
        let mut pred = [[0u8; PSTRIDE * 16]; 2];
        // Luma.
        for l in 0..2 {
            if r[l] < 0 {
                continue;
            }
            let rp = &dpb[self.refs[l][r[l] as usize].dpb_idx];
            let mv = mvs[l];
            inter::mc_luma(
                &rp.planes[0],
                rp.strides[0],
                pw,
                ph,
                x + (mv[0] as i32 >> 2),
                y + (mv[1] as i32 >> 2),
                (mv[0] & 3) as usize,
                (mv[1] & 3) as usize,
                w,
                h,
                &mut pred[l],
            );
        }
        let wt = self.weights(0, r);
        let stride = self.cur.strides[0];
        let dst = &mut self.cur.planes[0][y as usize * stride + x as usize..];
        let (p0, p1) = (
            if r[0] >= 0 { Some(&pred[0][..]) } else { None },
            if r[1] >= 0 { Some(&pred[1][..]) } else { None },
        );
        inter::combine(dst, stride, w, h, p0, p1, wt.as_ref());
        // Chroma.
        let (cw, ch) = (w / 2, h / 2);
        let (cx, cy) = (x / 2, y / 2);
        for comp in 1..3 {
            for l in 0..2 {
                if r[l] < 0 {
                    continue;
                }
                let rp = &dpb[self.refs[l][r[l] as usize].dpb_idx];
                let mv = mvs[l];
                inter::mc_chroma(
                    &rp.planes[comp],
                    rp.strides[comp],
                    pw / 2,
                    ph / 2,
                    cx + (mv[0] as i32 >> 3),
                    cy + (mv[1] as i32 >> 3),
                    (mv[0] & 7) as i32,
                    (mv[1] & 7) as i32,
                    cw,
                    ch,
                    &mut pred[l],
                );
            }
            let wt = self.weights(comp, r);
            let stride = self.cur.strides[comp];
            let dst = &mut self.cur.planes[comp][cy as usize * stride + cx as usize..];
            let (p0, p1) = (
                if r[0] >= 0 { Some(&pred[0][..]) } else { None },
                if r[1] >= 0 { Some(&pred[1][..]) } else { None },
            );
            inter::combine(dst, stride, cw, ch, p0, p1, wt.as_ref());
        }
    }

    /// Add the residual of an inter macroblock to its prediction.
    pub(crate) fn add_inter_residual(&mut self, t8: bool) {
        let (mx, my) = (self.mb_x * 16, self.mb_y * 16);
        if t8 {
            for i8 in 0..4 {
                self.add_luma_block8(i8, mx + (i8 & 1) * 8, my + (i8 >> 1) * 8);
            }
        } else {
            for r in 0..16 {
                self.add_luma_block(r, mx + (r & 3) * 4, my + (r >> 2) * 4);
            }
        }
        self.add_chroma_blocks();
    }
}
