//! `residual_coding()` (7.3.8.11): the quantised coefficients of one transform block.
use super::ctu::Dec;
use crate::cabac::Cabac;
use crate::ctx;
use crate::tables::{CTX_IDX_MAP_4X4, SCAN};
use crate::{Error, Result};

/// What parsing a transform block produced.
pub struct Residual {
    /// `transform_skip_flag`.
    pub transform_skip: bool,
    /// Largest column and row holding a non-zero coefficient.
    pub max_x: usize,
    /// See `max_x`.
    pub max_y: usize,
}

/// `coeff_abs_level_remaining` (9.3.3.11).
#[inline]
fn abs_level_remaining(c: &mut Cabac, rice: u32) -> Result<u32> {
    let mut q = 0u32;
    while q < 4 && c.bypass() == 1 {
        q += 1;
    }
    if q < 4 {
        return Ok((q << rice) + c.bypass_bits(rice));
    }
    let mut k = rice + 1;
    let mut v = 0u32;
    while c.bypass() == 1 {
        v = v.checked_add(1u32 << k).ok_or(Error::Invalid("coefficient level overflow"))?;
        k += 1;
        if k > 31 {
            return Err(Error::Invalid("coefficient level too large"));
        }
    }
    v = v.checked_add(c.bypass_bits(k)).ok_or(Error::Invalid("coefficient level overflow"))?;
    Ok((4 << rice) + v)
}

