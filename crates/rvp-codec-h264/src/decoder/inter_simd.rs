//! WebAssembly SIMD128 versions of the interpolation kernels in `inter` (8 samples per vector, 16-bit lanes). The
//! scalar code in `inter` defines the result; `selftest` compares the two on random pictures inside WebAssembly.
#![cfg(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64"))]

use super::inter::{self, PSTRIDE, Weights};
use alloc::vec;
use alloc::vec::Vec;
use rvp_core::simd::*;

/// Stride of the clamped copy used near the picture edges (wide enough that every vector load stays inside a row).
const TS: usize = 32;

/// Six-tap filter on eight lanes: `a - 5b + 20c + 20d - 5e + f` (the range fits 16 bits).
#[inline(always)]
fn tap6(a: v128, b: v128, c: v128, d: v128, e: v128, f: v128) -> v128 {
    let cd = i16x8_mul(i16x8_add(c, d), i16x8_splat(20));
    let be = i16x8_mul(i16x8_add(b, e), i16x8_splat(5));
    i16x8_sub(i16x8_add(i16x8_add(a, f), cd), be)
}

/// Eight bytes widened to 16-bit lanes.
#[inline(always)]
fn ld(s: &[u8]) -> v128 {
    u16x8_extend_low_u8x16(load8(s))
}

/// `clip((v + 16) >> 5)` as eight bytes in the low half.
#[inline(always)]
fn round5(v: v128) -> v128 {
    let r = i16x8_shr(i16x8_add(v, i16x8_splat(16)), 5);
    u8x16_narrow_i16x8(r, r)
}

/// Unrounded horizontal half-sample sums for eight columns; `r` starts two columns left of the first.
#[inline(always)]
fn hsum(r: &[u8]) -> v128 {
    tap6(ld(r), ld(&r[1..]), ld(&r[2..]), ld(&r[3..]), ld(&r[4..]), ld(&r[5..]))
}

/// Unrounded vertical half-sample sums for eight columns of six rows.
#[inline(always)]
fn vsum(rows: [&[u8]; 6]) -> v128 {
    tap6(ld(rows[0]), ld(rows[1]), ld(rows[2]), ld(rows[3]), ld(rows[4]), ld(rows[5]))
}

/// `(tap6 of six unrounded vectors + 512) >> 10`, clipped, eight bytes in the low half.
#[inline(always)]
fn centre(m: [v128; 6]) -> v128 {
    let side = |lo: bool| -> v128 {
        let w = |v: v128| if lo { i32x4_extend_low_i16x8(v) } else { i32x4_extend_high_i16x8(v) };
        let (a, b, c, d, e, f) = (w(m[0]), w(m[1]), w(m[2]), w(m[3]), w(m[4]), w(m[5]));
        let cd = i32x4_mul(i32x4_add(c, d), i32x4_splat(20));
        let be = i32x4_mul(i32x4_add(b, e), i32x4_splat(5));
        let s = i32x4_add(i32x4_sub(i32x4_add(i32x4_add(a, f), cd), be), i32x4_splat(512));
        i32x4_shr(s, 10)
    };
    let v = i16x8_narrow_i32x4(side(true), side(false));
    u8x16_narrow_i16x8(v, v)
}

/// Store the low `n` (4 or 8) bytes of `v`.
#[inline(always)]
fn put(dst: &mut [u8], v: v128, n: usize) {
    if n == 4 {
        store4(dst, v);
    } else {
        store8(dst, v);
    }
}

/// Load the low `n` bytes (4 or 8) of a row.
#[inline(always)]
fn get(src: &[u8], n: usize) -> v128 {
    if n == 4 { load4(src) } else { load8(src) }
}

