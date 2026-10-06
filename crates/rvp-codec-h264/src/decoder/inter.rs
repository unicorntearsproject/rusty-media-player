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

/// Copy the `(w + 5) x (h + 5)` window around `(x, y)` with coordinates clamped to the plane into `tmp` (stride 21).
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

#[inline(always)]
fn avg(a: u8, b: u8) -> u8 {
    ((a as u32 + b as u32 + 1) >> 1) as u8
}

/// Horizontal half-sample (rounded, clipped) of one source row `r` (which starts 2 samples left of the block).
#[inline(always)]
fn hhalf(r: &[u8], out: &mut [u8; MAX_BLK], w: usize) {
    let r = &r[..w + 5];
    for i in 0..w {
        out[i] = clip(
            (tap6(
                r[i] as i32,
                r[i + 1] as i32,
                r[i + 2] as i32,
                r[i + 3] as i32,
                r[i + 4] as i32,
                r[i + 5] as i32,
            ) + 16)
                >> 5,
        );
    }
}

/// Vertical half-sample (rounded, clipped) for columns `i + c0` of the six rows `rows` (each pointing at column 0 of
/// the block, with `c0 + w` samples available).
#[inline(always)]
fn vhalf(rows: [&[u8]; 6], c0: usize, out: &mut [u8; MAX_BLK], w: usize) {
    let [r0, r1, r2, r3, r4, r5] = rows.map(|r| &r[c0..c0 + w]);
    for i in 0..w {
        out[i] = clip(
            (tap6(r0[i] as i32, r1[i] as i32, r2[i] as i32, r3[i] as i32, r4[i] as i32, r5[i] as i32) + 16)
                >> 5,
        );
    }
}

/// Luma motion compensation (see [`mc_luma_scalar`], which defines the result): the WebAssembly SIMD128 build uses
/// the vector kernels in `inter_simd`.
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
    ds: usize,
) {
    #[cfg(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64"))]
    return super::inter_simd::mc_luma(plane, stride, pw, ph, x, y, fx, fy, w, h, dst, ds);
    #[cfg(not(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64")))]
    mc_luma_scalar(plane, stride, pw, ph, x, y, fx, fy, w, h, dst, ds)
}

/// Chroma motion compensation (see [`mc_chroma_scalar`]).
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
    ds: usize,
) {
    #[cfg(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64"))]
    return super::inter_simd::mc_chroma(plane, stride, pw, ph, x, y, fx, fy, w, h, dst, ds);
    #[cfg(not(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64")))]
    mc_chroma_scalar(plane, stride, pw, ph, x, y, fx, fy, w, h, dst, ds)
}

