//! The in-loop deblocking filter (8.7), for progressive frames.
use super::deblock_tables::{ALPHA, BETA, TC0};
use super::mbinfo::{F_INTRA, F_T8X8, F_UNIFORM, MbInfo};
use super::picture::Picture;

/// Per-slice deblocking parameters.
#[derive(Clone, Copy, Debug, Default)]
pub struct SliceFilter {
    /// `disable_deblocking_filter_idc`.
    pub disable_idc: u8,
    /// `FilterOffsetA` (`slice_alpha_c0_offset_div2 * 2`).
    pub offset_a: i8,
    /// `FilterOffsetB` (`slice_beta_offset_div2 * 2`).
    pub offset_b: i8,
}

#[inline]
fn clip3(lo: i32, hi: i32, v: i32) -> i32 {
    v.clamp(lo, hi)
}

/// Filter one luma edge (see [`filter_luma_scalar`], which defines the result). The vector version handles the usual 4 groups
/// of 4 lines.
#[allow(clippy::too_many_arguments)]
fn filter_luma(
    p: &mut [u8],
    q0: usize,
    step: usize,
    line: usize,
    bs: &[u8],
    per: usize,
    alpha: i32,
    beta: i32,
    index_a: usize,
) {
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    if per == 4 && bs.len() == 4 {
        return super::deblock_simd::luma(p, q0, step, line, bs, alpha, beta, index_a);
    }
    filter_luma_scalar(p, q0, step, line, bs, per, alpha, beta, index_a)
}

/// Filter one chroma edge (see [`filter_chroma_scalar`]).
#[allow(clippy::too_many_arguments)]
fn filter_chroma(
    p: &mut [u8],
    q0: usize,
    step: usize,
    line: usize,
    bs: &[u8],
    per: usize,
    alpha: i32,
    beta: i32,
    index_a: usize,
) {
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    if per == 2 && bs.len() == 4 {
        return super::deblock_simd::chroma(p, q0, step, line, bs, alpha, beta, index_a);
    }
    filter_chroma_scalar(p, q0, step, line, bs, per, alpha, beta, index_a)
}

