//! WebAssembly SIMD128 deblocking of one edge segment (16 luma or 8 chroma lines at a time). The scalar functions in
//! `deblock` define the result; `selftest` compares the two inside WebAssembly.
#![cfg(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64"))]

use super::deblock::{filter_chroma_scalar, filter_luma_scalar};
use super::deblock_tables::TC0;
use alloc::vec::Vec;
use rvp_core::simd::*;

/// `|a - b|` per byte.
#[inline(always)]
fn absdiff(a: v128, b: v128) -> v128 {
    v128_or(u8x16_sub_sat(a, b), u8x16_sub_sat(b, a))
}

/// Widen the two halves of a byte vector to 16-bit lanes.
#[inline(always)]
fn halves(v: v128) -> [v128; 2] {
    [u16x8_extend_low_u8x16(v), u16x8_extend_high_u8x16(v)]
}

#[inline(always)]
fn clamp16(v: v128, lo: v128, hi: v128) -> v128 {
    i16x8_min(i16x8_max(v, lo), hi)
}

/// Saturating pack of two 16-bit halves to bytes (clips to 0..=255).
#[inline(always)]
fn pack(h: [v128; 2]) -> v128 {
    u8x16_narrow_i16x8(h[0], h[1])
}

/// The samples across one edge for 16 lines: `p3..p0` before, `q0..q3` after (`p3`/`q3` only matter for luma).
struct Lines {
    p: [v128; 4], // p0, p1, p2, p3
    q: [v128; 4], // q0, q1, q2, q3
}

/// Per-line parameters: the lanes of a group of lines share `bS` and `tC0`.
struct Params {
    alpha: v128,
    beta: v128,
    /// 0xFF where `bS > 0`.
    active: v128,
    /// `tC0` per line (bytes).
    tc0: v128,
}

fn params(bs: &[u8], per: usize, alpha: i32, beta: i32, index_a: usize) -> Params {
    let mut tc0 = [0u8; 16];
    let mut act = [0u8; 16];
    for (g, &b) in bs.iter().enumerate() {
        for l in 0..per {
            act[g * per + l] = if b > 0 { 0xFF } else { 0 };
            tc0[g * per + l] = if (1..4).contains(&b) { TC0[b as usize - 1][index_a] } else { 0 };
        }
    }
    Params {
        alpha: u8x16_splat(alpha as u8),
        beta: u8x16_splat(beta as u8),
        active: load(&act),
        tc0: load(&tc0),
    }
}

