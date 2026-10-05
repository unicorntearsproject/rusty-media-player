//! CABAC syntax element decoding: binarizations and context selection (9.3.2, 9.3.3.1).
use super::entropy::{Cat, Entropy};
use super::mbinfo::*;
use super::slice::SliceDecoder;
use crate::cabac::{CabacDecoder, Contexts, LAST_COEFF_8X8, SIG_COEFF_8X8_FRAME, ctx};
use crate::error::{Error, Result};
use crate::params::SliceType;

/// CABAC entropy decoder for one slice.
pub struct Cabac<'a> {
    /// The arithmetic decoding engine.
    pub dec: CabacDecoder<'a>,
    /// Context states.
    pub ctxs: Contexts,
}

const BAD: Error = Error::Invalid("CABAC value out of range");

impl<'a> Cabac<'a> {
    /// Start CABAC decoding of `data` at byte `start` for a slice with the given QP and init idc.
    pub fn new(data: &'a [u8], start: usize, slice_qp: i32, init_idc: Option<u32>) -> Self {
        Self { dec: CabacDecoder::new(data, start), ctxs: Contexts::new(slice_qp, init_idc) }
    }

    #[inline]
    fn bin(&mut self, ctx_idx: usize) -> u32 {
        self.dec.decision(&mut self.ctxs, ctx_idx)
    }

    /// `mb_skip_flag`.
    pub fn skip_flag(&mut self, d: &SliceDecoder<'_>) -> bool {
        let cond = |n: Option<usize>| n.is_some_and(|a| d.mbs[a].flags & F_SKIP == 0) as usize;
        let inc = cond(d.na) + cond(d.nb);
        let base = if d.slice_type == SliceType::B { ctx::MB_SKIP_B } else { ctx::MB_SKIP_P };
        self.bin(base + inc) != 0
    }

    /// `end_of_slice_flag`.
    pub fn end_of_slice(&mut self) -> bool {
        self.dec.terminate() != 0
    }

    /// An intra `mb_type` (Table 9-36). `i_slice` selects the I-slice contexts with neighbour-dependent first bin;
    /// otherwise `base` is the suffix context offset of P/B slices.
    fn mb_type_intra(&mut self, d: &SliceDecoder<'_>, i_slice: bool, base: usize) -> u32 {
        let b0 = if i_slice {
            let cond = |n: Option<usize>| n.is_some_and(|a| d.mbs[a].flags & (F_I4 | F_I8) == 0) as usize;
            self.bin(base + cond(d.na) + cond(d.nb))
        } else {
            self.bin(base)
        };
        if b0 == 0 {
            return 0;
        }
        if self.dec.terminate() != 0 {
            return 25;
        }
        let (c2, c3, c4_chroma, c5_chroma, c6, c4_pred, c5_pred) = if i_slice {
            (base + 3, base + 4, base + 5, base + 6, base + 7, base + 6, base + 7)
        } else {
            (base + 1, base + 2, base + 2, base + 3, base + 3, base + 3, base + 3)
        };
        let luma = self.bin(c2);
        let b3 = self.bin(c3);
        let (chroma, p1, p0);
        if b3 == 0 {
            chroma = 0;
            p1 = self.bin(c4_pred);
            p0 = self.bin(c5_pred);
        } else {
            chroma = 1 + self.bin(c4_chroma);
            p1 = self.bin(c5_chroma);
            p0 = self.bin(c6);
        }
        1 + (p1 << 1 | p0) + 4 * chroma + 12 * luma
    }

    fn unary_ctx(&mut self, first: usize, rest: usize, max: u32) -> u32 {
        let mut v = 0;
        if self.bin(first) == 0 {
            return 0;
        }
        v += 1;
        while v < max && self.bin(rest) != 0 {
            v += 1;
        }
        v
    }

