//! WebAssembly SIMD128 kernels (added for rusty-video-player; see PATCHES.md): the 8-tap motion-compensation filters and
//! the loop filter's edge kernels. They do exactly the integer math of the scalar code in `inter.rs` and `loopfilter.rs`
//! (those stay the reference and the fallback); `selftest` compares the two inside WebAssembly.
#![cfg(all(target_arch = "wasm32", target_feature = "simd128"))]

use crate::inter::{RefPlane, SUBPEL_FILTERS};
use core::arch::wasm32::*;

// ---- loads and stores of u16 lanes -------------------------------------------------------------------------------

#[inline(always)]
fn ld8(s: &[u16]) -> v128 {
    let s = &s[..8];
    // SAFETY: 8 readable u16 (16 bytes); `v128_load` has no alignment requirement.
    unsafe { v128_load(s.as_ptr().cast()) }
}

#[inline(always)]
fn st8(s: &mut [u16], v: v128) {
    let s = &mut s[..8];
    // SAFETY: 8 writable u16.
    unsafe { v128_store(s.as_mut_ptr().cast(), v) }
}

// ---- motion compensation -------------------------------------------------------------------------------------------

/// The 8 taps as 4 lanes-of-pairs `(f[2k], f[2k+1])`, for `i32x4_dot_i16x8`.
#[inline(always)]
fn tap_pairs(f: &[i32; 8]) -> [v128; 4] {
    core::array::from_fn(|k| i32x4_splat((f[2 * k] & 0xFFFF) | (f[2 * k + 1] << 16)))
}

/// 8-tap convolution, bit-identical to the scalar `Σ_k src[i + k*tap_stride] * f[k]`, rounded `>>7` and clamped to
/// `[0, max]` (and averaged into `dst` for the second reference of a compound). Eight outputs at a time, then four.
///
/// # Safety
/// `src` must be readable for `i + 7*tap_stride + 7` u16s (`+ 3` for the final four outputs) and `dst` writable for `n`
/// u16s; the caller checks that the whole window lies inside the plane. `n` is a multiple of 4.
#[inline]
unsafe fn conv8(
    src: *const u16,
    tap_stride: usize,
    f: &[i32; 8],
    dst: *mut u16,
    n: usize,
    max: i32,
    avg: bool,
) {
    let taps = tap_pairs(f);
    let (round, zero, maxv) = (i32x4_splat(64), i32x4_splat(0), i32x4_splat(max));
    let mut i = 0usize;
    while i + 8 <= n {
        let (mut lo, mut hi) = (zero, zero);
        for (k, t) in taps.iter().enumerate() {
            // SAFETY: by the contract, 8 u16 are readable at each tap.
            let (a, b) = unsafe {
                (
                    v128_load(src.add(i + 2 * k * tap_stride).cast()),
                    v128_load(src.add(i + (2 * k + 1) * tap_stride).cast()),
                )
            };
            let il = i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(a, b);
            let ih = i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(a, b);
            lo = i32x4_add(lo, i32x4_dot_i16x8(il, *t));
            hi = i32x4_add(hi, i32x4_dot_i16x8(ih, *t));
        }
        lo = i32x4_min(i32x4_max(i32x4_shr(i32x4_add(lo, round), 7), zero), maxv);
        hi = i32x4_min(i32x4_max(i32x4_shr(i32x4_add(hi, round), 7), zero), maxv);
        let mut r = u16x8_narrow_i32x4(lo, hi);
        // SAFETY: `dst` is writable for `n` u16s.
        unsafe {
            if avg {
                r = u16x8_avgr(r, v128_load(dst.add(i).cast()));
            }
            v128_store(dst.add(i).cast(), r);
        }
        i += 8;
    }
    if i + 4 <= n {
        let mut acc = zero;
        for (k, t) in taps.iter().enumerate() {
            // SAFETY: 4 u16 are readable at each tap.
            let (a, b) = unsafe {
                (
                    v128_load64_zero(src.add(i + 2 * k * tap_stride).cast()),
                    v128_load64_zero(src.add(i + (2 * k + 1) * tap_stride).cast()),
                )
            };
            acc = i32x4_add(acc, i32x4_dot_i16x8(i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(a, b), *t));
        }
        acc = i32x4_min(i32x4_max(i32x4_shr(i32x4_add(acc, round), 7), zero), maxv);
        let mut r = u16x8_narrow_i32x4(acc, acc);
        // SAFETY: 4 u16 writable.
        unsafe {
            if avg {
                r = u16x8_avgr(r, v128_load64_zero(dst.add(i).cast()));
            }
            v128_store64_lane::<0>(r, dst.add(i).cast());
        }
    }
}

