//! The deblocking filter (8.7.2): boundary strengths on the 8x8 grid, then the luma and chroma edge filters, all vertical edges of the
//! picture before any horizontal one.
use super::frame::Frame;
use super::pic::*;
use crate::Result;
use crate::ps::{Pps, Sps};
use crate::tables::{BETA, TC, chroma_qp};
use alloc::vec;

/// Boundary strength of the edge between two blocks that are both inter (8.7.2.4).
fn motion_bs(p: &MvField, q: &MvField) -> u8 {
    let np = (p.ref_idx[0] >= 0) as u8 + (p.ref_idx[1] >= 0) as u8;
    let nq = (q.ref_idx[0] >= 0) as u8 + (q.ref_idx[1] >= 0) as u8;
    if np != nq {
        return 1;
    }
    let far = |a: super::frame::Mv, b: super::frame::Mv| {
        (a.x as i32 - b.x as i32).abs() >= 4 || (a.y as i32 - b.y as i32).abs() >= 4
    };
    if np == 1 {
        let lp = if p.ref_idx[0] >= 0 { 0 } else { 1 };
        let lq = if q.ref_idx[0] >= 0 { 0 } else { 1 };
        if p.ref_poc[lp] != q.ref_poc[lq] {
            return 1;
        }
        return far(p.mv[lp], q.mv[lq]) as u8;
    }
    // Two vectors each: the same pair of pictures?
    let (p0, p1, q0, q1) = (p.ref_poc[0], p.ref_poc[1], q.ref_poc[0], q.ref_poc[1]);
    if !((p0 == q0 && p1 == q1) || (p0 == q1 && p1 == q0)) {
        return 1;
    }
    if p0 != p1 {
        // Two different pictures: compare the vectors that point at the same one.
        if p0 == q0 {
            (far(p.mv[0], q.mv[0]) || far(p.mv[1], q.mv[1])) as u8
        } else {
            (far(p.mv[0], q.mv[1]) || far(p.mv[1], q.mv[0])) as u8
        }
    } else {
        // Both vectors of each block point at one picture: either pairing may match.
        ((far(p.mv[0], q.mv[0]) || far(p.mv[1], q.mv[1])) && (far(p.mv[0], q.mv[1]) || far(p.mv[1], q.mv[0])))
            as u8
    }
}

