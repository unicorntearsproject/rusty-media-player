//! Inter prediction sample interpolation (8.4.2.2): quarter-sample luma, eighth-sample chroma, and the
//! weighted sample prediction combinations (8.4.2.3).

/// Maximum block size handled in one call.
pub const MAX_BLK: usize = 16;
/// Stride of the temporary prediction blocks.
pub const PSTRIDE: usize = 16;

#[inline(always)]
fn tap6(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) -> i32 {
    a - 5 * b + 20 * c + 20 * d - 5 * e + f
}

#[inline(always)]
fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// A window of reference samples with at least 2 samples of margin before and 3 after the block in each
/// direction, either a view into the picture or a clamped copy.
struct Window<'a> {
    data: &'a [u8],
    stride: usize,
    /// Index of the sample at the block's integer position.
    origin: usize,
}

/// Copy the `(w + 5) x (h + 5)` window around `(x, y)` with coordinates clamped to the plane.
fn fetch_clamped(
    plane: &[u8],
    stride: usize,
    pw: i32,
    ph: i32,
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    tmp: &mut [u8; 21 * 21],
) {
    for j in 0..h + 5 {
        let sy = (y - 2 + j as i32).clamp(0, ph - 1) as usize;
        let row = &plane[sy * stride..sy * stride + pw as usize];
        for i in 0..w + 5 {
            let sx = (x - 2 + i as i32).clamp(0, pw - 1) as usize;
            tmp[j * 21 + i] = row[sx];
        }
    }
}

/// Luma motion compensation for a `w` x `h` block (each 4, 8 or 16) whose top-left sample is at integer position
/// `(x, y)` in the reference `plane` plus the quarter-sample fraction `(fx, fy)`. The result goes to `dst` with
/// stride [`PSTRIDE`].
#[allow(clippy::too_many_arguments)]
pub fn mc_luma(
    plane: &[u8],
    stride: usize,
    pw: i32,
    ph: i32,
    x: i32,
    y: i32,
    fx: usize,
    fy: usize,
    w: usize,
    h: usize,
    dst: &mut [u8],
) {
    let mut tmp = [0u8; 21 * 21];
    let inside = x >= 2 && y >= 2 && x + w as i32 + 3 <= pw && y + h as i32 + 3 <= ph;
    let win = if inside {
        Window { data: plane, stride, origin: y as usize * stride + x as usize }
    } else {
        fetch_clamped(plane, stride, pw, ph, x, y, w, h, &mut tmp);
        Window { data: &tmp, stride: 21, origin: 2 * 21 + 2 }
    };
    let s = |dx: i32, dy: i32| -> i32 {
        win.data[(win.origin as isize + dy as isize * win.stride as isize + dx as isize) as usize] as i32
    };
    // Horizontal half-sample (unrounded) at integer row dy, between columns dx and dx+1.
    let b1 = |dx: i32, dy: i32| {
        tap6(s(dx - 2, dy), s(dx - 1, dy), s(dx, dy), s(dx + 1, dy), s(dx + 2, dy), s(dx + 3, dy))
    };
    // Vertical half-sample (unrounded) at column dx, between rows dy and dy+1.
    let h1 = |dx: i32, dy: i32| {
        tap6(s(dx, dy - 2), s(dx, dy - 1), s(dx, dy), s(dx, dy + 1), s(dx, dy + 2), s(dx, dy + 3))
    };
    let (wi, hi) = (w as i32, h as i32);
    match (fx, fy) {
        (0, 0) => {
            for j in 0..hi {
                for i in 0..wi {
                    dst[j as usize * PSTRIDE + i as usize] = s(i, j) as u8;
                }
            }
        }
        (_, 0) => {
            for j in 0..hi {
                for i in 0..wi {
                    let b = clip((b1(i, j) + 16) >> 5) as i32;
                    let v = match fx {
                        1 => (s(i, j) + b + 1) >> 1,
                        2 => b,
                        _ => (s(i + 1, j) + b + 1) >> 1,
                    };
                    dst[j as usize * PSTRIDE + i as usize] = v as u8;
                }
            }
        }
        (0, _) => {
            for j in 0..hi {
                for i in 0..wi {
                    let hh = clip((h1(i, j) + 16) >> 5) as i32;
                    let v = match fy {
                        1 => (s(i, j) + hh + 1) >> 1,
                        2 => hh,
                        _ => (s(i, j + 1) + hh + 1) >> 1,
                    };
                    dst[j as usize * PSTRIDE + i as usize] = v as u8;
                }
            }
        }
        _ if fx == 2 || fy == 2 => {
            // Needs the centre sample j from the unrounded horizontal half-samples of rows -2..h+3.
            let mut mid = [0i32; 21 * 16];
            for j in 0..hi + 5 {
                for i in 0..wi {
                    mid[j as usize * 16 + i as usize] = b1(i, j - 2);
                }
            }
            for j in 0..hi {
                for i in 0..wi {
                    let m = |r: i32| mid[(j + 2 + r) as usize * 16 + i as usize];
                    let j1 = tap6(m(-2), m(-1), m(0), m(1), m(2), m(3));
                    let jv = clip((j1 + 512) >> 10) as i32;
                    let v = match (fx, fy) {
                        (2, 2) => jv,
                        (2, 1) => (clip((m(0) + 16) >> 5) as i32 + jv + 1) >> 1,
                        (2, 3) => (clip((m(1) + 16) >> 5) as i32 + jv + 1) >> 1,
                        (1, 2) => (clip((h1(i, j) + 16) >> 5) as i32 + jv + 1) >> 1,
                        _ => (clip((h1(i + 1, j) + 16) >> 5) as i32 + jv + 1) >> 1,
                    };
                    dst[j as usize * PSTRIDE + i as usize] = v as u8;
                }
            }
        }
        _ => {
            // Diagonal quarter positions e, g, p, r: average of a horizontal and a vertical half-sample.
            for j in 0..hi {
                for i in 0..wi {
                    let bh = clip((b1(i, j + (fy == 3) as i32) + 16) >> 5) as i32;
                    let hv = clip((h1(i + (fx == 3) as i32, j) + 16) >> 5) as i32;
                    dst[j as usize * PSTRIDE + i as usize] = ((bh + hv + 1) >> 1) as u8;
                }
            }
        }
    }
}