/// Luma motion compensation; the same contract as `inter::mc_luma_scalar`.
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
    let mut tmp = [0u8; 21 * TS];
    // Eight-lane loads read up to `max(w, 8) + 5` columns from the block's left edge minus two.
    let inside = x >= 2 && y >= 2 && x + w.max(8) as i32 + 6 <= pw && y + h as i32 + 3 <= ph;
    let (src, ss, org): (&[u8], usize, usize) = if inside {
        (plane, stride, y as usize * stride + x as usize)
    } else {
        for j in 0..h + 5 {
            let sy = (y - 2 + j as i32).clamp(0, ph - 1) as usize;
            let row = &plane[sy * stride..sy * stride + pw as usize];
            for i in 0..w.max(8) + 5 {
                tmp[j * TS + i] = row[(x - 2 + i as i32).clamp(0, pw - 1) as usize];
            }
        }
        (&tmp, TS, 2 * TS + 2)
    };
    // The row `dy` of the block starting at column `dx` (both may be negative down to -2), at least 16 bytes long.
    let at = |dy: i32, dx: i32| -> &[u8] {
        &src[(org as isize + dy as isize * ss as isize + dx as isize) as usize..]
    };
    let n = w.min(8); // lanes per chunk: 4 or 8
    for c0 in (0..w).step_by(8) {
        let c0i = c0 as i32;
        match (fx, fy) {
            (0, 0) => {
                for j in 0..h {
                    let o = j * ds + c0;
                    if w == 16 && c0 == 0 {
                        store(&mut dst[o..], load(at(j as i32, 0)));
                    } else if w != 16 {
                        put(&mut dst[o..], get(at(j as i32, c0i), n), n);
                    }
                }
            }
            (_, 0) => {
                for j in 0..h {
                    let r = at(j as i32, c0i - 2);
                    let b = round5(hsum(r));
                    let v = match fx {
                        1 => u8x16_avgr(get(&r[2..], n), b),
                        2 => b,
                        _ => u8x16_avgr(get(&r[3..], n), b),
                    };
                    put(&mut dst[j * ds + c0..], v, n);
                }
            }
            (0, _) => {
                for j in 0..h as i32 {
                    let rows = [-2, -1, 0, 1, 2, 3].map(|k| at(j + k, c0i));
                    let hv = round5(vsum(rows));
                    let v = match fy {
                        1 => u8x16_avgr(get(at(j, c0i), n), hv),
                        2 => hv,
                        _ => u8x16_avgr(get(at(j + 1, c0i), n), hv),
                    };
                    put(&mut dst[j as usize * ds + c0..], v, n);
                }
            }
            _ if fx == 2 || fy == 2 => {
                // Unrounded horizontal sums for rows -2..h+3, then the centre sample from six of them.
                let mut mid = [i16x8_splat(0); 21];
                for (k, m) in mid.iter_mut().enumerate().take(h + 5) {
                    *m = hsum(at(k as i32 - 2, c0i - 2));
                }
                for j in 0..h {
                    let jv = centre([mid[j], mid[j + 1], mid[j + 2], mid[j + 3], mid[j + 4], mid[j + 5]]);
                    let v = match (fx, fy) {
                        (2, 2) => jv,
                        (2, 1) => u8x16_avgr(round5(mid[j + 2]), jv),
                        (2, _) => u8x16_avgr(round5(mid[j + 3]), jv),
                        _ => {
                            let rows = [-2, -1, 0, 1, 2, 3].map(|k| at(j as i32 + k, c0i + (fx == 3) as i32));
                            u8x16_avgr(round5(vsum(rows)), jv)
                        }
                    };
                    put(&mut dst[j * ds + c0..], v, n);
                }
            }
            _ => {
                // Diagonal quarter positions: the average of a horizontal and a vertical half-sample.
                for j in 0..h as i32 {
                    let bh = round5(hsum(at(j + (fy == 3) as i32, c0i - 2)));
                    let rows = [-2, -1, 0, 1, 2, 3].map(|k| at(j + k, c0i + (fx == 3) as i32));
                    let hv = round5(vsum(rows));
                    put(&mut dst[j as usize * ds + c0..], u8x16_avgr(bh, hv), n);
                }
            }
        }
    }
}

