//! Inter prediction samples (8.5.3.3): fractional interpolation from a reference picture and the weighting of one or two predictions.
use super::frame::Plane;
use crate::tables::{CHROMA_FILTER, LUMA_FILTER};

/// Interpolate the `w x h` block whose top-left integer position in `r` is (`x`, `y`) with fractions (`fx`, `fy`) (quarter samples for
/// luma, eighth for chroma) into 14-bit intermediate samples in `out` (row-major, stride `w`).
#[allow(clippy::too_many_arguments)]
pub fn interpolate(
    r: &Plane,
    x: i32,
    y: i32,
    fx: usize,
    fy: usize,
    w: usize,
    h: usize,
    luma: bool,
    bit_depth: u32,
    out: &mut [i32],
) {
    let (taps, before) = if luma { (8usize, 3i32) } else { (4usize, 1i32) };
    let shift1 = (bit_depth as i32 - 8).min(4);
    let shift3 = 14 - bit_depth as i32;
    let (pw, ph) = (r.width as i32, r.height as i32);
    let at = |xx: i32, yy: i32| -> i32 {
        r.data[(yy.clamp(0, ph - 1) as usize) * r.stride + xx.clamp(0, pw - 1) as usize] as i32
    };
    let coef =
        |frac: usize, i: usize| -> i32 { if luma { LUMA_FILTER[frac][i] } else { CHROMA_FILTER[frac][i] } };
    if fx == 0 && fy == 0 {
        for j in 0..h {
            for i in 0..w {
                out[j * w + i] = at(x + i as i32, y + j as i32) << shift3;
            }
        }
    } else if fy == 0 {
        for j in 0..h {
            for i in 0..w {
                let mut s = 0;
                for k in 0..taps {
                    s += coef(fx, k) * at(x + i as i32 + k as i32 - before, y + j as i32);
                }
                out[j * w + i] = s >> shift1;
            }
        }
    } else if fx == 0 {
        for j in 0..h {
            for i in 0..w {
                let mut s = 0;
                for k in 0..taps {
                    s += coef(fy, k) * at(x + i as i32, y + j as i32 + k as i32 - before);
                }
                out[j * w + i] = s >> shift1;
            }
        }
    } else {
        // Horizontal pass over the rows the vertical filter needs, then the vertical pass at 6 bits.
        let rows = h + taps - 1;
        let mut tmp = alloc::vec![0i32; rows * w];
        for j in 0..rows {
            for i in 0..w {
                let mut s = 0;
                for k in 0..taps {
                    s += coef(fx, k) * at(x + i as i32 + k as i32 - before, y + j as i32 - before);
                }
                tmp[j * w + i] = s >> shift1;
            }
        }
        for j in 0..h {
            for i in 0..w {
                let mut s = 0;
                for k in 0..taps {
                    s += coef(fy, k) * tmp[(j + k) * w + i];
                }
                out[j * w + i] = s >> 6;
            }
        }
    }
}

/// Explicit weights of one list entry for one component.
#[derive(Clone, Copy)]
pub struct Weight {
    pub w: i32,
    pub o: i32,
}

/// Write a block predicted from one list to the picture (8.5.3.3.4.2 / .3).
pub fn put_uni(
    dst: &mut Plane,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    p: &[i32],
    bit_depth: u32,
    explicit: Option<(u32, Weight)>,
) {
    let max = (1i32 << bit_depth) - 1;
    let shift1 = 14 - bit_depth;
    for j in 0..h {
        for i in 0..w {
            let v = p[j * w + i];
            let s = match explicit {
                None => (v + ((1 << shift1) >> 1)) >> shift1,
                Some((denom, wt)) => {
                    let log2wd = denom as i32 + shift1 as i32;
                    if log2wd >= 1 {
                        ((v * wt.w + (1 << (log2wd - 1))) >> log2wd) + wt.o
                    } else {
                        v * wt.w + wt.o
                    }
                }
            };
            dst.data[(y + j) * dst.stride + x + i] = s.clamp(0, max) as u16;
        }
    }
}

/// Write a block predicted from both lists.
#[allow(clippy::too_many_arguments)]
pub fn put_bi(
    dst: &mut Plane,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    p0: &[i32],
    p1: &[i32],
    bit_depth: u32,
    explicit: Option<(u32, Weight, Weight)>,
) {
    let max = (1i32 << bit_depth) - 1;
    let shift2 = 15 - bit_depth;
    for j in 0..h {
        for i in 0..w {
            let (a, b) = (p0[j * w + i], p1[j * w + i]);
            let s = match explicit {
                None => (a + b + (1 << (shift2 - 1))) >> shift2,
                Some((denom, w0, w1)) => {
                    let log2wd = denom as i32 + 14 - bit_depth as i32;
                    (a * w0.w + b * w1.w + ((w0.o + w1.o + 1) << log2wd)) >> (log2wd + 1)
                }
            };
            dst.data[(y + j) * dst.stride + x + i] = s.clamp(0, max) as u16;
        }
    }
}