/// Luma motion compensation for a `w` x `h` block (each 4, 8 or 16) whose top-left sample is at integer position
/// `(x, y)` in the reference `plane` plus the quarter-sample fraction `(fx, fy)`. The result goes to `dst` with
/// stride `ds`.
#[allow(clippy::too_many_arguments)]
pub fn mc_luma_scalar(
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
    ds: usize,
) {
    let mut tmp = [0u8; 21 * 21];
    let inside = x >= 2 && y >= 2 && x + w as i32 + 3 <= pw && y + h as i32 + 3 <= ph;
    // `src[origin + dy * sstride + dx]` is the reference sample at block offset (dx, dy), valid for dx in -2..w+3 and
    // dy in -2..h+3.
    let (src, sstride, origin): (&[u8], usize, usize) = if inside {
        (plane, stride, y as usize * stride + x as usize)
    } else {
        fetch_clamped(plane, stride, pw, ph, x, y, w, h, &mut tmp);
        (&tmp, 21, 2 * 21 + 2)
    };
    // Row `dy` starting at dx = -2 (so index `i + 2` is column `i`), at least `w + 5` samples long.
    let row = |dy: i32| -> &[u8] {
        let start = (origin as isize + dy as isize * sstride as isize - 2) as usize;
        &src[start..start + w + 5]
    };
    // Row `dy` starting at column 0.
    let col0 = |dy: i32| -> &[u8] { &row(dy)[2..] };
    match (fx, fy) {
        (0, 0) => {
            for j in 0..h {
                dst[j * ds..j * ds + w].copy_from_slice(&col0(j as i32)[..w]);
            }
        }
        (_, 0) => {
            let mut b = [0u8; MAX_BLK];
            for j in 0..h {
                let r = row(j as i32);
                hhalf(r, &mut b, w);
                let d = &mut dst[j * ds..j * ds + w];
                match fx {
                    1 => {
                        for i in 0..w {
                            d[i] = avg(r[i + 2], b[i]);
                        }
                    }
                    2 => d.copy_from_slice(&b[..w]),
                    _ => {
                        for i in 0..w {
                            d[i] = avg(r[i + 3], b[i]);
                        }
                    }
                }
            }
        }
        (0, _) => {
            let mut hv = [0u8; MAX_BLK];
            for j in 0..h as i32 {
                vhalf(
                    [col0(j - 2), col0(j - 1), col0(j), col0(j + 1), col0(j + 2), col0(j + 3)],
                    0,
                    &mut hv,
                    w,
                );
                let d = &mut dst[j as usize * ds..j as usize * ds + w];
                match fy {
                    1 => {
                        let g = col0(j);
                        for i in 0..w {
                            d[i] = avg(g[i], hv[i]);
                        }
                    }
                    2 => d.copy_from_slice(&hv[..w]),
                    _ => {
                        let m = col0(j + 1);
                        for i in 0..w {
                            d[i] = avg(m[i], hv[i]);
                        }
                    }
                }
            }
        }
        _ if fx == 2 || fy == 2 => {
            // The centre sample j from the unrounded horizontal half-samples of rows -2..h+3.
            let mut mid = [[0i32; MAX_BLK]; MAX_BLK + 5];
            for jj in 0..h + 5 {
                let r = &row(jj as i32 - 2)[..w + 5];
                for i in 0..w {
                    mid[jj][i] = tap6(
                        r[i] as i32,
                        r[i + 1] as i32,
                        r[i + 2] as i32,
                        r[i + 3] as i32,
                        r[i + 4] as i32,
                        r[i + 5] as i32,
                    );
                }
            }
            let mut other = [0u8; MAX_BLK];
            for j in 0..h {
                let (m0, m1, m2, m3, m4, m5) =
                    (&mid[j], &mid[j + 1], &mid[j + 2], &mid[j + 3], &mid[j + 4], &mid[j + 5]);
                let d = &mut dst[j * ds..j * ds + w];
                // The neighbouring half-sample to average with, if any.
                match (fx, fy) {
                    (2, 1) => {
                        for i in 0..w {
                            other[i] = clip((m2[i] + 16) >> 5);
                        }
                    }
                    (2, 3) => {
                        for i in 0..w {
                            other[i] = clip((m3[i] + 16) >> 5);
                        }
                    }
                    (1, 2) | (3, 2) => {
                        let jj = j as i32;
                        vhalf(
                            [col0(jj - 2), col0(jj - 1), col0(jj), col0(jj + 1), col0(jj + 2), col0(jj + 3)],
                            (fx == 3) as usize,
                            &mut other,
                            w,
                        )
                    }
                    _ => {}
                }
                for i in 0..w {
                    let jv = clip((tap6(m0[i], m1[i], m2[i], m3[i], m4[i], m5[i]) + 512) >> 10);
                    d[i] = if (fx, fy) == (2, 2) { jv } else { avg(other[i], jv) };
                }
            }
        }
        _ => {
            // Diagonal quarter positions e, g, p, r: average of a horizontal and a vertical half-sample.
            let (mut bh, mut hv) = ([0u8; MAX_BLK], [0u8; MAX_BLK]);
            for j in 0..h as i32 {
                hhalf(row(j + (fy == 3) as i32), &mut bh, w);
                vhalf(
                    [col0(j - 2), col0(j - 1), col0(j), col0(j + 1), col0(j + 2), col0(j + 3)],
                    (fx == 3) as usize,
                    &mut hv,
                    w,
                );
                let d = &mut dst[j as usize * ds..j as usize * ds + w];
                for i in 0..w {
                    d[i] = avg(bh[i], hv[i]);
                }
            }
        }
    }
}