/// Filter `lines` lines across one edge. `q0` is the index of the first q0 sample, `step` the distance between
/// consecutive samples across the edge (1 for a vertical edge, the stride for a horizontal one), `line` the
/// distance between lines along the edge. `bs` is per group of `lines / bs.len()` lines.
#[allow(clippy::too_many_arguments)]
pub(super) fn filter_luma_scalar(
    p: &mut [u8],
    q0: usize,
    step: usize,
    line: usize,
    bs: &[u8],
    per: usize,
    alpha: i32,
    beta: i32,
    index_a: usize,
) {
    for (g, &b) in bs.iter().enumerate() {
        if b == 0 {
            continue;
        }
        let tc0 = if b < 4 { TC0[b as usize - 1][index_a] as i32 } else { 0 };
        for l in 0..per {
            let o = q0 + (g * per + l) * line;
            let (p0, p1, p2) = (p[o - step] as i32, p[o - 2 * step] as i32, p[o - 3 * step] as i32);
            let (q0v, q1, q2) = (p[o] as i32, p[o + step] as i32, p[o + 2 * step] as i32);
            if (p0 - q0v).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0v).abs() >= beta {
                continue;
            }
            let ap = (p2 - p0).abs();
            let aq = (q2 - q0v).abs();
            if b < 4 {
                let tc = tc0 + (ap < beta) as i32 + (aq < beta) as i32;
                let delta = clip3(-tc, tc, (((q0v - p0) << 2) + (p1 - q1) + 4) >> 3);
                p[o - step] = clip3(0, 255, p0 + delta) as u8;
                p[o] = clip3(0, 255, q0v - delta) as u8;
                if ap < beta {
                    p[o - 2 * step] =
                        (p1 + clip3(-tc0, tc0, (p2 + ((p0 + q0v + 1) >> 1) - (p1 << 1)) >> 1)) as u8;
                }
                if aq < beta {
                    p[o + step] =
                        (q1 + clip3(-tc0, tc0, (q2 + ((p0 + q0v + 1) >> 1) - (q1 << 1)) >> 1)) as u8;
                }
            } else {
                let small = (p0 - q0v).abs() < ((alpha >> 2) + 2);
                if ap < beta && small {
                    let p3 = p[o - 4 * step] as i32;
                    p[o - step] = ((p2 + 2 * p1 + 2 * p0 + 2 * q0v + q1 + 4) >> 3) as u8;
                    p[o - 2 * step] = ((p2 + p1 + p0 + q0v + 2) >> 2) as u8;
                    p[o - 3 * step] = ((2 * p3 + 3 * p2 + p1 + p0 + q0v + 4) >> 3) as u8;
                } else {
                    p[o - step] = ((2 * p1 + p0 + q1 + 2) >> 2) as u8;
                }
                if aq < beta && small {
                    let q3 = p[o + 3 * step] as i32;
                    p[o] = ((p1 + 2 * p0 + 2 * q0v + 2 * q1 + q2 + 4) >> 3) as u8;
                    p[o + step] = ((p0 + q0v + q1 + q2 + 2) >> 2) as u8;
                    p[o + 2 * step] = ((2 * q3 + 3 * q2 + q1 + q0v + p0 + 4) >> 3) as u8;
                } else {
                    p[o] = ((2 * q1 + q0v + p1 + 2) >> 2) as u8;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn filter_chroma_scalar(
    p: &mut [u8],
    q0: usize,
    step: usize,
    line: usize,
    bs: &[u8],
    per: usize,
    alpha: i32,
    beta: i32,
    index_a: usize,
) {
    for (g, &b) in bs.iter().enumerate() {
        if b == 0 {
            continue;
        }
        let tc0 = if b < 4 { TC0[b as usize - 1][index_a] as i32 } else { 0 };
        for l in 0..per {
            let o = q0 + (g * per + l) * line;
            let (p0, p1) = (p[o - step] as i32, p[o - 2 * step] as i32);
            let (q0v, q1) = (p[o] as i32, p[o + step] as i32);
            if (p0 - q0v).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0v).abs() >= beta {
                continue;
            }
            if b < 4 {
                let tc = tc0 + 1;
                let delta = clip3(-tc, tc, (((q0v - p0) << 2) + (p1 - q1) + 4) >> 3);
                p[o - step] = clip3(0, 255, p0 + delta) as u8;
                p[o] = clip3(0, 255, q0v - delta) as u8;
            } else {
                p[o - step] = ((2 * p1 + p0 + q1 + 2) >> 2) as u8;
                p[o] = ((2 * q1 + q0v + p1 + 2) >> 2) as u8;
            }
        }
    }
}

/// Boundary strength between two blocks of the same or adjacent macroblocks.
struct Bs<'a> {
    pic: &'a Picture,
    mbs: &'a [MbInfo],
    w4: usize,
    w8: usize,
}

impl Bs<'_> {
    /// `(ref ids, mvs)` of the 4x4 block at grid position `(x4, y4)`.
    #[inline]
    fn motion(&self, x4: usize, y4: usize) -> ([i32; 2], [[i16; 2]; 2]) {
        let i8 = (y4 >> 1) * self.w8 + (x4 >> 1);
        let i4 = y4 * self.w4 + x4;
        let mut refs = [-1i32; 2];
        for l in 0..2 {
            if self.pic.ref_idx[l][i8] >= 0 {
                refs[l] = self.pic.ref_id[l][i8];
            }
        }
        (refs, [self.pic.mv[0][i4], self.pic.mv[1][i4]])
    }

    /// bS 0 or 1 from motion (both blocks inter coded, no coefficients).
    fn motion_bs(&self, px: usize, py: usize, qx: usize, qy: usize) -> u8 {
        let (pr, pm) = self.motion(px, py);
        let (qr, qm) = self.motion(qx, qy);
        let diff = |a: [i16; 2], b: [i16; 2]| {
            (a[0] as i32 - b[0] as i32).abs() >= 4 || (a[1] as i32 - b[1] as i32).abs() >= 4
        };
        let np = (pr[0] >= 0) as u8 + (pr[1] >= 0) as u8;
        let nq = (qr[0] >= 0) as u8 + (qr[1] >= 0) as u8;
        if np != nq {
            return 1;
        }
        if np == 1 {
            let (lp, lq) = (if pr[0] >= 0 { 0 } else { 1 }, if qr[0] >= 0 { 0 } else { 1 });
            return (pr[lp] != qr[lq] || diff(pm[lp], qm[lq])) as u8;
        }
        if np == 0 {
            return 0;
        }
        // Two motion vectors each.
        if !((pr[0] == qr[0] && pr[1] == qr[1]) || (pr[0] == qr[1] && pr[1] == qr[0])) {
            return 1;
        }
        if pr[0] != pr[1] {
            if pr[0] == qr[0] {
                (diff(pm[0], qm[0]) || diff(pm[1], qm[1])) as u8
            } else {
                (diff(pm[0], qm[1]) || diff(pm[1], qm[0])) as u8
            }
        } else {
            ((diff(pm[0], qm[0]) || diff(pm[1], qm[1])) && (diff(pm[0], qm[1]) || diff(pm[1], qm[0]))) as u8
        }
    }

    /// Boundary strength for the edge between block `(px, py)` of MB `pa` and block `(qx, qy)` of MB `qa`
    /// (grid coordinates in 4x4 units), `mb_edge` for edges on a macroblock boundary.
    fn bs(&self, pa: usize, qa: usize, px: usize, py: usize, qx: usize, qy: usize, mb_edge: bool) -> u8 {
        let (p, q) = (&self.mbs[pa], &self.mbs[qa]);
        if (p.flags | q.flags) & F_INTRA != 0 {
            return if mb_edge { 4 } else { 3 };
        }
        let nzbit = |m: &MbInfo, x4: usize, y4: usize| m.nzmask & (1 << ((y4 & 3) * 4 + (x4 & 3))) != 0;
        if nzbit(p, px, py) || nzbit(q, qx, qy) {
            return 2;
        }
        self.motion_bs(px, py, qx, qy)
    }
}

/// Filter a whole decoded picture in macroblock order.
pub(crate) fn deblock_picture(pic: &mut Picture, mbs: &[MbInfo], slices: &[SliceFilter]) {
    if slices.iter().all(|s| s.disable_idc == 1) {
        return;
    }
    let (mbw, mbh) = (pic.mbw, pic.mbh);
    let (w4, w8) = (mbw * 4, mbw * 2);
    let mut planes = core::mem::take(&mut pic.planes);
    let strides = pic.strides;
    {
        let b = Bs { pic, mbs, w4, w8 };
        for addr in 0..mbw * mbh {
            let q = &mbs[addr];
            if q.slice == 0 {
                continue;
            }
            let sf = slices[q.slice as usize - 1];
            if sf.disable_idc == 1 {
                continue;
            }
            let (mx, my) = (addr % mbw, addr / mbw);
            let left = (mx > 0
                && mbs[addr - 1].slice != 0
                && (sf.disable_idc != 2 || mbs[addr - 1].slice == q.slice))
                .then(|| addr - 1);
            let top = (my > 0
                && mbs[addr - mbw].slice != 0
                && (sf.disable_idc != 2 || mbs[addr - mbw].slice == q.slice))
                .then(|| addr - mbw);
            let t8 = q.flags & F_T8X8 != 0;
            // Boundary strengths: bs_v[edge][row block], bs_h[edge][column block].
            let mut bs_v = [[0u8; 4]; 4];
            let mut bs_h = [[0u8; 4]; 4];
            let q_intra = q.flags & F_INTRA != 0;
            // Inter macroblocks with one motion and no coefficients need no internal edge checks.
            let q_flat = q.flags & F_UNIFORM != 0 && q.nzmask == 0;
            for e in 1..4 {
                if t8 && e % 2 == 1 {
                    continue;
                }
                if q_intra {
                    bs_v[e] = [3; 4];
                    bs_h[e] = [3; 4];
                } else if !q_flat {
                    for k in 0..4 {
                        bs_v[e][k] =
                            b.bs(addr, addr, mx * 4 + e - 1, my * 4 + k, mx * 4 + e, my * 4 + k, false);
                        bs_h[e][k] =
                            b.bs(addr, addr, mx * 4 + k, my * 4 + e - 1, mx * 4 + k, my * 4 + e, false);
                    }
                }
            }
            // Macroblock edges.
            for (dir, nb) in [(0usize, left), (1usize, top)] {
                let Some(pa) = nb else { continue };
                let p = &mbs[pa];
                let out = if dir == 0 { &mut bs_v[0] } else { &mut bs_h[0] };
                if (p.flags | q.flags) & F_INTRA != 0 {
                    *out = [4; 4];
                    continue;
                }
                let both_flat = p.flags & q.flags & F_UNIFORM != 0;
                let at = |k: usize| if dir == 0 { (mx * 4, my * 4 + k) } else { (mx * 4 + k, my * 4) };
                let pos_p = |(qx, qy): (usize, usize)| if dir == 0 { (qx - 1, qy) } else { (qx, qy - 1) };
                let mut shared = None;
                for k in 0..4 {
                    let (qx, qy) = at(k);
                    let (px, py) = pos_p((qx, qy));
                    let nzbit =
                        |m: &MbInfo, x4: usize, y4: usize| m.nzmask & (1 << ((y4 & 3) * 4 + (x4 & 3))) != 0;
                    out[k] = if nzbit(p, px, py) || nzbit(q, qx, qy) {
                        2
                    } else if both_flat {
                        *shared.get_or_insert_with(|| b.motion_bs(px, py, qx, qy))
                    } else {
                        b.motion_bs(px, py, qx, qy)
                    };
                }
            }
            // Luma.
            let stride = strides[0];
            let base = my * 16 * stride + mx * 16;
            let qp_of = |m: &MbInfo| m.qp as i32;
            for e in 0..4 {
                let pq = if e == 0 { left.map(|a| qp_of(&mbs[a])) } else { Some(qp_of(q)) };
                if let Some(pqp) = pq {
                    if bs_v[e].iter().any(|&v| v != 0) {
                        let qpav = (pqp + qp_of(q) + 1) >> 1;
                        let ia = clip3(0, 51, qpav + sf.offset_a as i32) as usize;
                        let ib = clip3(0, 51, qpav + sf.offset_b as i32) as usize;
                        filter_luma(
                            &mut planes[0],
                            base + e * 4,
                            1,
                            stride,
                            &bs_v[e],
                            4,
                            ALPHA[ia] as i32,
                            BETA[ib] as i32,
                            ia,
                        );
                    }
                }
            }
            for e in 0..4 {
                let pq = if e == 0 { top.map(|a| qp_of(&mbs[a])) } else { Some(qp_of(q)) };
                if let Some(pqp) = pq {
                    if bs_h[e].iter().any(|&v| v != 0) {
                        let qpav = (pqp + qp_of(q) + 1) >> 1;
                        let ia = clip3(0, 51, qpav + sf.offset_a as i32) as usize;
                        let ib = clip3(0, 51, qpav + sf.offset_b as i32) as usize;
                        // Horizontal edge: lines run along x, samples across the edge are `stride` apart.
                        filter_luma(
                            &mut planes[0],
                            base + e * 4 * stride,
                            stride,
                            1,
                            &bs_h[e],
                            4,
                            ALPHA[ia] as i32,
                            BETA[ib] as i32,
                            ia,
                        );
                    }
                }
            }
            // Chroma: edges 0 and 2 of the luma grid map to chroma edges 0 and 4.
            for c in 0..2 {
                let stride = strides[1 + c];
                let base = my * 8 * stride + mx * 8;
                for e in [0usize, 2] {
                    let pq = if e == 0 { left.map(|a| mbs[a].qpc[c] as i32) } else { Some(q.qpc[c] as i32) };
                    if let Some(pqp) = pq {
                        if bs_v[e].iter().any(|&v| v != 0) {
                            let qpav = (pqp + q.qpc[c] as i32 + 1) >> 1;
                            let ia = clip3(0, 51, qpav + sf.offset_a as i32) as usize;
                            let ib = clip3(0, 51, qpav + sf.offset_b as i32) as usize;
                            // 8 chroma rows, bS per 2 rows.
                            filter_chroma(
                                &mut planes[1 + c],
                                base + e * 2,
                                1,
                                stride,
                                &bs_v[e],
                                2,
                                ALPHA[ia] as i32,
                                BETA[ib] as i32,
                                ia,
                            );
                        }
                    }
                }
                for e in [0usize, 2] {
                    let pq = if e == 0 { top.map(|a| mbs[a].qpc[c] as i32) } else { Some(q.qpc[c] as i32) };
                    if let Some(pqp) = pq {
                        if bs_h[e].iter().any(|&v| v != 0) {
                            let qpav = (pqp + q.qpc[c] as i32 + 1) >> 1;
                            let ia = clip3(0, 51, qpav + sf.offset_a as i32) as usize;
                            let ib = clip3(0, 51, qpav + sf.offset_b as i32) as usize;
                            filter_chroma(
                                &mut planes[1 + c],
                                base + e * 2 * stride,
                                stride,
                                1,
                                &bs_h[e],
                                2,
                                ALPHA[ia] as i32,
                                BETA[ib] as i32,
                                ia,
                            );
                        }
                    }
                }
            }
        }
    }
    pic.planes = planes;
}
