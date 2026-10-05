//! Picture order count (8.2.1), for frames.
use crate::params::{SliceHeader, Sps};

/// POC state carried between pictures.
#[derive(Debug, Clone, Default)]
pub struct PocState {
    /// `prevPicOrderCntMsb` (type 0).
    pub prev_msb: i32,
    /// `prevPicOrderCntLsb` (type 0).
    pub prev_lsb: i32,
    /// `prevFrameNumOffset` (types 1, 2).
    pub prev_frame_num_offset: i32,
    /// `prevFrameNum` of the previous picture (types 1, 2).
    pub prev_frame_num: u32,
}

/// The result for one picture.
#[derive(Debug, Clone, Copy)]
pub struct PocResult {
    /// `TopFieldOrderCnt`.
    pub top: i32,
    /// `BottomFieldOrderCnt`.
    pub bottom: i32,
    /// `FrameNumOffset` (needed to update the state).
    pub frame_num_offset: i32,
    /// `PicOrderCnt` of the frame: the minimum of top and bottom.
    pub poc: i32,
}

impl PocState {
    /// Compute the POC of a picture with header `h` (`h.idr` and `h.nal_ref_idc` are used).
    pub fn compute(&self, sps: &Sps, h: &SliceHeader) -> PocResult {
        let max_frame_num = 1i64 << sps.log2_max_frame_num;
        match sps.poc_type {
            0 => {
                let (prev_msb, prev_lsb) = if h.idr { (0, 0) } else { (self.prev_msb, self.prev_lsb) };
                let max_lsb = 1i32 << sps.log2_max_poc_lsb;
                let lsb = h.pic_order_cnt_lsb as i32;
                let msb = if lsb < prev_lsb && prev_lsb - lsb >= max_lsb / 2 {
                    prev_msb.wrapping_add(max_lsb)
                } else if lsb > prev_lsb && lsb - prev_lsb > max_lsb / 2 {
                    prev_msb.wrapping_sub(max_lsb)
                } else {
                    prev_msb
                };
                let top = msb.wrapping_add(lsb);
                let bottom = top.wrapping_add(h.delta_pic_order_cnt_bottom);
                PocResult { top, bottom, frame_num_offset: 0, poc: top.min(bottom) }
            }
            1 | 2 => {
                let fno = if h.idr {
                    0
                } else if self.prev_frame_num > h.frame_num {
                    (self.prev_frame_num_offset as i64 + max_frame_num) as i32
                } else {
                    self.prev_frame_num_offset
                };
                let abs_frame_num0 = fno as i64 + h.frame_num as i64;
                if sps.poc_type == 1 {
                    let n = sps.offset_for_ref_frame.len() as i64;
                    let mut abs_frame_num = if n != 0 { abs_frame_num0 } else { 0 };
                    if h.nal_ref_idc == 0 && abs_frame_num > 0 {
                        abs_frame_num -= 1;
                    }
                    let mut expected: i64 = 0;
                    if abs_frame_num > 0 {
                        let cycle_cnt = (abs_frame_num - 1) / n;
                        let in_cycle = ((abs_frame_num - 1) % n) as usize;
                        let per_cycle: i64 = sps.offset_for_ref_frame.iter().map(|&v| v as i64).sum();
                        expected = cycle_cnt * per_cycle;
                        for i in 0..=in_cycle {
                            expected += sps.offset_for_ref_frame[i] as i64;
                        }
                    }
                    if h.nal_ref_idc == 0 {
                        expected += sps.offset_for_non_ref_pic as i64;
                    }
                    let top = (expected + h.delta_pic_order_cnt[0] as i64) as i32;
                    let bottom = (top as i64
                        + sps.offset_for_top_to_bottom_field as i64
                        + h.delta_pic_order_cnt[1] as i64) as i32;
                    PocResult { top, bottom, frame_num_offset: fno, poc: top.min(bottom) }
                } else {
                    let t = if h.idr {
                        0
                    } else if h.nal_ref_idc == 0 {
                        2 * abs_frame_num0 - 1
                    } else {
                        2 * abs_frame_num0
                    } as i32;
                    PocResult { top: t, bottom: t, frame_num_offset: fno, poc: t }
                }
            }
            _ => PocResult { top: 0, bottom: 0, frame_num_offset: 0, poc: 0 },
        }
    }
}