/// Chroma motion compensation (4:2:0) for a `w` x `h` block at integer chroma position `(x, y)` and eighth-sample
/// fraction `(fx, fy)`. The result goes to `dst` with stride `ds`.
#[allow(clippy::too_many_arguments)]
pub fn mc_chroma_scalar(
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
    ds: usize,
) {
    let inside = x >= 0 && y >= 0 && x + (w as i32) < pw && y + (h as i32) < ph;
    let (a, b, c, d) = ((8 - fx) * (8 - fy), fx * (8 - fy), (8 - fx) * fy, fx * fy);
    let mut tmp = [0u8; 17 * 17];
    let (src, sstride, origin): (&[u8], usize, usize) = if inside {
        (plane, stride, y as usize * stride + x as usize)
    } else {
        for j in 0..h + 1 {
            let sy = (y + j as i32).clamp(0, ph - 1) as usize;
            for i in 0..w + 1 {
                let sx = (x + i as i32).clamp(0, pw - 1) as usize;
                tmp[j * 17 + i] = plane[sy * stride + sx];
            }
        }
        (&tmp, 17, 0)
    };
    for j in 0..h {
        let o = origin + j * sstride;
        let out = &mut dst[j * ds..j * ds + w];
        if fx == 0 && fy == 0 {
            out.copy_from_slice(&src[o..o + w]);
        } else if fy == 0 {
            // (8*((8-fx)*A + fx*B) + 32) >> 6 == ((8-fx)*A + fx*B + 4) >> 3
            let r0 = &src[o..o + w + 1];
            for i in 0..w {
                out[i] = (((8 - fx) * r0[i] as i32 + fx * r0[i + 1] as i32 + 4) >> 3) as u8;
            }
        } else if fx == 0 {
            let (r0, r1) = (&src[o..o + w], &src[o + sstride..o + sstride + w]);
            for i in 0..w {
                out[i] = (((8 - fy) * r0[i] as i32 + fy * r1[i] as i32 + 4) >> 3) as u8;
            }
        } else {
            let (r0, r1) = (&src[o..o + w + 1], &src[o + sstride..o + sstride + w + 1]);
            for i in 0..w {
                out[i] =
                    ((a * r0[i] as i32 + b * r0[i + 1] as i32 + c * r1[i] as i32 + d * r1[i + 1] as i32 + 32)
                        >> 6) as u8;
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

/// Combine up to two predictions into `dst` (see [`combine_scalar`], which defines the result).
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
    #[cfg(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64"))]
    if let (Some(a), Some(b)) = (p0, p1) {
        return match weights {
            None => super::inter_simd::average(dst, dst_stride, w, h, a, b),
            Some(wt) => super::inter_simd::weighted_bi(dst, dst_stride, w, h, a, b, wt),
        };
    }
    combine_scalar(dst, dst_stride, w, h, p0, p1, weights)
}

/// Combine up to two predictions into `dst` (a plane region with `dst_stride`), `w` x `h` samples.
/// `p0`/`p1` are predictions with stride [`PSTRIDE`]; `weights` is `None` for default weighting.
#[allow(clippy::too_many_arguments)]
pub fn combine_scalar(
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
                let (ra, rb) = (&a[j * PSTRIDE..j * PSTRIDE + w], &b[j * PSTRIDE..j * PSTRIDE + w]);
                let d = &mut dst[j * dst_stride..j * dst_stride + w];
                for i in 0..w {
                    d[i] = ((ra[i] as u32 + rb[i] as u32 + 1) >> 1) as u8;
                }
            }
        }
        (Some(a), Some(b), Some(wt)) => {
            let rnd = 1i32 << wt.log_wd;
            let off = (wt.o[0] + wt.o[1] + 1) >> 1;
            let sh = wt.log_wd + 1;
            for j in 0..h {
                let (ra, rb) = (&a[j * PSTRIDE..j * PSTRIDE + w], &b[j * PSTRIDE..j * PSTRIDE + w]);
                let d = &mut dst[j * dst_stride..j * dst_stride + w];
                for i in 0..w {
                    d[i] = clip(((ra[i] as i32 * wt.w[0] + rb[i] as i32 * wt.w[1] + rnd) >> sh) + off);
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
                    let (rnd, sh) = if wt.log_wd >= 1 { (1 << (wt.log_wd - 1), wt.log_wd) } else { (0, 0) };
                    for j in 0..h {
                        let ra = &a[j * PSTRIDE..j * PSTRIDE + w];
                        let d = &mut dst[j * dst_stride..j * dst_stride + w];
                        for i in 0..w {
                            d[i] = clip(((ra[i] as i32 * wgt + rnd) >> sh) + o);
                        }
                    }
                }
            }
        }
        (None, None, _) => {}
    }
}