/// Filter luma lines in place. `strong` selects bS = 4 (all lines of an edge share it).
fn luma_core(l: &mut Lines, pr: &Params, strong: bool) {
    let [p0, p1, p2, p3] = l.p;
    let [q0, q1, q2, q3] = l.q;
    let filter = v128_and(
        v128_and(u8x16_lt(absdiff(p0, q0), pr.alpha), u8x16_lt(absdiff(p1, p0), pr.beta)),
        v128_and(u8x16_lt(absdiff(q1, q0), pr.beta), pr.active),
    );
    let ap_lt = u8x16_lt(absdiff(p2, p0), pr.beta);
    let aq_lt = u8x16_lt(absdiff(q2, q0), pr.beta);
    let (np0, np1, mut np2, nq0, nq1, mut nq2);
    np2 = p2;
    nq2 = q2;
    if !strong {
        // tc = tc0 + (ap < beta) + (aq < beta): a true mask is -1.
        let tc = u8x16_sub(u8x16_sub(pr.tc0, ap_lt), aq_lt);
        let (hp0, hp1, hp2, hq0, hq1, hq2) =
            (halves(p0), halves(p1), halves(p2), halves(q0), halves(q1), halves(q2));
        let (htc, htc0) = (halves(tc), halves(pr.tc0));
        let mut out = [[i16x8_splat(0); 2]; 4]; // p0, p1, q0, q1
        for k in 0..2 {
            let zero = i16x8_splat(0);
            let ntc = i16x8_sub(zero, htc[k]);
            let d = i16x8_shr(
                i16x8_add(
                    i16x8_add(i16x8_shl(i16x8_sub(hq0[k], hp0[k]), 2), i16x8_sub(hp1[k], hq1[k])),
                    i16x8_splat(4),
                ),
                3,
            );
            let delta = clamp16(d, ntc, htc[k]);
            out[0][k] = i16x8_add(hp0[k], delta);
            out[2][k] = i16x8_sub(hq0[k], delta);
            let avg = i16x8_shr(i16x8_add(i16x8_add(hp0[k], hq0[k]), i16x8_splat(1)), 1);
            let ntc0 = i16x8_sub(zero, htc0[k]);
            let dp =
                clamp16(i16x8_shr(i16x8_sub(i16x8_add(hp2[k], avg), i16x8_shl(hp1[k], 1)), 1), ntc0, htc0[k]);
            let dq =
                clamp16(i16x8_shr(i16x8_sub(i16x8_add(hq2[k], avg), i16x8_shl(hq1[k], 1)), 1), ntc0, htc0[k]);
            out[1][k] = i16x8_add(hp1[k], dp);
            out[3][k] = i16x8_add(hq1[k], dq);
        }
        np0 = v128_bitselect(pack(out[0]), p0, filter);
        nq0 = v128_bitselect(pack(out[2]), q0, filter);
        np1 = v128_bitselect(pack(out[1]), p1, v128_and(filter, ap_lt));
        nq1 = v128_bitselect(pack(out[3]), q1, v128_and(filter, aq_lt));
    } else {
        let small = u8x16_lt(absdiff(p0, q0), u8x16_add(u8x16_shr(pr.alpha, 2), u8x16_splat(2)));
        let (hp0, hp1, hp2, hp3) = (halves(p0), halves(p1), halves(p2), halves(p3));
        let (hq0, hq1, hq2, hq3) = (halves(q0), halves(q1), halves(q2), halves(q3));
        let c = |n: i16| i16x8_splat(n);
        let mut s = [[i16x8_splat(0); 2]; 8]; // strong p0,p1,p2, weak p0, strong q0,q1,q2, weak q0
        for k in 0..2 {
            // p side
            let sum_p = i16x8_add(i16x8_add(hp1[k], hp0[k]), i16x8_add(hq0[k], hp2[k])); // p2 + p1 + p0 + q0
            s[0][k] = i16x8_shr(
                i16x8_add(
                    i16x8_add(i16x8_add(sum_p, i16x8_add(hp1[k], hp0[k])), i16x8_add(hq0[k], hq1[k])),
                    c(4),
                ),
                3,
            ); // (p2 + 2p1 + 2p0 + 2q0 + q1 + 4) >> 3
            s[1][k] = i16x8_shr(i16x8_add(sum_p, c(2)), 2);
            s[2][k] = i16x8_shr(
                i16x8_add(
                    i16x8_add(i16x8_shl(hp3[k], 1), i16x8_mul(hp2[k], c(3))),
                    i16x8_add(i16x8_add(hp1[k], hp0[k]), i16x8_add(hq0[k], c(4))),
                ),
                3,
            ); // (2p3 + 3p2 + p1 + p0 + q0 + 4) >> 3
            s[3][k] =
                i16x8_shr(i16x8_add(i16x8_add(i16x8_shl(hp1[k], 1), i16x8_add(hp0[k], hq1[k])), c(2)), 2);
            // q side
            let sum_q = i16x8_add(i16x8_add(hq1[k], hq0[k]), i16x8_add(hp0[k], hq2[k])); // q2 + q1 + q0 + p0
            s[4][k] = i16x8_shr(
                i16x8_add(
                    i16x8_add(i16x8_add(sum_q, i16x8_add(hq1[k], hq0[k])), i16x8_add(hp0[k], hp1[k])),
                    c(4),
                ),
                3,
            ); // (q2 + 2q1 + 2q0 + 2p0 + p1 + 4) >> 3
            s[5][k] = i16x8_shr(i16x8_add(sum_q, c(2)), 2);
            s[6][k] = i16x8_shr(
                i16x8_add(
                    i16x8_add(i16x8_shl(hq3[k], 1), i16x8_mul(hq2[k], c(3))),
                    i16x8_add(i16x8_add(hq1[k], hq0[k]), i16x8_add(hp0[k], c(4))),
                ),
                3,
            );
            s[7][k] =
                i16x8_shr(i16x8_add(i16x8_add(i16x8_shl(hq1[k], 1), i16x8_add(hq0[k], hp1[k])), c(2)), 2);
        }
        let sp = v128_and(v128_and(filter, ap_lt), small);
        let sq = v128_and(v128_and(filter, aq_lt), small);
        // p0: strong where `sp`, else the weak form, wherever the filter applies.
        np0 = v128_bitselect(pack(s[0]), v128_bitselect(pack(s[3]), p0, filter), sp);
        np1 = v128_bitselect(pack(s[1]), p1, sp);
        np2 = v128_bitselect(pack(s[2]), p2, sp);
        nq0 = v128_bitselect(pack(s[4]), v128_bitselect(pack(s[7]), q0, filter), sq);
        nq1 = v128_bitselect(pack(s[5]), q1, sq);
        nq2 = v128_bitselect(pack(s[6]), q2, sq);
    }
    l.p = [np0, np1, np2, p3];
    l.q = [nq0, nq1, nq2, q3];
}