/// Chroma motion compensation; the same contract as `inter::mc_chroma_scalar`.
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
    if w < 4 {
        return inter::mc_chroma_scalar(plane, stride, pw, ph, x, y, fx, fy, w, h, dst, ds);
    }
    let inside = x >= 0 && y >= 0 && x + (w as i32) < pw && y + (h as i32) < ph;
    let mut tmp = [0u8; 17 * 17];
    let (src, ss, org): (&[u8], usize, usize) = if inside {
        (plane, stride, y as usize * stride + x as usize)
    } else {
        for j in 0..h + 1 {
            let sy = (y + j as i32).clamp(0, ph - 1) as usize;
            for i in 0..w + 1 {
                tmp[j * 17 + i] = plane[sy * stride + (x + i as i32).clamp(0, pw - 1) as usize];
            }
        }
        (&tmp, 17, 0)
    };
    let (a, b, c, d) = ((8 - fx) * (8 - fy), fx * (8 - fy), (8 - fx) * fy, fx * fy);
    let (va, vb, vc, vd) =
        (i16x8_splat(a as i16), i16x8_splat(b as i16), i16x8_splat(c as i16), i16x8_splat(d as i16));
    let rnd = i16x8_splat(32);
    for j in 0..h {
        let o = org + j * ss;
        let out = &mut dst[j * ds..];
        if fx == 0 && fy == 0 {
            put(out, get(&src[o..], w), w);
            continue;
        }
        let acc = |i: usize| -> v128 {
            let r0 = ld_n(&src[o + i..], w);
            let r1 = ld_n(&src[o + ss + i..], w);
            let r0b = ld_n(&src[o + i + 1..], w);
            let r1b = ld_n(&src[o + ss + i + 1..], w);
            let s = i16x8_add(
                i16x8_add(i16x8_mul(r0, va), i16x8_mul(r0b, vb)),
                i16x8_add(i16x8_mul(r1, vc), i16x8_mul(r1b, vd)),
            );
            let t = i16x8_shr(i16x8_add(s, rnd), 6);
            u8x16_narrow_i16x8(t, t)
        };
        put(out, acc(0), w);
    }
}

/// `w` (4 or 8) bytes widened to 16-bit lanes.
#[inline(always)]
fn ld_n(s: &[u8], w: usize) -> v128 {
    u16x8_extend_low_u8x16(get(s, w))
}

/// `combine` for two predictions with default weights: `(a + b + 1) >> 1`.
pub fn average(dst: &mut [u8], ds: usize, w: usize, h: usize, a: &[u8], b: &[u8]) {
    for j in 0..h {
        let (ra, rb) = (&a[j * PSTRIDE..], &b[j * PSTRIDE..]);
        let d = &mut dst[j * ds..];
        match w {
            16 => store(d, u8x16_avgr(load(ra), load(rb))),
            8 => store8(d, u8x16_avgr(load8(ra), load8(rb))),
            4 => store4(d, u8x16_avgr(load4(ra), load4(rb))),
            _ => {
                for i in 0..w {
                    d[i] = ((ra[i] as u32 + rb[i] as u32 + 1) >> 1) as u8;
                }
            }
        }
    }
}

/// `combine` for two predictions with weights: `clip(((a*w0 + b*w1 + 2^logWD) >> (logWD + 1)) + o)`.
pub fn weighted_bi(dst: &mut [u8], ds: usize, w: usize, h: usize, a: &[u8], b: &[u8], wt: &Weights) {
    if w < 4 {
        // Not produced by the decoder (blocks are at least 2 wide for chroma); keep the scalar loop for safety.
        return weighted_bi_scalar(dst, ds, w, h, a, b, wt);
    }
    let rnd = i32x4_splat(1 << wt.log_wd);
    let off = i32x4_splat((wt.o[0] + wt.o[1] + 1) >> 1);
    let sh = wt.log_wd + 1;
    // (a, b) byte pairs as 16-bit lanes against (w0, w1) pairs: one dot product gives a*w0 + b*w1 per sample.
    let wp = i32x4_splat((wt.w[0] & 0xFFFF) | (wt.w[1] << 16));
    for j in 0..h {
        let (ra, rb) = (&a[j * PSTRIDE..], &b[j * PSTRIDE..]);
        let d = &mut dst[j * ds..];
        for c0 in (0..w).step_by(8) {
            let n = (w - c0).min(8);
            let (va, vb) = (get(&ra[c0..], n), get(&rb[c0..], n));
            let pairs = i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(va, vb);
            let lo = u16x8_extend_low_u8x16(pairs);
            let hi = u16x8_extend_high_u8x16(pairs);
            let f = |p: v128| i32x4_add(i32x4_shr(i32x4_add(i32x4_dot_i16x8(p, wp), rnd), sh), off);
            let r = i16x8_narrow_i32x4(f(lo), f(hi));
            put(&mut d[c0..], u8x16_narrow_i16x8(r, r), n);
        }
    }
}