impl Dec<'_> {
    /// Parse the coefficients of a block of `1 << log2` samples into `self.coef` (row-major; the block is cleared first).
    pub(super) fn residual_coding(&mut self, log2: u32, c_idx: usize, scan_idx: usize) -> Result<Residual> {
        let n = 1usize << log2;
        self.coef[..n * n].fill(0);
        let luma = c_idx == 0;
        let transform_skip = self.pps.transform_skip_enabled
            && !self.cu_transquant_bypass
            && log2 <= 2
            && self.cabac.decision(&mut self.ctx, ctx::TRANSFORM_SKIP + (!luma) as usize) == 1;
        // The position of the last significant coefficient.
        let (c_off, c_shift) =
            if luma { (3 * (log2 - 2) + ((log2 - 1) >> 2), (log2 + 1) >> 2) } else { (15, log2 - 2) };
        let max_prefix = (log2 << 1) - 1;
        let mut px = 0;
        while px < max_prefix
            && self
                .cabac
                .decision(&mut self.ctx, ctx::LAST_X_PREFIX + c_off as usize + (px >> c_shift) as usize)
                == 1
        {
            px += 1;
        }
        let mut py = 0;
        while py < max_prefix
            && self
                .cabac
                .decision(&mut self.ctx, ctx::LAST_Y_PREFIX + c_off as usize + (py >> c_shift) as usize)
                == 1
        {
            py += 1;
        }
        let mut last_x = px;
        if px > 3 {
            let bits = (px >> 1) - 1;
            last_x = (1 << bits) * (2 + (px & 1)) + self.cabac.bypass_bits(bits);
        }
        let mut last_y = py;
        if py > 3 {
            let bits = (py >> 1) - 1;
            last_y = (1 << bits) * (2 + (py & 1)) + self.cabac.bypass_bits(bits);
        }
        if scan_idx == 2 {
            core::mem::swap(&mut last_x, &mut last_y);
        }
        let (last_x, last_y) = (last_x as usize, last_y as usize);
        if last_x >= n || last_y >= n {
            return Err(Error::Invalid("last significant coefficient outside the block"));
        }
        let sb_log2 = (log2 - 2) as usize;
        let sb_scan = &SCAN[sb_log2][scan_idx];
        let pos_scan = &SCAN[2][scan_idx];
        // Where the last coefficient sits in scan order.
        let (lsx, lsy) = ((last_x >> 2) as u8, (last_y >> 2) as u8);
        let last_sub = (0..1usize << (2 * sb_log2))
            .find(|i| sb_scan[*i] == (lsx, lsy))
            .ok_or(Error::Invalid("last sub-block"))?;
        let (lpx, lpy) = ((last_x & 3) as u8, (last_y & 3) as u8);
        let last_pos =
            (0..16usize).find(|i| pos_scan[*i] == (lpx, lpy)).ok_or(Error::Invalid("last position"))?;

        let mut csbf = [[false; 10]; 10];
        let sign_hiding = self.pps.sign_data_hiding_enabled && !self.cu_transquant_bypass;
        let mut greater1_state = 1u32; // `greater1Ctx` carried between sub-blocks
        let mut first_sub_done = false;
        let (mut max_x, mut max_y) = (0usize, 0usize);
        let mut rice;
        for i in (0..=last_sub).rev() {
            let (xs, ys) = (sb_scan[i].0 as usize, sb_scan[i].1 as usize);
            let right = csbf[xs + 1][ys];
            let below = csbf[xs][ys + 1];
            let mut infer_dc = false;
            let coded = if i < last_sub && i > 0 {
                let inc = (right || below) as usize + if luma { 0 } else { 2 };
                let f = self.cabac.decision(&mut self.ctx, ctx::CODED_SUB_BLOCK + inc) == 1;
                infer_dc = f;
                f
            } else {
                true
            };
            csbf[xs][ys] = coded;
            if !coded {
                continue;
            }
            // Significance of each position, scanning down from the last.
            let mut sig = [0u8; 16];
            let mut n_sig = 0usize;
            let start = if i == last_sub {
                sig[0] = last_pos as u8;
                n_sig = 1;
                last_pos as i32 - 1
            } else {
                15
            };
            let prev_csbf = right as usize | ((below as usize) << 1);
            for p in (0..=start).rev() {
                let p = p as usize;
                let (xp, yp) = (pos_scan[p].0 as usize, pos_scan[p].1 as usize);
                let decode = p > 0 || !infer_dc;
                let s = if decode {
                    let sig_ctx = if log2 == 2 {
                        CTX_IDX_MAP_4X4[(yp << 2) + xp] as usize
                    } else if xs == 0 && ys == 0 && p == 0 {
                        0
                    } else {
                        let base = match prev_csbf {
                            0 => {
                                if xp + yp == 0 {
                                    2
                                } else if xp + yp < 3 {
                                    1
                                } else {
                                    0
                                }
                            }
                            1 => [2, 1, 0, 0][yp],
                            2 => [2, 1, 0, 0][xp],
                            _ => 2,
                        };
                        if luma {
                            let mut c = base;
                            if xs > 0 || ys > 0 {
                                c += 3;
                            }
                            c + if log2 == 3 { if scan_idx == 0 { 9 } else { 15 } } else { 21 }
                        } else {
                            base + if log2 == 3 { 9 } else { 12 }
                        }
                    };
                    let inc = if luma { sig_ctx } else { 27 + sig_ctx };
                    self.cabac.decision(&mut self.ctx, ctx::SIG_COEFF + inc) == 1
                } else {
                    // The last position of a coded sub-block with no other significant coefficient must be significant.
                    true
                };
                if s {
                    sig[n_sig] = p as u8;
                    n_sig += 1;
                    infer_dc = false;
                }
            }
            if n_sig == 0 {
                continue;
            }
            // Greater-than-1 and -2 flags for the first eight.
            let mut ctx_set = if i == 0 || !luma { 0 } else { 2 };
            if first_sub_done && greater1_state == 0 {
                ctx_set += 1;
            }
            first_sub_done = true;
            let mut g1ctx = 1u32;
            let mut abs = [1u32; 16];
            let mut last_g1_idx: Option<usize> = None;
            let n_g1 = n_sig.min(8);
            for k in 0..n_g1 {
                let inc = ctx_set * 4 + g1ctx.min(3) as usize + if luma { 0 } else { 16 };
                let f = self.cabac.decision(&mut self.ctx, ctx::GREATER1 + inc);
                if f == 1 {
                    abs[k] = 2;
                    g1ctx = 0;
                    if last_g1_idx.is_none() {
                        last_g1_idx = Some(k);
                    }
                } else if g1ctx > 0 {
                    g1ctx += 1;
                }
            }
            greater1_state = g1ctx;
            if let Some(k) = last_g1_idx {
                let inc = ctx_set + if luma { 0 } else { 4 };
                if self.cabac.decision(&mut self.ctx, ctx::GREATER2 + inc) == 1 {
                    abs[k] = 3;
                }
            }
            // Signs: one bypass bin per coefficient except a hidden one, the first coefficient first.
            let first_pos = sig[n_sig - 1] as i32;
            let hidden = sign_hiding && (sig[0] as i32 - first_pos) > 3;
            let n_signs = if hidden { n_sig - 1 } else { n_sig };
            let sign_word = self.cabac.bypass_bits(n_signs as u32);
            // The remaining magnitudes, with the Rice parameter adapting as it goes.
            rice = 0;
            let mut sum_abs = 0u32;
            for k in 0..n_sig {
                let base = abs[k];
                let thresh = if k < 8 { if Some(k) == last_g1_idx { 3 } else { 2 } } else { 1 };
                let mut level = base;
                if base == thresh {
                    level = base
                        .checked_add(abs_level_remaining(&mut self.cabac, rice)?)
                        .ok_or(Error::Invalid("coefficient level"))?;
                    if level > 3 * (1 << rice) {
                        rice = (rice + 1).min(4);
                    }
                }
                abs[k] = level;
                sum_abs = sum_abs.wrapping_add(level);
            }
            for k in 0..n_sig {
                let mut v = abs[k] as i32;
                let negative = if k < n_signs {
                    (sign_word >> (n_signs - 1 - k)) & 1 == 1
                } else {
                    // The hidden sign: odd sum of the magnitudes means negative.
                    sum_abs & 1 == 1
                };
                if negative {
                    v = -v;
                }
                let p = sig[k] as usize;
                let (xc, yc) = (xs * 4 + pos_scan[p].0 as usize, ys * 4 + pos_scan[p].1 as usize);
                self.coef[yc * n + xc] = v;
                max_x = max_x.max(xc);
                max_y = max_y.max(yc);
            }
        }
        if self.cabac.overrun() {
            return Err(Error::Truncated);
        }
        Ok(Residual { transform_skip, max_x, max_y })
    }
}