/// Chroma: only `p1, p0, q0, q1` are involved.
fn chroma_core(l: &mut Lines, pr: &Params, strong: bool) {
    let [p0, p1, ..] = l.p;
    let [q0, q1, ..] = l.q;
    let filter = v128_and(
        v128_and(u8x16_lt(absdiff(p0, q0), pr.alpha), u8x16_lt(absdiff(p1, p0), pr.beta)),
        v128_and(u8x16_lt(absdiff(q1, q0), pr.beta), pr.active),
    );
    let (hp0, hp1, hq0, hq1) = (halves(p0), halves(p1), halves(q0), halves(q1));
    let mut o = [[i16x8_splat(0); 2]; 2];
    for k in 0..2 {
        if strong {
            o[0][k] = i16x8_shr(
                i16x8_add(i16x8_add(i16x8_shl(hp1[k], 1), hp0[k]), i16x8_add(hq1[k], i16x8_splat(2))),
                2,
            );
            o[1][k] = i16x8_shr(
                i16x8_add(i16x8_add(i16x8_shl(hq1[k], 1), hq0[k]), i16x8_add(hp1[k], i16x8_splat(2))),
                2,
            );
        } else {
            let tc = halves(u8x16_add(pr.tc0, u8x16_splat(1)))[k];
            let d = i16x8_shr(
                i16x8_add(
                    i16x8_add(i16x8_shl(i16x8_sub(hq0[k], hp0[k]), 2), i16x8_sub(hp1[k], hq1[k])),
                    i16x8_splat(4),
                ),
                3,
            );
            let delta = clamp16(d, i16x8_sub(i16x8_splat(0), tc), tc);
            o[0][k] = i16x8_add(hp0[k], delta);
            o[1][k] = i16x8_sub(hq0[k], delta);
        }
    }
    l.p[0] = v128_bitselect(pack(o[0]), p0, filter);
    l.q[0] = v128_bitselect(pack(o[1]), q0, filter);
}

/// Transpose 8 rows of 8 bytes (each in the low half of a vector) into 8 vectors holding columns of 8 bytes in the low
/// half and, for the second tile, the high half.
#[inline(always)]
fn transpose8(r: [v128; 8]) -> [v128; 4] {
    // Columns (2c, 2c+1) of the 8 rows end up in `w[c]`: bytes 0..8 are column 2c, bytes 8..16 column 2c+1.
    let b = |x: v128, y: v128| i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(x, y);
    let (t0, t1, t2, t3) = (b(r[0], r[1]), b(r[2], r[3]), b(r[4], r[5]), b(r[6], r[7]));
    let lo16 =
        |x: v128, y: v128| i8x16_shuffle::<0, 1, 16, 17, 2, 3, 18, 19, 4, 5, 20, 21, 6, 7, 22, 23>(x, y);
    let hi16 = |x: v128, y: v128| {
        i8x16_shuffle::<8, 9, 24, 25, 10, 11, 26, 27, 12, 13, 28, 29, 14, 15, 30, 31>(x, y)
    };
    let (u0, u1, u2, u3) = (lo16(t0, t1), hi16(t0, t1), lo16(t2, t3), hi16(t2, t3));
    let lo32 =
        |x: v128, y: v128| i8x16_shuffle::<0, 1, 2, 3, 16, 17, 18, 19, 4, 5, 6, 7, 20, 21, 22, 23>(x, y);
    let hi32 = |x: v128, y: v128| {
        i8x16_shuffle::<8, 9, 10, 11, 24, 25, 26, 27, 12, 13, 14, 15, 28, 29, 30, 31>(x, y)
    };
    [lo32(u0, u2), hi32(u0, u2), lo32(u1, u3), hi32(u1, u3)]
}