/// `dst[i] = (src[i] + dst[i] + 1) >> 1` for `n` (a multiple of 4) samples.
///
/// # Safety
/// `src` readable and `dst` writable for `n` u16s.
#[inline]
unsafe fn avg_row(src: *const u16, dst: *mut u16, n: usize) {
    let mut i = 0;
    // SAFETY: by the contract.
    unsafe {
        while i + 8 <= n {
            v128_store(
                dst.add(i).cast(),
                u16x8_avgr(v128_load(src.add(i).cast()), v128_load(dst.add(i).cast())),
            );
            i += 8;
        }
        if i + 4 <= n {
            let r = u16x8_avgr(v128_load64_zero(src.add(i).cast()), v128_load64_zero(dst.add(i).cast()));
            v128_store64_lane::<0>(r, dst.add(i).cast());
        }
    }
}

thread_local! {
    /// Scratch for the intermediate (horizontal) pass of the 2-D filter: up to (64 + 7) rows of 64.
    static TMP: core::cell::RefCell<[u16; 71 * 64]> = const { core::cell::RefCell::new([0; 71 * 64]) };
}

/// Separable motion compensation of an interior block: the same four cases as `predict_block`.
///
/// # Safety
/// The whole read window must lie inside the plane (the caller checks), and `dst` must hold `h` rows of `w` samples.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn predict_block_interior(
    refp: &RefPlane,
    bx: i32,
    by: i32,
    fx: &[i32; 8],
    fy: &[i32; 8],
    subx: bool,
    suby: bool,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
    max: i32,
    avg: bool,
) {
    let buf = refp.buf.as_ptr();
    let stride = refp.stride;
    let dptr = dst.as_mut_ptr();
    // SAFETY: see the function contract; every pointer below stays inside the checked window.
    unsafe {
        match (subx, suby) {
            (false, false) => {
                for y in 0..h {
                    let s = buf.add((by as usize + y) * stride + bx as usize);
                    let d = dptr.add(y * dst_stride);
                    if avg {
                        avg_row(s, d, w);
                    } else {
                        core::ptr::copy_nonoverlapping(s, d, w);
                    }
                }
            }
            (true, false) => {
                for y in 0..h {
                    let s = buf.add((by as usize + y) * stride + (bx - 3) as usize);
                    conv8(s, 1, fx, dptr.add(y * dst_stride), w, max, avg);
                }
            }
            (false, true) => {
                for y in 0..h {
                    let s = buf.add((by + y as i32 - 3) as usize * stride + bx as usize);
                    conv8(s, stride, fy, dptr.add(y * dst_stride), w, max, avg);
                }
            }
            (true, true) => TMP.with(|cell| {
                let mut tmp = cell.borrow_mut();
                let tptr = tmp.as_mut_ptr();
                for r in 0..h + 7 {
                    let s = buf.add((by + r as i32 - 3) as usize * stride + (bx - 3) as usize);
                    conv8(s, 1, fx, tptr.add(r * w), w, max, false);
                }
                for y in 0..h {
                    conv8(tptr.add(y * w) as *const u16, w, fy, dptr.add(y * dst_stride), w, max, avg);
                }
            }),
        }
    }
}