pub(super) fn run(st: &mut PicState, cur: &mut Frame, sps: &Sps, pps: &Pps) -> Result<()> {
    if st.slices.iter().all(|s| s.deblocking_disabled) {
        return Ok(());
    }
    let (w4, h4) = (st.w4, st.h4);
    let bd = sps.bit_depth_luma as i32;
    // Boundary strengths: bs[0] for vertical edges (on the left of a block), bs[1] for horizontal ones (on top).
    let mut bs = [vec![0u8; w4 * h4], vec![0u8; w4 * h4]];
    for dir in 0..2usize {
        for y4 in 0..h4 {
            for x4 in 0..w4 {
                let on_grid = if dir == 0 { x4 > 0 && x4 % 2 == 0 } else { y4 > 0 && y4 % 2 == 0 };
                if !on_grid {
                    continue;
                }
                let i = y4 * w4 + x4;
                let q = st.cells[i];
                let (edge_tu, edge_pu) = if dir == 0 { (F_TU_LEFT, F_PU_LEFT) } else { (F_TU_TOP, F_PU_TOP) };
                if q.flags & (edge_tu | edge_pu) == 0 {
                    continue;
                }
                let pi = if dir == 0 { i - 1 } else { i - w4 };
                let pcell = st.cells[pi];
                let (xq, yq) = ((x4 * 4) as i32, (y4 * 4) as i32);
                let (xp, yp) = if dir == 0 { (xq - 1, yq) } else { (xq, yq - 1) };
                // Edges at the border of the picture never get here; slices and tiles may switch the edge off.
                let (cq, cp) = (st.ctb_of(xq, yq), st.ctb_of(xp, yp));
                let sq = &st.slices[st.ctb_slice[cq] as usize];
                if sq.deblocking_disabled {
                    continue;
                }
                if st.ctb_slice_addr[cq] != st.ctb_slice_addr[cp] && !sq.loop_filter_across_slices {
                    continue;
                }
                if st.tile_id_rs[cq] != st.tile_id_rs[cp] && !pps.loop_filter_across_tiles_enabled {
                    continue;
                }
                let v = if q.mode == 1 || pcell.mode == 1 {
                    2
                } else if q.flags & edge_tu != 0 && (q.flags & F_CBF != 0 || pcell.flags & F_CBF != 0) {
                    1
                } else {
                    motion_bs(&st.mvf[pi], &st.mvf[i])
                };
                bs[dir][i] = v;
            }
        }
    }
    for dir in 0..2usize {
        // Luma.
        let max = (1i32 << bd) - 1;
        for y4 in 0..h4 {
            for x4 in 0..w4 {
                let i = y4 * w4 + x4;
                let strength = bs[dir][i] as i32;
                if strength == 0 {
                    continue;
                }
                let pi = if dir == 0 { i - 1 } else { i - w4 };
                let (q, p) = (st.cells[i], st.cells[pi]);
                let (xq, yq) = ((x4 * 4) as i32, (y4 * 4) as i32);
                let sl = &st.slices[st.ctb_slice[st.ctb_of(xq, yq)] as usize];
                let qpl = (q.qp as i32 + p.qp as i32 + 1) >> 1;
                let beta = BETA[(qpl + (sl.beta_offset_div2 as i32) * 2).clamp(0, 51) as usize] as i32
                    * (1 << (bd - 8));
                let tc = TC[(qpl + 2 * (strength - 1) + (sl.tc_offset_div2 as i32) * 2).clamp(0, 53) as usize]
                    as i32
                    * (1 << (bd - 8));
                let no_p = p.flags & (F_BYPASS | F_PCM_NOFILTER) != 0;
                let no_q = q.flags & (F_BYPASS | F_PCM_NOFILTER) != 0;
                let plane = &mut cur.planes[0];
                let stride = plane.stride as isize;
                // Step along the edge (`along`) and across it (`across`) in samples.
                let (along, across) = if dir == 0 { (stride, 1isize) } else { (1isize, stride) };
                let base = (yq as isize) * stride + xq as isize;
                filter_luma(&mut plane.data, base, along, across, beta, tc, no_p, no_q, max);
            }
        }
        // Chroma: edges on the 8-sample chroma grid with strength 2, in segments of four chroma lines.
        for c in 1..3usize {
            let max = (1i32 << sps.bit_depth_chroma) - 1;
            let cqp_offset = if c == 1 { pps.cb_qp_offset as i32 } else { pps.cr_qp_offset as i32 };
            for y4 in 0..h4 {
                for x4 in 0..w4 {
                    let i = y4 * w4 + x4;
                    // 8 chroma samples = 16 luma samples between edges; each chroma segment of 4 lines is 8 luma lines.
                    let (on_grid, seg_ok) = if dir == 0 {
                        (x4 % 4 == 0 && x4 > 0, y4 % 2 == 0)
                    } else {
                        (y4 % 4 == 0 && y4 > 0, x4 % 2 == 0)
                    };
                    if !on_grid || !seg_ok || bs[dir][i] != 2 {
                        continue;
                    }
                    let pi = if dir == 0 { i - 1 } else { i - w4 };
                    let (q, p) = (st.cells[i], st.cells[pi]);
                    let (xq, yq) = ((x4 * 4) as i32, (y4 * 4) as i32);
                    let sl = &st.slices[st.ctb_slice[st.ctb_of(xq, yq)] as usize];
                    let qpc = chroma_qp(((q.qp as i32 + p.qp as i32 + 1) >> 1) + cqp_offset);
                    let tc = TC[(qpc + 2 + (sl.tc_offset_div2 as i32) * 2).clamp(0, 53) as usize] as i32
                        * (1 << (sps.bit_depth_chroma - 8));
                    let no_p = p.flags & (F_BYPASS | F_PCM_NOFILTER) != 0;
                    let no_q = q.flags & (F_BYPASS | F_PCM_NOFILTER) != 0;
                    let plane = &mut cur.planes[c];
                    let stride = plane.stride as isize;
                    let (along, across) = if dir == 0 { (stride, 1isize) } else { (1isize, stride) };
                    let base = ((yq / 2) as isize) * stride + (xq / 2) as isize;
                    for k in 0..4isize {
                        let o = base + k * along;
                        let (p0, p1, q0, q1) = (
                            plane.data[(o - across) as usize] as i32,
                            plane.data[(o - 2 * across) as usize] as i32,
                            plane.data[o as usize] as i32,
                            plane.data[(o + across) as usize] as i32,
                        );
                        let d = ((((q0 - p0) << 2) + p1 - q1 + 4) >> 3).clamp(-tc, tc);
                        if !no_p {
                            plane.data[(o - across) as usize] = (p0 + d).clamp(0, max) as u16;
                        }
                        if !no_q {
                            plane.data[o as usize] = (q0 - d).clamp(0, max) as u16;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Filter one four-sample segment of a luma edge (8.7.2.5.3 and 8.7.2.5.7). `base` is the first sample on the Q side.
#[allow(clippy::too_many_arguments)]
fn filter_luma(
    d: &mut [u16],
    base: isize,
    along: isize,
    across: isize,
    beta: i32,
    tc: i32,
    no_p: bool,
    no_q: bool,
    max: i32,
) {
    let s = |d: &[u16], k: isize, i: isize| d[(base + k * along + i * across) as usize] as i32;
    // p_i is at -(i+1), q_i at +i.
    let dp = |d: &[u16], k| (s(d, k, -3) - 2 * s(d, k, -2) + s(d, k, -1)).abs();
    let dq = |d: &[u16], k| (s(d, k, 2) - 2 * s(d, k, 1) + s(d, k, 0)).abs();
    let (dp0, dp3, dq0, dq3) = (dp(d, 0), dp(d, 3), dq(d, 0), dq(d, 3));
    let (dpq0, dpq3) = (dp0 + dq0, dp3 + dq3);
    let (dpa, dqa) = (dp0 + dp3, dq0 + dq3);
    if dpq0 + dpq3 >= beta {
        return;
    }
    let strong_line = |d: &[u16], k: isize, dpq: i32| -> bool {
        2 * dpq < (beta >> 2)
            && (s(d, k, -4) - s(d, k, -1)).abs() + (s(d, k, 0) - s(d, k, 3)).abs() < (beta >> 3)
            && (s(d, k, -1) - s(d, k, 0)).abs() < ((5 * tc + 1) >> 1)
    };
    let strong = strong_line(d, 0, dpq0) && strong_line(d, 3, dpq3);
    let side_thr = (beta + (beta >> 1)) >> 3;
    let (de_p, de_q) = (dpa < side_thr, dqa < side_thr);
    for k in 0..4isize {
        let (p0, p1, p2, p3) = (s(d, k, -1), s(d, k, -2), s(d, k, -3), s(d, k, -4));
        let (q0, q1, q2, q3) = (s(d, k, 0), s(d, k, 1), s(d, k, 2), s(d, k, 3));
        let at = |i: isize| (base + k * along + i * across) as usize;
        if strong {
            if !no_p {
                d[at(-1)] =
                    ((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3).clamp(p0 - 2 * tc, p0 + 2 * tc) as u16;
                d[at(-2)] = ((p2 + p1 + p0 + q0 + 2) >> 2).clamp(p1 - 2 * tc, p1 + 2 * tc) as u16;
                d[at(-3)] =
                    ((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3).clamp(p2 - 2 * tc, p2 + 2 * tc) as u16;
            }
            if !no_q {
                d[at(0)] =
                    ((p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3).clamp(q0 - 2 * tc, q0 + 2 * tc) as u16;
                d[at(1)] = ((p0 + q0 + q1 + q2 + 2) >> 2).clamp(q1 - 2 * tc, q1 + 2 * tc) as u16;
                d[at(2)] = ((p0 + q0 + q1 + 3 * q2 + 2 * q3 + 4) >> 3).clamp(q2 - 2 * tc, q2 + 2 * tc) as u16;
            }
        } else {
            let mut delta = (9 * (q0 - p0) - 3 * (q1 - p1) + 8) >> 4;
            if delta.abs() >= tc * 10 {
                continue;
            }
            delta = delta.clamp(-tc, tc);
            if !no_p {
                d[at(-1)] = (p0 + delta).clamp(0, max) as u16;
                if de_p {
                    let dp = ((((p2 + p0 + 1) >> 1) - p1 + delta) >> 1).clamp(-(tc >> 1), tc >> 1);
                    d[at(-2)] = (p1 + dp).clamp(0, max) as u16;
                }
            }
            if !no_q {
                d[at(0)] = (q0 - delta).clamp(0, max) as u16;
                if de_q {
                    let dq = ((((q2 + q0 + 1) >> 1) - q1 - delta) >> 1).clamp(-(tc >> 1), tc >> 1);
                    d[at(1)] = (q1 + dq).clamp(0, max) as u16;
                }
            }
        }
    }
}