/// Columns 0..8 of 16 lines (rows `o + k * line` of `p`, 8 bytes each starting at `o`), as 8 vectors of 16 lanes.
fn load_cols(p: &[u8], o: usize, line: usize) -> [v128; 8] {
    let rows = |base: usize| -> [v128; 8] { core::array::from_fn(|k| load8(&p[o + (base + k) * line..])) };
    let (a, b) = (transpose8(rows(0)), transpose8(rows(8)));
    let mut out = [i8x16_splat(0); 8];
    for c in 0..4 {
        out[2 * c] = i8x16_shuffle::<0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 20, 21, 22, 23>(a[c], b[c]);
        out[2 * c + 1] =
            i8x16_shuffle::<8, 9, 10, 11, 12, 13, 14, 15, 24, 25, 26, 27, 28, 29, 30, 31>(a[c], b[c]);
    }
    out
}

/// Inverse of [`load_cols`] for the columns `first..first + n` (n <= 8): write them back to the 16 lines.
fn store_cols(p: &mut [u8], o: usize, line: usize, cols: &[v128; 8], first: usize, n: usize) {
    for (base, hi) in [(0usize, false), (8usize, true)] {
        // The low half of each vector holds this tile's 8 lines of one column; transposing yields the lines.
        let c: [v128; 8] = core::array::from_fn(|k| {
            if hi {
                i8x16_shuffle::<8, 9, 10, 11, 12, 13, 14, 15, 0, 0, 0, 0, 0, 0, 0, 0>(cols[k], cols[k])
            } else {
                cols[k]
            }
        });
        let w = transpose8(c);
        for (j, v) in w.iter().enumerate() {
            let mut buf = [0u8; 16];
            store(&mut buf, *v);
            for half in 0..2 {
                let at = o + (base + 2 * j + half) * line + first;
                p[at..at + n].copy_from_slice(&buf[half * 8 + first..half * 8 + first + n]);
            }
        }
    }
}

/// `filter_luma` for one edge of 16 lines in 4 groups of 4.
#[allow(clippy::too_many_arguments)]
pub fn luma(
    p: &mut [u8],
    q0: usize,
    step: usize,
    line: usize,
    bs: &[u8],
    alpha: i32,
    beta: i32,
    index_a: usize,
) {
    if bs.iter().all(|&b| b == 0) {
        return;
    }
    let strong = bs[0] == 4;
    debug_assert!(bs.iter().all(|&b| (b == 4) == strong));
    let pr = params(bs, 4, alpha, beta, index_a);
    if step != 1 {
        // Horizontal edge: lines are columns, samples across the edge are `step` apart.
        let at = |k: isize| (q0 as isize + k * step as isize) as usize;
        let mut l = Lines {
            p: [load(&p[at(-1)..]), load(&p[at(-2)..]), load(&p[at(-3)..]), load(&p[at(-4)..])],
            q: [load(&p[at(0)..]), load(&p[at(1)..]), load(&p[at(2)..]), load(&p[at(3)..])],
        };
        luma_core(&mut l, &pr, strong);
        store(&mut p[at(-3)..], l.p[2]);
        store(&mut p[at(-2)..], l.p[1]);
        store(&mut p[at(-1)..], l.p[0]);
        store(&mut p[at(0)..], l.q[0]);
        store(&mut p[at(1)..], l.q[1]);
        store(&mut p[at(2)..], l.q[2]);
    } else {
        // Vertical edge: 16 rows of [p3 p2 p1 p0 q0 q1 q2 q3].
        let cols = load_cols(p, q0 - 4, line);
        let mut l =
            Lines { p: [cols[3], cols[2], cols[1], cols[0]], q: [cols[4], cols[5], cols[6], cols[7]] };
        luma_core(&mut l, &pr, strong);
        let out = [cols[0], l.p[2], l.p[1], l.p[0], l.q[0], l.q[1], l.q[2], cols[7]];
        store_cols(p, q0 - 4, line, &out, 1, 6);
    }
}