/// Chroma motion compensation (4:2:0) for a `w` x `h` block at integer chroma position `(x, y)` and eighth-sample
/// fraction `(fx, fy)`. The result goes to `dst` with stride [`PSTRIDE`].
#[allow(clippy::too_many_arguments)]
pub fn mc_chroma(
    plane: &[u8],
    stride: usize,
    pw: i32,
    ph: i32,
    x: i32,
    y: i32,
    fx: i32,
    fy: i32,
    w: usize,
    h: usize,
    dst: &mut [u8],
) {
    let inside = x >= 0 && y >= 0 && x + w as i32 + 1 <= pw && y + h as i32 + 1 <= ph;
    let (a, b, c, d) = ((8 - fx) * (8 - fy), fx * (8 - fy), (8 - fx) * fy, fx * fy);
    if inside {
        for j in 0..h {
            let o = (y as usize + j) * stride + x as usize;
            let (r0, r1) = (&plane[o..o + w + 1], &plane[o + stride..o + stride + w + 1]);
            for i in 0..w {
                dst[j * PSTRIDE + i] =
                    ((a * r0[i] as i32 + b * r0[i + 1] as i32 + c * r1[i] as i32 + d * r1[i + 1] as i32 + 32)
                        >> 6) as u8;
            }
        }
    } else {
        let at = |xx: i32, yy: i32| {
            plane[yy.clamp(0, ph - 1) as usize * stride + xx.clamp(0, pw - 1) as usize] as i32
        };
        for j in 0..h as i32 {
            for i in 0..w as i32 {
                let v = (a * at(x + i, y + j)
                    + b * at(x + i + 1, y + j)
                    + c * at(x + i, y + j + 1)
                    + d * at(x + i + 1, y + j + 1)
                    + 32)
                    >> 6;
                dst[j as usize * PSTRIDE + i as usize] = v as u8;
            }
        }
    }
}

/// Weighted sample prediction parameters for one component.
#[derive(Clone, Copy, Debug)]
pub struct Weights {
    /// `logWD`.
    pub log_wd: u32,
    /// `w0`, `w1`.
    pub w: [i32; 2],
    /// `o0`, `o1`.
    pub o: [i32; 2],
}

/// Combine up to two predictions into `dst` (a plane region with `dst_stride`), `w` x `h` samples.
/// `p0`/`p1` are predictions with stride [`PSTRIDE`]; `weights` is `None` for default weighting.
#[allow(clippy::too_many_arguments)]
pub fn combine(
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    p0: Option<&[u8]>,
    p1: Option<&[u8]>,
    weights: Option<&Weights>,
) {
    match (p0, p1, weights) {
        (Some(a), Some(b), None) => {
            for j in 0..h {
                for i in 0..w {
                    dst[j * dst_stride + i] =
                        ((a[j * PSTRIDE + i] as u32 + b[j * PSTRIDE + i] as u32 + 1) >> 1) as u8;
                }
            }
        }
        (Some(a), Some(b), Some(wt)) => {
            let rnd = 1i32 << wt.log_wd;
            let off = (wt.o[0] + wt.o[1] + 1) >> 1;
            for j in 0..h {
                for i in 0..w {
                    let v =
                        ((a[j * PSTRIDE + i] as i32 * wt.w[0] + b[j * PSTRIDE + i] as i32 * wt.w[1] + rnd)
                            >> (wt.log_wd + 1))
                            + off;
                    dst[j * dst_stride + i] = clip(v);
                }
            }
        }
        (Some(a), None, w_) | (None, Some(a), w_) => {
            let which = if p0.is_some() { 0 } else { 1 };
            match w_ {
                None => {
                    for j in 0..h {
                        dst[j * dst_stride..j * dst_stride + w]
                            .copy_from_slice(&a[j * PSTRIDE..j * PSTRIDE + w]);
                    }
                }
                Some(wt) => {
                    let (wgt, o) = (wt.w[which], wt.o[which]);
                    for j in 0..h {
                        for i in 0..w {
                            let p = a[j * PSTRIDE + i] as i32;
                            let v = if wt.log_wd >= 1 {
                                ((p * wgt + (1 << (wt.log_wd - 1))) >> wt.log_wd) + o
                            } else {
                                p * wgt + o
                            };
                            dst[j * dst_stride + i] = clip(v);
                        }
                    }
                }
            }
        }
        (None, None, _) => {}
    }
}