// ---- loop filter ------------------------------------------------------------------------------------------------------

/// Transpose an 8x8 tile of u16 (8 vectors of 8 lanes).
#[inline(always)]
fn transpose8x8(r: [v128; 8]) -> [v128; 8] {
    let (t0, t1) = (
        i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(r[0], r[1]),
        i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(r[0], r[1]),
    );
    let (t2, t3) = (
        i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(r[2], r[3]),
        i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(r[2], r[3]),
    );
    let (t4, t5) = (
        i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(r[4], r[5]),
        i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(r[4], r[5]),
    );
    let (t6, t7) = (
        i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(r[6], r[7]),
        i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(r[6], r[7]),
    );
    // 32-bit interleave of (t0,t2), (t1,t3), (t4,t6), (t5,t7).
    let (u0, u1) = (i32x4_shuffle::<0, 4, 1, 5>(t0, t2), i32x4_shuffle::<2, 6, 3, 7>(t0, t2));
    let (u2, u3) = (i32x4_shuffle::<0, 4, 1, 5>(t1, t3), i32x4_shuffle::<2, 6, 3, 7>(t1, t3));
    let (u4, u5) = (i32x4_shuffle::<0, 4, 1, 5>(t4, t6), i32x4_shuffle::<2, 6, 3, 7>(t4, t6));
    let (u6, u7) = (i32x4_shuffle::<0, 4, 1, 5>(t5, t7), i32x4_shuffle::<2, 6, 3, 7>(t5, t7));
    // 64-bit interleave.
    [
        i64x2_shuffle::<0, 2>(u0, u4),
        i64x2_shuffle::<1, 3>(u0, u4),
        i64x2_shuffle::<0, 2>(u1, u5),
        i64x2_shuffle::<1, 3>(u1, u5),
        i64x2_shuffle::<0, 2>(u2, u6),
        i64x2_shuffle::<1, 3>(u2, u6),
        i64x2_shuffle::<0, 2>(u3, u7),
        i64x2_shuffle::<1, 3>(u3, u7),
    ]
}

#[inline(always)]
fn absd(a: v128, b: v128) -> v128 {
    v128_or(u16x8_sub_sat(a, b), u16x8_sub_sat(b, a))
}

#[inline(always)]
fn le(d: v128, t: v128) -> v128 {
    u16x8_le(d, t)
}

#[inline(always)]
fn sel(m: v128, a: v128, b: v128) -> v128 {
    v128_bitselect(a, b, m)
}