/// `filter_chroma` for one edge of 8 lines in 4 groups of 2.
#[allow(clippy::too_many_arguments)]
pub fn chroma(
    p: &mut [u8],
    q0: usize,
    step: usize,
    line: usize,
    bs: &[u8],
    alpha: i32,
    beta: i32,
    index_a: usize,
) {
    if bs.iter().all(|&b| b == 0) {
        return;
    }
    let strong = bs[0] == 4;
    let pr = params(bs, 2, alpha, beta, index_a);
    if step != 1 {
        let at = |k: isize| (q0 as isize + k * step as isize) as usize;
        let mut l = Lines {
            p: [load8(&p[at(-1)..]), load8(&p[at(-2)..]), i8x16_splat(0), i8x16_splat(0)],
            q: [load8(&p[at(0)..]), load8(&p[at(1)..]), i8x16_splat(0), i8x16_splat(0)],
        };
        chroma_core(&mut l, &pr, strong);
        store8(&mut p[at(-1)..], l.p[0]);
        store8(&mut p[at(0)..], l.q[0]);
    } else {
        // 8 rows of [p3 p2 p1 p0 q0 q1 q2 q3]; the transpose gives the columns in the low halves.
        let rows: [v128; 8] = core::array::from_fn(|k| load8(&p[q0 - 4 + k * line..]));
        let t = transpose8(rows);
        let col = |c: usize| {
            if c % 2 == 0 {
                t[c / 2]
            } else {
                i8x16_shuffle::<8, 9, 10, 11, 12, 13, 14, 15, 0, 0, 0, 0, 0, 0, 0, 0>(t[c / 2], t[c / 2])
            }
        };
        let mut l = Lines { p: [col(3), col(2), col(1), col(0)], q: [col(4), col(5), col(6), col(7)] };
        chroma_core(&mut l, &pr, strong);
        // Write back p0 and q0 (columns 3 and 4) of each row.
        let (mut b3, mut b4) = ([0u8; 16], [0u8; 16]);
        store(&mut b3, l.p[0]);
        store(&mut b4, l.q[0]);
        for k in 0..8 {
            p[q0 - 1 + k * line] = b3[k];
            p[q0 + k * line] = b4[k];
        }
    }
}

/// Compare the vector edge filters with the scalar ones on pseudo-random sample patches; returns the mismatches.
pub fn selftest() -> u32 {
    use super::deblock_tables::{ALPHA, BETA};
    let mut seed = 0x0BAD_CAFEu32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let stride = 24usize;
    let (mut bad, mut changed) = (0u32, 0u32);
    for round in 0..600 {
        let index_a = (rnd() % 52) as usize;
        let (alpha, beta) = (
            ALPHA[index_a] as i32,
            BETA[((index_a as i32 + (rnd() % 5) as i32 - 2).clamp(0, 51)) as usize] as i32,
        );
        let amp = 1 + rnd() % 40; // how rough the samples are
        let base = 40 + rnd() % 150;
        let plane: Vec<u8> = (0..stride * 40)
            .map(|i| {
                let step = if round % 3 == 0 { 0 } else { (i / stride) as u32 / 4 * 3 };
                (base + step + rnd() % amp).min(255) as u8
            })
            .collect();
        let strong = round % 4 == 0;
        for per in [4usize, 2] {
            let bs: Vec<u8> = (0..4).map(|_| if strong { 4 } else { (rnd() % 4) as u8 }).collect();
            for &(vertical, q0) in &[(true, 8 + 4 * stride), (false, 8 + 8 * stride)] {
                let (step, line) = if vertical { (1, stride) } else { (stride, 1) };
                let mut a = plane.clone();
                let mut b = plane.clone();
                if per == 4 {
                    filter_luma_scalar(&mut a, q0, step, line, &bs, 4, alpha, beta, index_a);
                    luma(&mut b, q0, step, line, &bs, alpha, beta, index_a);
                } else {
                    filter_chroma_scalar(&mut a, q0, step, line, &bs, 2, alpha, beta, index_a);
                    chroma(&mut b, q0, step, line, &bs, alpha, beta, index_a);
                }
                bad += (a != b) as u32;
                changed += (a != plane) as u32;
            }
        }
    }
    // The patches must actually exercise the filters, or agreement would prove nothing.
    if changed < 600 { bad + 100_000 } else { bad }
}