    /// Exp-Golomb suffix of order `k` (bypass bins).
    fn exp_golomb_bypass(&mut self, mut k: u32) -> Result<u32> {
        let mut v = 0u32;
        while self.dec.bypass() != 0 {
            v = v.wrapping_add(1 << k);
            k += 1;
            if k > 24 {
                return Err(BAD);
            }
        }
        while k > 0 {
            k -= 1;
            v = v.wrapping_add(self.dec.bypass() << k);
        }
        Ok(v)
    }

    /// `coded_block_flag` context increment for the neighbours `(a, b)`: `Some(Some(bit))` for an available
    /// neighbour (PCM neighbours must pass `true`), `None` for an unavailable one.
    fn cbf_inc(a: Option<bool>, b: Option<bool>, cur_intra: bool) -> usize {
        let cond = |n: Option<bool>| match n {
            None => cur_intra as usize,
            Some(v) => v as usize,
        };
        cond(a) + 2 * cond(b)
    }
}

impl Entropy for Cabac<'_> {
    const IS_CABAC: bool = true;

    fn mb_type(&mut self, d: &SliceDecoder<'_>) -> Result<u32> {
        Ok(match d.slice_type {
            SliceType::I | SliceType::Si => self.mb_type_intra(d, true, ctx::MB_TYPE_I),
            SliceType::P | SliceType::Sp => {
                if self.bin(ctx::MB_TYPE_P) == 0 {
                    let b1 = self.bin(ctx::MB_TYPE_P + 1);
                    if b1 == 0 {
                        if self.bin(ctx::MB_TYPE_P + 2) == 0 { 0 } else { 3 }
                    } else if self.bin(ctx::MB_TYPE_P + 3) != 0 {
                        1
                    } else {
                        2
                    }
                } else {
                    5 + self.mb_type_intra(d, false, ctx::MB_TYPE_P_INTRA)
                }
            }
            SliceType::B => {
                let cond = |n: Option<usize>| n.is_some_and(|a| d.mbs[a].flags & F_DIRECT == 0) as usize;
                let b = ctx::MB_TYPE_B;
                if self.bin(b + cond(d.na) + cond(d.nb)) == 0 {
                    0
                } else if self.bin(b + 3) == 0 {
                    1 + self.bin(b + 5)
                } else {
                    let b2 = self.bin(b + 4);
                    let b3 = self.bin(b + 5);
                    let b4 = self.bin(b + 5);
                    let b5 = self.bin(b + 5);
                    let bits = b2 << 3 | b3 << 2 | b4 << 1 | b5;
                    match bits {
                        0..=7 => 3 + bits,
                        13 => 23 + self.mb_type_intra(d, false, ctx::MB_TYPE_B_INTRA),
                        14 => 11,
                        15 => 22,
                        _ => 12 + ((bits - 8) << 1) + self.bin(b + 5),
                    }
                }
            }
        })
    }

    fn sub_mb_type(&mut self, d: &SliceDecoder<'_>) -> Result<u32> {
        if d.slice_type != SliceType::B {
            let b = ctx::SUB_MB_TYPE_P;
            return Ok(if self.bin(b) != 0 {
                0
            } else if self.bin(b + 1) == 0 {
                1
            } else if self.bin(b + 2) != 0 {
                2
            } else {
                3
            });
        }
        let b = ctx::SUB_MB_TYPE_B;
        if self.bin(b) == 0 {
            return Ok(0);
        }
        if self.bin(b + 1) == 0 {
            return Ok(1 + self.bin(b + 3));
        }
        if self.bin(b + 2) == 0 {
            let x = self.bin(b + 3);
            let y = self.bin(b + 3);
            return Ok(3 + (x << 1 | y));
        }
        if self.bin(b + 3) != 0 {
            return Ok(11 + self.bin(b + 3));
        }
        let x = self.bin(b + 3);
        let y = self.bin(b + 3);
        Ok(7 + (x << 1 | y))
    }

    fn transform_8x8_flag(&mut self, d: &SliceDecoder<'_>) -> Result<bool> {
        let cond = |n: Option<usize>| n.is_some_and(|a| d.mbs[a].flags & F_T8X8 != 0) as usize;
        Ok(self.bin(ctx::TRANSFORM_8X8 + cond(d.na) + cond(d.nb)) != 0)
    }

    fn intra_pred_mode(&mut self) -> Result<(bool, u8)> {
        if self.bin(ctx::PREV_INTRA_PRED_MODE_FLAG) != 0 {
            return Ok((true, 0));
        }
        let b0 = self.bin(ctx::REM_INTRA_PRED_MODE);
        let b1 = self.bin(ctx::REM_INTRA_PRED_MODE);
        let b2 = self.bin(ctx::REM_INTRA_PRED_MODE);
        Ok((false, (b0 | b1 << 1 | b2 << 2) as u8))
    }

    fn intra_chroma_pred_mode(&mut self, d: &SliceDecoder<'_>) -> Result<u8> {
        let cond = |n: Option<usize>| n.is_some_and(|a| d.mbs[a].chroma_mode != 0) as usize;
        let b = ctx::INTRA_CHROMA_PRED_MODE;
        Ok(self.unary_ctx(b + cond(d.na) + cond(d.nb), b + 3, 3) as u8)
    }

    fn ref_idx(
        &mut self,
        d: &SliceDecoder<'_>,
        list: usize,
        x8: usize,
        y8: usize,
        _num_ref: usize,
    ) -> Result<u32> {
        // Neighbouring 8x8 blocks A (left) and B (above) of the partition's top-left 8x8 block.
        let cond = |dx: i32, dy: i32| -> usize {
            let (nx, ny) = (x8 as i32 + dx, y8 as i32 + dy);
            let (mb, bx, by) = if nx < 0 {
                match d.na {
                    Some(a) => (a, 1usize, ny as usize),
                    None => return 0,
                }
            } else if ny < 0 {
                match d.nb {
                    Some(a) => (a, nx as usize, 1usize),
                    None => return 0,
                }
            } else {
                (d.mb_addr, nx as usize, ny as usize)
            };
            let m = &d.mbs[mb];
            if m.flags & (F_SKIP | F_INTRA) != 0 || m.direct8 >> (by * 2 + bx) & 1 != 0 {
                return 0;
            }
            let (mx, my) = (mb % d.mbw, mb / d.mbw);
            let r = d.cur.ref_idx[list][(my * 2 + by) * (d.mbw * 2) + mx * 2 + bx];
            (r > 0) as usize
        };
        let inc = cond(-1, 0) + 2 * cond(0, -1);
        let b = ctx::REF_IDX;
        let mut v = 0;
        if self.bin(b + inc) == 0 {
            return Ok(0);
        }
        v += 1;
        if self.bin(b + 4) == 0 {
            return Ok(v);
        }
        v += 1;
        while self.bin(b + 5) != 0 {
            v += 1;
            if v > 32 {
                return Err(BAD);
            }
        }
        Ok(v)
    }

    fn mvd(&mut self, d: &SliceDecoder<'_>, list: usize, x4: usize, y4: usize, comp: usize) -> Result<i32> {
        let abs_at = |dx: i32, dy: i32| -> u32 {
            let (nx, ny) = (x4 as i32 + dx, y4 as i32 + dy);
            let (mb, bx, by) = if nx < 0 {
                match d.na {
                    Some(a) => (a, 3usize, ny as usize),
                    None => return 0,
                }
            } else if ny < 0 {
                match d.nb {
                    Some(a) => (a, nx as usize, 3usize),
                    None => return 0,
                }
            } else {
                (d.mb_addr, nx as usize, ny as usize)
            };
            d.mbs[mb].mvd[list][by * 4 + bx][comp] as u32
        };
        let sum = abs_at(-1, 0) + abs_at(0, -1);
        let inc = if sum < 3 {
            0
        } else if sum > 32 {
            2
        } else {
            1
        };
        let base = if comp == 0 { ctx::MVD_X } else { ctx::MVD_Y };
        let mut v = 0u32;
        while v < 9 {
            let c = base
                + match v {
                    0 => inc,
                    1 => 3,
                    2 => 4,
                    3 => 5,
                    _ => 6,
                };
            if self.bin(c) == 0 {
                break;
            }
            v += 1;
        }
        if v >= 9 {
            v = v.wrapping_add(self.exp_golomb_bypass(3)?);
        }
        if v != 0 && self.dec.bypass() != 0 {
            return Ok(-(v as i32));
        }
        Ok(v as i32)
    }

    fn coded_block_pattern(&mut self, d: &SliceDecoder<'_>, _intra: bool) -> Result<u8> {
        let mut luma = 0u8;
        for b8 in 0..4usize {
            let (bx, by) = (b8 & 1, b8 >> 1);
            let a = if bx > 0 {
                (luma >> (b8 - 1) & 1 == 0) as usize
            } else {
                d.na.is_some_and(|n| d.mbs[n].cbp >> (by * 2 + 1) & 1 == 0) as usize
            };
            let b = if by > 0 {
                (luma >> (b8 - 2) & 1 == 0) as usize
            } else {
                d.nb.is_some_and(|n| d.mbs[n].cbp >> (2 + bx) & 1 == 0) as usize
            };
            if self.bin(ctx::CBP_LUMA + a + 2 * b) != 0 {
                luma |= 1 << b8;
            }
        }
        let chroma_cond = |n: Option<usize>, bin1: bool| -> usize {
            n.is_some_and(|a| {
                let c = d.mbs[a].cbp >> 4;
                if bin1 { c == 2 } else { c != 0 }
            }) as usize
        };
        let inc0 = chroma_cond(d.na, false) + 2 * chroma_cond(d.nb, false);
        let mut chroma = 0u8;
        if self.bin(ctx::CBP_CHROMA + inc0) != 0 {
            let inc1 = chroma_cond(d.na, true) + 2 * chroma_cond(d.nb, true) + 4;
            chroma = 1 + self.bin(ctx::CBP_CHROMA + inc1) as u8;
        }
        Ok(luma | chroma << 4)
    }

    fn mb_qp_delta(&mut self, d: &SliceDecoder<'_>) -> Result<i32> {
        let b = ctx::MB_QP_DELTA;
        let mut k = 0i32;
        if self.bin(b + d.prev_dqp_nonzero as usize) == 0 {
            return Ok(0);
        }
        k += 1;
        if self.bin(b + 2) != 0 {
            k += 1;
            while self.bin(b + 3) != 0 {
                k += 1;
                if k > 104 {
                    return Err(BAD);
                }
            }
        }
        Ok(if k & 1 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }

    fn residual_block(
        &mut self,
        d: &SliceDecoder<'_>,
        cat: Cat,
        idx: usize,
        max: usize,
        out: &mut [i32],
    ) -> Result<usize> {
        for v in out[..max].iter_mut() {
            *v = 0;
        }
        let cur_intra = d.mbs[d.mb_addr].flags & F_INTRA != 0;
        let cat_n = match cat {
            Cat::LumaDc16 => 0,
            Cat::LumaAc16 => 1,
            Cat::Luma4x4 => 2,
            Cat::ChromaDc => 3,
            Cat::ChromaAc => 4,
            Cat::Luma8x8 => 5,
        };
        // coded_block_flag (not sent for 8x8 blocks in 4:2:0: inferred 1).
        if cat != Cat::Luma8x8 {
            let pcm = |n: usize| d.mbs[n].flags & F_PCM != 0;
            let (a, b): (Option<bool>, Option<bool>) = match cat {
                Cat::LumaDc16 => {
                    let f = |n: Option<usize>| n.map(|m| pcm(m) || d.mbs[m].cbf_dc & 1 != 0);
                    (f(d.na), f(d.nb))
                }
                Cat::ChromaDc => {
                    let f = |n: Option<usize>| n.map(|m| pcm(m) || d.mbs[m].cbf_dc >> (1 + idx) & 1 != 0);
                    (f(d.na), f(d.nb))
                }
                Cat::ChromaAc => {
                    let (comp, blk) = (idx / 4, idx % 4);
                    (
                        d.nz_left(1 + comp, blk & 1, blk >> 1).map(|v| v != 0),
                        d.nz_above(1 + comp, blk & 1, blk >> 1).map(|v| v != 0),
                    )
                }
                _ => (
                    d.nz_left(0, idx & 3, idx >> 2).map(|v| v != 0),
                    d.nz_above(0, idx & 3, idx >> 2).map(|v| v != 0),
                ),
            };
            let inc = Self::cbf_inc(a, b, cur_intra);
            if self.bin(ctx::CODED_BLOCK_FLAG + ctx::CBF_CAT_OFFSET[cat_n] + inc) == 0 {
                return Ok(0);
            }
        }
        // Significance map.
        let (sig_base, last_base, abs_base) = if cat == Cat::Luma8x8 {
            (ctx::SIG_COEFF_8X8, ctx::LAST_COEFF_8X8, ctx::COEFF_ABS_LEVEL_8X8)
        } else {
            (
                ctx::SIG_COEFF + ctx::SIG_CAT_OFFSET[cat_n],
                ctx::LAST_COEFF + ctx::SIG_CAT_OFFSET[cat_n],
                ctx::COEFF_ABS_LEVEL + ctx::ABS_CAT_OFFSET[cat_n],
            )
        };
        let mut sig_pos = [0u8; 64];
        let mut n_sig = 0usize;
        let mut i = 0usize;
        let mut found_last = false;
        while i < max - 1 {
            let (sinc, linc) = match cat {
                Cat::Luma8x8 => (SIG_COEFF_8X8_FRAME[i] as usize, LAST_COEFF_8X8[i] as usize),
                Cat::ChromaDc => (i.min(2), i.min(2)),
                _ => (i, i),
            };
            if self.bin(sig_base + sinc) != 0 {
                sig_pos[n_sig] = i as u8;
                n_sig += 1;
                if self.bin(last_base + linc) != 0 {
                    found_last = true;
                    break;
                }
            }
            i += 1;
        }
        if !found_last {
            sig_pos[n_sig] = (max - 1) as u8;
            n_sig += 1;
        }
        // Levels, from the last significant coefficient backwards.
        let mut eq1 = 0usize;
        let mut gt1 = 0usize;
        for k in (0..n_sig).rev() {
            let inc0 = if gt1 != 0 { 0 } else { (1 + eq1).min(4) };
            let inc_n = 5 + gt1.min(4 - (cat == Cat::ChromaDc) as usize);
            let mut v = 0u32;
            if self.bin(abs_base + inc0) != 0 {
                v = 1;
                while v < 14 && self.bin(abs_base + inc_n) != 0 {
                    v += 1;
                }
                if v == 14 {
                    v = v.wrapping_add(self.exp_golomb_bypass(0)?);
                }
            }
            let abs = v.wrapping_add(1);
            if abs == 1 {
                eq1 += 1;
            } else {
                gt1 += 1;
            }
            let level = if self.dec.bypass() != 0 { -(abs as i32) } else { abs as i32 };
            out[sig_pos[k] as usize] = level;
        }
        if self.dec.overrun() {
            return Err(Error::Truncated);
        }
        Ok(n_sig)
    }

    fn pcm_samples(&mut self, out: &mut [u8; 384]) -> Result<()> {
        let start = self.dec.byte_aligned_pos();
        let data = self.dec.data();
        if start + 384 > data.len() {
            return Err(Error::Truncated);
        }
        out.copy_from_slice(&data[start..start + 384]);
        self.dec.restart(start + 384);
        Ok(())
    }
}