fn weighted_bi_scalar(dst: &mut [u8], ds: usize, w: usize, h: usize, a: &[u8], b: &[u8], wt: &Weights) {
    let rnd = 1i32 << wt.log_wd;
    let off = (wt.o[0] + wt.o[1] + 1) >> 1;
    let sh = wt.log_wd + 1;
    for j in 0..h {
        for i in 0..w {
            let v = ((a[j * PSTRIDE + i] as i32 * wt.w[0] + b[j * PSTRIDE + i] as i32 * wt.w[1] + rnd) >> sh)
                + off;
            dst[j * ds + i] = v.clamp(0, 255) as u8;
        }
    }
}

/// Compare the vector kernels with the scalar ones on pseudo-random pictures; returns the number of mismatches.
pub fn selftest() -> u32 {
    let mut seed = 0xC0FF_EE11u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let (pw, ph) = (64usize, 48usize);
    let plane: Vec<u8> = (0..pw * ph).map(|_| rnd() as u8).collect();
    let mut bad = 0;
    // Luma: every fraction, every block size, positions inside and outside the picture.
    for &(w, h) in &[(4usize, 4usize), (8, 4), (4, 8), (8, 8), (16, 8), (8, 16), (16, 16)] {
        for fy in 0..4usize {
            for fx in 0..4usize {
                for &(x, y) in &[
                    (20i32, 16i32),
                    (0, 0),
                    (-5, 3),
                    (pw as i32 - w as i32, ph as i32 - h as i32),
                    (60, 45),
                    (2, 2),
                    (45, 20),
                ] {
                    let mut a = vec![0u8; PSTRIDE * 16 + 32];
                    let mut b = a.clone();
                    inter::mc_luma_scalar(
                        &plane, pw, pw as i32, ph as i32, x, y, fx, fy, w, h, &mut a, PSTRIDE,
                    );
                    mc_luma(&plane, pw, pw as i32, ph as i32, x, y, fx, fy, w, h, &mut b, PSTRIDE);
                    bad += (a != b) as u32;
                }
            }
        }
    }
    // Chroma.
    for &(w, h) in &[(2usize, 2usize), (4, 4), (8, 4), (4, 8), (8, 8)] {
        for fy in 0..8 {
            for fx in 0..8 {
                for &(x, y) in &[
                    (20i32, 16i32),
                    (0, 0),
                    (-3, 2),
                    (pw as i32 - w as i32, ph as i32 - h as i32),
                    (62, 46),
                    (40, 30),
                ] {
                    let mut a = vec![0u8; PSTRIDE * 16 + 32];
                    let mut b = a.clone();
                    inter::mc_chroma_scalar(
                        &plane, pw, pw as i32, ph as i32, x, y, fx, fy, w, h, &mut a, PSTRIDE,
                    );
                    mc_chroma(&plane, pw, pw as i32, ph as i32, x, y, fx, fy, w, h, &mut b, PSTRIDE);
                    bad += (a != b) as u32;
                }
            }
        }
    }
    // Combination of two predictions.
    let pa: Vec<u8> = (0..PSTRIDE * 16).map(|_| rnd() as u8).collect();
    let pb: Vec<u8> = (0..PSTRIDE * 16).map(|_| rnd() as u8).collect();
    for &(w, h) in &[(4usize, 4usize), (8, 8), (16, 16), (2, 2), (8, 4)] {
        let mut a = vec![0u8; 16 * 16];
        let mut b = a.clone();
        inter::combine_scalar(&mut a, 16, w, h, Some(&pa), Some(&pb), None);
        average(&mut b, 16, w, h, &pa, &pb);
        bad += (a != b) as u32;
        for &(w0, w1, log_wd, o0, o1) in &[
            (32, 32, 5u32, 0, 0),
            (20, 44, 5, 0, 0),
            (-10, 70, 5, 3, -4),
            (64, 64, 6, 7, 9),
            (128, -64, 7, -20, 30),
            (1, 1, 0, 0, 0),
            (-64, -64, 5, 100, 100),
        ] {
            let wt = Weights { log_wd, w: [w0, w1], o: [o0, o1] };
            let mut a = vec![0u8; 16 * 16];
            let mut b = a.clone();
            inter::combine_scalar(&mut a, 16, w, h, Some(&pa), Some(&pb), Some(&wt));
            weighted_bi(&mut b, 16, w, h, &pa, &pb, &wt);
            bad += (a != b) as u32;
        }
    }
    bad
}