/// The filter of one edge for 8 positions in parallel (lane = position); `p[k]` / `q[k]` are the samples `k + 1` before
/// and `k` after the edge. Returns the new `p0..p6` and `q0..q6` (unmodified where the width does not reach).
#[allow(clippy::too_many_arguments)]
fn lf_core(
    p: [v128; 8],
    q: [v128; 8],
    width: usize,
    lim: i32,
    blimit: i32,
    hev_thr: i32,
    bd: i32,
) -> ([v128; 7], [v128; 7]) {
    let base = 1i32 << (bd - 1);
    let ft = u16x8_splat(1 << (bd - 8));
    let (lim, blimit, hev_thr) =
        (u16x8_splat(lim as u16), u16x8_splat(blimit as u16), u16x8_splat(hev_thr as u16));
    let (p0, p1, p2, p3) = (p[0], p[1], p[2], p[3]);
    let (q0, q1, q2, q3) = (q[0], q[1], q[2], q[3]);
    // filter_mask
    let mut mask = v128_and(le(absd(p3, p2), lim), le(absd(p2, p1), lim));
    mask = v128_and(mask, le(absd(p1, p0), lim));
    mask = v128_and(mask, le(absd(q1, q0), lim));
    mask = v128_and(mask, le(absd(q2, q1), lim));
    mask = v128_and(mask, le(absd(q3, q2), lim));
    let grad = u16x8_add(u16x8_shl(absd(p0, q0), 1), u16x8_shr(absd(p1, q1), 1));
    mask = v128_and(mask, le(grad, blimit));
    // flat over the inner 4 taps
    let mut flat = v128_and(le(absd(p1, p0), ft), le(absd(q1, q0), ft));
    flat = v128_and(flat, le(absd(p2, p0), ft));
    flat = v128_and(flat, le(absd(q2, q0), ft));
    flat = v128_and(flat, le(absd(p3, p0), ft));
    flat = v128_and(flat, le(absd(q3, q0), ft));
    // high edge variance
    let hev = v128_or(u16x8_gt(absd(p1, p0), hev_thr), u16x8_gt(absd(q1, q0), hev_thr));

    // filter4 on offset (signed) samples
    let (bv, lo_c, hi_c) =
        (i16x8_splat(base as i16), i16x8_splat(-base as i16), i16x8_splat((base - 1) as i16));
    let scl = |v: v128| i16x8_min(i16x8_max(v, lo_c), hi_c);
    let (ps1, ps0, qs0, qs1) = (i16x8_sub(p1, bv), i16x8_sub(p0, bv), i16x8_sub(q0, bv), i16x8_sub(q1, bv));
    let f_hev = scl(i16x8_sub(ps1, qs1));
    let mut filt = v128_and(f_hev, hev);
    filt = scl(i16x8_add(filt, i16x8_mul(i16x8_sub(qs0, ps0), i16x8_splat(3))));
    let filter1 = i16x8_shr(scl(i16x8_add(filt, i16x8_splat(4))), 3);
    let filter2 = i16x8_shr(scl(i16x8_add(filt, i16x8_splat(3))), 3);
    let f4q0 = i16x8_add(scl(i16x8_sub(qs0, filter1)), bv);
    let f4p0 = i16x8_add(scl(i16x8_add(ps0, filter2)), bv);
    let ff = v128_andnot(i16x8_shr(i16x8_add(filter1, i16x8_splat(1)), 1), hev);
    let f4q1 = i16x8_add(scl(i16x8_sub(qs1, ff)), bv);
    let f4p1 = i16x8_add(scl(i16x8_add(ps1, ff)), bv);

    if width < 8 {
        let out_p = [sel(mask, f4p0, p0), sel(mask, f4p1, p1), p2, p3, p[4], p[5], p[6]];
        let out_q = [sel(mask, f4q0, q0), sel(mask, f4q1, q1), q2, q3, q[4], q[5], q[6]];
        return (out_p, out_q);
    }

    let r3 = |x: v128| u16x8_shr(u16x8_add(x, u16x8_splat(4)), 3);
    let add = |a: v128, b: v128| u16x8_add(a, b);
    let mul = |a: v128, k: u16| u16x8_mul(a, u16x8_splat(k));
    let f8p2 = r3(add(add(add(mul(p3, 3), mul(p2, 2)), add(p1, p0)), q0));
    let f8p1 = r3(add(add(add(mul(p3, 2), p2), add(mul(p1, 2), p0)), add(q0, q1)));
    let f8p0 = r3(add(add(add(p3, p2), add(p1, mul(p0, 2))), add(add(q0, q1), q2)));
    let f8q0 = r3(add(add(add(p2, p1), add(p0, mul(q0, 2))), add(add(q1, q2), q3)));
    let f8q1 = r3(add(add(add(p1, p0), add(q0, mul(q1, 2))), add(add(q2, q3), q3)));
    let f8q2 = r3(add(add(add(p0, q0), add(q1, mul(q2, 2))), add(add(q3, q3), q3)));

    if width < 16 {
        let use8 = v128_and(mask, flat);
        let use4 = v128_andnot(mask, flat);
        let out_p = [
            sel(use8, f8p0, sel(use4, f4p0, p0)),
            sel(use8, f8p1, sel(use4, f4p1, p1)),
            sel(use8, f8p2, p2),
            p3,
            p[4],
            p[5],
            p[6],
        ];
        let out_q = [
            sel(use8, f8q0, sel(use4, f4q0, q0)),
            sel(use8, f8q1, sel(use4, f4q1, q1)),
            sel(use8, f8q2, q2),
            q3,
            q[4],
            q[5],
            q[6],
        ];
        return (out_p, out_q);
    }

    // flat2 and the 15-tap filter
    let (p4, p5, p6, p7) = (p[4], p[5], p[6], p[7]);
    let (q4, q5, q6, q7) = (q[4], q[5], q[6], q[7]);
    let mut flat2 = v128_and(le(absd(p4, p0), ft), le(absd(q4, q0), ft));
    flat2 = v128_and(flat2, le(absd(p5, p0), ft));
    flat2 = v128_and(flat2, le(absd(q5, q0), ft));
    flat2 = v128_and(flat2, le(absd(p6, p0), ft));
    flat2 = v128_and(flat2, le(absd(q6, q0), ft));
    flat2 = v128_and(flat2, le(absd(p7, p0), ft));
    flat2 = v128_and(flat2, le(absd(q7, q0), ft));
    let r4 = |x: v128| u16x8_shr(u16x8_add(x, u16x8_splat(8)), 4);
    let s = |v: &[v128]| v.iter().copied().fold(u16x8_splat(0), |a, b| u16x8_add(a, b));
    let f16p6 = r4(s(&[mul(p7, 7), mul(p6, 2), p5, p4, p3, p2, p1, p0, q0]));
    let f16p5 = r4(s(&[mul(p7, 6), p6, mul(p5, 2), p4, p3, p2, p1, p0, q0, q1]));
    let f16p4 = r4(s(&[mul(p7, 5), p6, p5, mul(p4, 2), p3, p2, p1, p0, q0, q1, q2]));
    let f16p3 = r4(s(&[mul(p7, 4), p6, p5, p4, mul(p3, 2), p2, p1, p0, q0, q1, q2, q3]));
    let f16p2 = r4(s(&[mul(p7, 3), p6, p5, p4, p3, mul(p2, 2), p1, p0, q0, q1, q2, q3, q4]));
    let f16p1 = r4(s(&[mul(p7, 2), p6, p5, p4, p3, p2, mul(p1, 2), p0, q0, q1, q2, q3, q4, q5]));
    let f16p0 = r4(s(&[p7, p6, p5, p4, p3, p2, p1, mul(p0, 2), q0, q1, q2, q3, q4, q5, q6]));
    let f16q0 = r4(s(&[p6, p5, p4, p3, p2, p1, p0, mul(q0, 2), q1, q2, q3, q4, q5, q6, q7]));
    let f16q1 = r4(s(&[p5, p4, p3, p2, p1, p0, q0, mul(q1, 2), q2, q3, q4, q5, q6, mul(q7, 2)]));
    let f16q2 = r4(s(&[p4, p3, p2, p1, p0, q0, q1, mul(q2, 2), q3, q4, q5, q6, mul(q7, 3)]));
    let f16q3 = r4(s(&[p3, p2, p1, p0, q0, q1, q2, mul(q3, 2), q4, q5, q6, mul(q7, 4)]));
    let f16q4 = r4(s(&[p2, p1, p0, q0, q1, q2, q3, mul(q4, 2), q5, q6, mul(q7, 5)]));
    let f16q5 = r4(s(&[p1, p0, q0, q1, q2, q3, q4, mul(q5, 2), q6, mul(q7, 6)]));
    let f16q6 = r4(s(&[p0, q0, q1, q2, q3, q4, q5, mul(q6, 2), mul(q7, 7)]));

    let mf = v128_and(mask, flat);
    let use16 = v128_and(mf, flat2);
    let use8 = v128_andnot(mf, flat2);
    let use4 = v128_andnot(mask, flat);
    let out_p = [
        sel(use16, f16p0, sel(use8, f8p0, sel(use4, f4p0, p0))),
        sel(use16, f16p1, sel(use8, f8p1, sel(use4, f4p1, p1))),
        sel(use16, f16p2, sel(use8, f8p2, p2)),
        sel(use16, f16p3, p3),
        sel(use16, f16p4, p4),
        sel(use16, f16p5, p5),
        sel(use16, f16p6, p6),
    ];
    let out_q = [
        sel(use16, f16q0, sel(use8, f8q0, sel(use4, f4q0, q0))),
        sel(use16, f16q1, sel(use8, f8q1, sel(use4, f4q1, q1))),
        sel(use16, f16q2, sel(use8, f8q2, q2)),
        sel(use16, f16q3, q3),
        sel(use16, f16q4, q4),
        sel(use16, f16q5, q5),
        sel(use16, f16q6, q6),
    ];
    (out_p, out_q)
}

