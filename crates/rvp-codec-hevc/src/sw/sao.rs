//! Sample adaptive offset (8.7.3), applied to the deblocked picture.
use super::frame::Frame;
use super::pic::*;
use crate::Result;
use crate::ps::{Pps, Sps};

const H_POS: [[i32; 2]; 4] = [[-1, 1], [0, 0], [-1, 1], [1, -1]];
const V_POS: [[i32; 2]; 4] = [[0, 0], [-1, 1], [-1, 1], [-1, 1]];

pub(super) fn run(st: &mut PicState, cur: &mut Frame, sps: &Sps, pps: &Pps) -> Result<()> {
    if !st.slices.iter().any(|s| s.sao_luma || s.sao_chroma) {
        return Ok(());
    }
    // The deblocked samples are what every SAO decision reads; the result goes to the picture.
    let src: [alloc::vec::Vec<u16>; 3] =
        [cur.planes[0].data.clone(), cur.planes[1].data.clone(), cur.planes[2].data.clone()];
    let ctb = 1usize << st.ctb_log2;
    for ry in 0..st.h_ctb {
        for rx in 0..st.w_ctb {
            let rs = ry * st.w_ctb + rx;
            let p = st.sao[rs];
            for c in 0..3usize {
                if p.type_idx[c] == 0 {
                    continue;
                }
                let sub = (c > 0) as usize;
                let bd = if c == 0 { sps.bit_depth_luma } else { sps.bit_depth_chroma } as i32;
                let max = (1i32 << bd) - 1;
                let (cw, ch) = (ctb >> sub, ctb >> sub);
                let (x0, y0) = (rx * cw, ry * ch);
                let plane = &mut cur.planes[c];
                let (pw, ph) = (plane.width, plane.height);
                let stride = plane.stride;
                let (x1, y1) = ((x0 + cw).min(pw), (y0 + ch).min(ph));
                let offsets = p.offset[c];
                if p.type_idx[c] == 1 {
                    let shift = bd - 5;
                    let mut table = [0i32; 32];
                    for k in 0..4 {
                        table[(k + p.band_pos[c] as usize) & 31] = offsets[k] as i32;
                    }
                    for y in y0..y1 {
                        for x in x0..x1 {
                            if skip_sample(st, x << sub, y << sub) {
                                continue;
                            }
                            let v = src[c][y * stride + x] as i32;
                            plane.data[y * stride + x] =
                                (v + table[(v >> shift) as usize]).clamp(0, max) as u16;
                        }
                    }
                } else {
                    let class = p.eo_class[c] as usize;
                    let cur_slice = st.ctb_slice[rs] as usize;
                    for y in y0..y1 {
                        for x in x0..x1 {
                            if skip_sample(st, x << sub, y << sub) {
                                continue;
                            }
                            let v = src[c][y * stride + x] as i32;
                            let mut edge = 2i32;
                            let mut ok = true;
                            for k in 0..2 {
                                let (xn, yn) = (x as i32 + H_POS[class][k], y as i32 + V_POS[class][k]);
                                if xn < 0 || yn < 0 || xn >= pw as i32 || yn >= ph as i32 {
                                    ok = false;
                                    break;
                                }
                                // Across a CTB border the neighbour may be in another slice or tile.
                                if (xn as usize) < x0
                                    || (xn as usize) >= x0 + cw
                                    || (yn as usize) < y0
                                    || (yn as usize) >= y0 + ch
                                {
                                    let (xl, yl) = ((xn << sub), (yn << sub));
                                    let rn = st.ctb_of(xl, yl);
                                    if st.ctb_slice_addr[rn] != st.ctb_slice_addr[rs] {
                                        let cur_z = st.zs[((y << sub) >> st.log2_min_tb) * st.w_tb
                                            + ((x << sub) >> st.log2_min_tb)];
                                        let nb_z = st.zs[(yl as usize >> st.log2_min_tb) * st.w_tb
                                            + (xl as usize >> st.log2_min_tb)];
                                        let nb_slice = st.ctb_slice[rn] as usize;
                                        if (cur_z < nb_z && !st.slices[nb_slice].loop_filter_across_slices)
                                            || (nb_z < cur_z
                                                && !st.slices[cur_slice].loop_filter_across_slices)
                                        {
                                            ok = false;
                                            break;
                                        }
                                    }
                                    if !pps.loop_filter_across_tiles_enabled
                                        && st.tile_id_rs[rn] != st.tile_id_rs[rs]
                                    {
                                        ok = false;
                                        break;
                                    }
                                }
                                let nv = src[c][yn as usize * stride + xn as usize] as i32;
                                edge += (v - nv).signum();
                            }
                            if !ok {
                                continue;
                            }
                            let idx = match edge {
                                0 => 1,
                                1 => 2,
                                2 => 0,
                                e => e as usize,
                            };
                            if idx != 0 {
                                plane.data[y * stride + x] =
                                    (v + offsets[idx - 1] as i32).clamp(0, max) as u16;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Samples of coding units coded without loop filtering are left as they are.
fn skip_sample(st: &PicState, xl: usize, yl: usize) -> bool {
    st.cells[(yl >> 2) * st.w4 + (xl >> 2)].flags & (F_BYPASS | F_PCM_NOFILTER) != 0
}
