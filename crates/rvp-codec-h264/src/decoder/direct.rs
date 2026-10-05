//! Direct prediction for B macroblocks (8.4.1.2): spatial and temporal, for progressive frames.
use super::picture::Motion;
use super::slice::SliceDecoder;
use crate::error::{Error, Result};

/// Motion of the co-located block: `(reference index, id of the referenced frame, motion vector)`; the index is
/// `-1` for intra blocks.
#[derive(Clone, Copy)]
struct Col {
    ref_idx: i32,
    ref_id: i32,
    mv: [i32; 2],
}

fn min_positive(x: i32, y: i32) -> i32 {
    if x >= 0 && y >= 0 { x.min(y) } else { x.max(y) }
}

/// The co-located block (8.4.1.2.1 for frame pictures) at 4x4 grid position `(gx, gy)` of `col`.
fn col_motion(col: &Motion, gx: usize, gy: usize) -> Col {
    let i8 = (gy >> 1) * (col.mbw * 2) + (gx >> 1);
    let i4 = gy * (col.mbw * 4) + gx;
    for l in 0..2 {
        if col.ref_idx[l][i8] >= 0 {
            let mv = col.mv[l][i4];
            return Col {
                ref_idx: col.ref_idx[l][i8] as i32,
                ref_id: col.ref_id[l][i8],
                mv: [mv[0] as i32, mv[1] as i32],
            };
        }
    }
    Col { ref_idx: -1, ref_id: -1, mv: [0, 0] }
}

impl SliceDecoder<'_> {
    /// Derive motion for the direct 8x8 blocks in `mask` (bit per 8x8 block) of the current macroblock.
    pub(crate) fn direct_mb(&mut self, mask: u8) -> Result<()> {
        if self.refs[0].is_empty() || self.refs[1].is_empty() {
            return Err(Error::Invalid("direct prediction without reference lists"));
        }
        if self.col.is_none() {
            // The first use waits for the parsing of the co-located picture (it may be running on another thread).
            self.col = Some(self.col_slot.as_ref().ok_or(Error::Invalid("no co-located picture"))?.wait());
        }
        let col = self.col.clone().ok_or(Error::Invalid("no co-located picture"))?;
        if col.mbw != self.mbw || col.mbh != self.mbh {
            return Err(Error::Invalid("co-located picture has a different size"));
        }
        if self.hdr.direct_spatial_mv_pred {
            self.direct_spatial(mask, &col);
        } else {
            self.direct_temporal(mask, &col);
        }
        Ok(())
    }

    /// The 4x4 blocks `(bx, by)` of quadrant `q` that are derived separately, and the co-located block for each.
    fn direct_blocks(
        &self,
        q: usize,
    ) -> impl Iterator<Item = (usize, usize, usize, usize, usize, usize)> + use<> {
        // (bx, by, w, h, col_x, col_y) in 4x4 units: whole quadrant with the corner block when inferring.
        let (qx, qy) = ((q & 1) * 2, (q >> 1) * 2);
        let inference = self.sps.direct_8x8_inference;
        (0..if inference { 1 } else { 4 }).map(move |k| {
            if inference {
                (qx, qy, 2, 2, (q & 1) * 3, (q >> 1) * 3)
            } else {
                let (sx, sy) = (qx + (k & 1), qy + (k >> 1));
                (sx, sy, 1, 1, sx, sy)
            }
        })
    }

    fn direct_spatial(&mut self, mask: u8, col: &Motion) {
        let mut ref_idx = [-1i32; 2];
        for (list, r) in ref_idx.iter_mut().enumerate() {
            let a = self.nb_motion(list, -1, 0, 0);
            let b = self.nb_motion(list, 0, -1, 0);
            let mut c = self.nb_motion(list, 4, -1, 0);
            if !c.avail {
                c = self.nb_motion(list, -1, -1, 0);
            }
            *r = min_positive(a.ref_idx, min_positive(b.ref_idx, c.ref_idx));
        }
        let zero_pred = ref_idx[0] < 0 && ref_idx[1] < 0;
        if zero_pred {
            ref_idx = [0, 0];
        }
        let mut mvp = [[0i32; 2]; 2];
        for list in 0..2 {
            if ref_idx[list] >= 0 && !zero_pred {
                mvp[list] = self.pred_mv(list, ref_idx[list], 0, 0, 4, 4, 0);
            }
        }
        let short_col = !self.refs[1][0].long;
        for q in 0..4 {
            if mask >> q & 1 == 0 {
                continue;
            }
            let (qx, qy) = ((q & 1) * 2, (q >> 1) * 2);
            for list in 0..2 {
                self.store_ref(list, qx, qy, 2, 2, ref_idx[list]);
            }
            for (bx, by, w, h, cx, cy) in self.direct_blocks(q) {
                let c = col_motion(col, self.mb_x * 4 + cx, self.mb_y * 4 + cy);
                let col_zero =
                    short_col && c.ref_idx == 0 && (-1..=1).contains(&c.mv[0]) && (-1..=1).contains(&c.mv[1]);
                for list in 0..2 {
                    let mv = if ref_idx[list] < 0 || zero_pred || (ref_idx[list] == 0 && col_zero) {
                        [0, 0]
                    } else {
                        mvp[list]
                    };
                    self.store_mv(list, bx, by, w, h, mv);
                }
            }
        }
    }

    fn direct_temporal(&mut self, mask: u8, col: &Motion) {
        let poc1 = self.refs[1][0].poc;
        for q in 0..4 {
            if mask >> q & 1 == 0 {
                continue;
            }
            let (qx, qy) = ((q & 1) * 2, (q >> 1) * 2);
            // One reference per 8x8 block: the co-located 8x8 block has a single reference.
            let c0 = col_motion(col, self.mb_x * 4 + (q & 1) * 3, self.mb_y * 4 + (q >> 1) * 3);
            let ref_l0 = if c0.ref_idx < 0 {
                0
            } else {
                self.refs[0].iter().position(|r| r.uid == c0.ref_id).unwrap_or(0)
            };
            self.store_ref(0, qx, qy, 2, 2, ref_l0 as i32);
            self.store_ref(1, qx, qy, 2, 2, 0);
            let r0 = self.refs[0][ref_l0];
            let tb = (self.cur_poc - r0.poc).clamp(-128, 127);
            let td = (poc1 - r0.poc).clamp(-128, 127);
            for (bx, by, w, h, cx, cy) in self.direct_blocks(q) {
                let c = col_motion(col, self.mb_x * 4 + cx, self.mb_y * 4 + cy);
                let (mv0, mv1) = if r0.long || td == 0 {
                    (c.mv, [0, 0])
                } else {
                    let tx = (16384 + (td / 2).abs()) / td;
                    let dsf = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
                    let m0 = [(dsf * c.mv[0] + 128) >> 8, (dsf * c.mv[1] + 128) >> 8];
                    (m0, [m0[0] - c.mv[0], m0[1] - c.mv[1]])
                };
                self.store_mv(0, bx, by, w, h, mv0);
                self.store_mv(1, bx, by, w, h, mv1);
            }
        }
    }
}