/// The vector twin of `loopfilter::filter_edge8`: filter the 8 positions `i + l * lane_stride` across an edge (samples
/// `pos_stride` apart) with the width-4/8/16 filter. The caller guarantees the whole window is inside `buf`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn filter_edge8(
    buf: &mut [u16],
    i: usize,
    pos_stride: usize,
    lane_stride: usize,
    width: usize,
    lim: i32,
    blimit: i32,
    hev_thr: i32,
    bd: i32,
) {
    let outer = if width >= 16 { 8 } else { 4 };
    let zero = u16x8_splat(0);
    let mut p = [zero; 8];
    let mut q = [zero; 8];
    if lane_stride == 1 {
        // A horizontal edge: lanes are consecutive samples of a row, taps are rows.
        for k in 0..outer {
            p[k] = ld8(&buf[i - (k + 1) * pos_stride..]);
            q[k] = ld8(&buf[i + k * pos_stride..]);
        }
        let (np, nq) = lf_core(p, q, width, lim, blimit, hev_thr, bd);
        let modified = if width >= 16 {
            7
        } else if width >= 8 {
            3
        } else {
            2
        };
        for k in 0..modified {
            st8(&mut buf[i - (k + 1) * pos_stride..], np[k]);
            st8(&mut buf[i + k * pos_stride..], nq[k]);
        }
    } else {
        // A vertical edge: lanes are rows, taps are columns: transpose 8-row tiles in and out.
        let left = if width >= 16 { 8 } else { 4 };
        let rows: [v128; 8] = core::array::from_fn(|l| ld8(&buf[i + l * lane_stride - left..]));
        let t = transpose8x8(rows);
        if width >= 16 {
            let rows2: [v128; 8] = core::array::from_fn(|l| ld8(&buf[i + l * lane_stride..]));
            let t2 = transpose8x8(rows2);
            // t holds p7..p0 as columns 0..8 (p7 first); t2 holds q0..q7.
            for k in 0..8 {
                p[k] = t[7 - k];
                q[k] = t2[k];
            }
            let (np, nq) = lf_core(p, q, width, lim, blimit, hev_thr, bd);
            // Back to rows: columns p7..p0 = [t0=p7 (unchanged), np6..np0]; q0..q7 = [nq0..nq6, q7].
            let left_cols = [t[0], np[6], np[5], np[4], np[3], np[2], np[1], np[0]];
            let right_cols = [nq[0], nq[1], nq[2], nq[3], nq[4], nq[5], nq[6], q[7]];
            let (lr, rr) = (transpose8x8(left_cols), transpose8x8(right_cols));
            for l in 0..8 {
                st8(&mut buf[i + l * lane_stride - 8..], lr[l]);
                st8(&mut buf[i + l * lane_stride..], rr[l]);
            }
        } else {
            // t holds p3..p0 q0..q3 as columns 0..8.
            for k in 0..4 {
                p[k] = t[3 - k];
                q[k] = t[4 + k];
            }
            let (np, nq) = lf_core(p, q, width, lim, blimit, hev_thr, bd);
            let cols = [np[3], np[2], np[1], np[0], nq[0], nq[1], nq[2], nq[3]];
            let r = transpose8x8(cols);
            for l in 0..8 {
                st8(&mut buf[i + l * lane_stride - 4..], r[l]);
            }
        }
    }
}

/// Compare the vector kernels with the scalar ones on pseudo-random data; returns the number of mismatches.
pub fn selftest() -> u32 {
    let mut seed = 0x1357_9BDFu32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut bad = 0u32;
    // Motion compensation: random planes at 8 and 10 bits.
    for &bd in &[8i32, 10] {
        let max = (1i32 << bd) - 1;
        let (pw, ph) = (96usize, 80usize);
        let plane: Vec<u16> = (0..pw * ph).map(|_| (rnd() as i32 & max) as u16).collect();
        let refp = RefPlane { buf: &plane, stride: pw, w: pw as i32, h: ph as i32 };
        for &(w, h) in &[(4usize, 4usize), (8, 8), (16, 16), (32, 32), (64, 64), (8, 4), (4, 8)] {
            for &filter in &[0usize, 1, 2] {
                for &(sx, sy) in &[(0usize, 0usize), (5, 0), (0, 9), (7, 3), (15, 15), (1, 14)] {
                    for &avg in &[false, true] {
                        let (bx, by) = (12 + (rnd() % 8) as i32, 10 + (rnd() % 8) as i32);
                        if bx + w as i32 + 4 > pw as i32 || by + h as i32 + 4 > ph as i32 {
                            continue;
                        }
                        let seed_dst: Vec<u16> = (0..w * h).map(|_| (rnd() as i32 & max) as u16).collect();
                        let mut a = seed_dst.clone();
                        let mut b = seed_dst;
                        // `predict_block` with the scalar branches only: call the reference through the unvectorised entry.
                        crate::inter::predict_block_scalar(
                            &refp, bx, by, sx, sy, filter, &mut a, w, w, h, avg, max,
                        );
                        let fx = &SUBPEL_FILTERS[filter][sx];
                        let fy = &SUBPEL_FILTERS[filter][sy];
                        // SAFETY: the window was checked to lie inside the plane.
                        unsafe {
                            predict_block_interior(
                                &refp,
                                bx,
                                by,
                                fx,
                                fy,
                                sx != 0,
                                sy != 0,
                                &mut b,
                                w,
                                w,
                                h,
                                max,
                                avg,
                            )
                        };
                        bad += (a != b) as u32;
                    }
                }
            }
        }
    }
    // Loop filter: both orientations, every width, flat and rough data.
    for &bd in &[8i32, 10] {
        let max = (1u32 << bd) - 1;
        for round in 0..400 {
            let stride = 40usize;
            let rough = 1 + rnd() % (if round % 3 == 0 { 3 } else { 40 });
            let base = rnd() % (max / 2) + (max / 4);
            let data: Vec<u16> = (0..stride * 40).map(|_| (base + rnd() % rough).min(max) as u16).collect();
            let lvl = 1 + (rnd() % 63) as i32;
            let shift = bd - 8;
            let lim = (1 + (lvl >> 2).min(8)) << shift;
            let blimit = (2 * (lvl + 2) + lim) << shift >> shift;
            let hev = (lvl >> 4) << shift;
            for &width in &[4usize, 8, 16] {
                for &(pos, lane, i) in
                    &[(1usize, stride, 12 + 10 * stride), (stride, 1usize, 8 + 14 * stride)]
                {
                    let mut a = data.clone();
                    let mut b = data.clone();
                    crate::loopfilter::filter_edge8_scalar(&mut a, i, pos, lane, width, lim, blimit, hev, bd);
                    filter_edge8(&mut b, i, pos, lane, width, lim, blimit, hev, bd);
                    bad += (a != b) as u32;
                }
            }
        }
    }
    bad
}
