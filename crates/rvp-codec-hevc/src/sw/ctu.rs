//! Slice segment data: the coding tree, coding units, prediction and transform units, and the reconstruction that follows each.
use super::frame::{ColMotion, Frame, Mv};
use super::inter;
use super::intra::{self, Refs};
use super::mv::PartMode;
use super::pic::*;
use super::{invalid, transform};
use crate::cabac::{Cabac, Contexts};
use crate::ctx;
use crate::ps::{Pps, ScalingList, Sps};
use crate::slice::{SliceHeader, SliceType};
use crate::stream::Picture;
use crate::tables::{LEVEL_SCALE, SCAN, chroma_qp};
use crate::{Error, Result};
use alloc::vec;
use alloc::vec::Vec;

/// Context state kept between slice segments (dependent slices) and between CTB rows (WPP).
pub(super) type SavedState = Contexts;

/// `ScalingFactor` for one size and matrix (row-major).
type Factors = Vec<u8>;

/// The decoder of one slice segment.
pub(super) struct Dec<'a> {
    pub sps: &'a Sps,
    pub pps: &'a Pps,
    pub hdr: &'a SliceHeader,
    pub ref_lists: &'a [Vec<usize>; 2],
    pub refs: &'a [&'a Frame],
    pub pic: &'a mut PicState,
    pub cur: &'a mut Frame,
    pub cabac: Cabac<'a>,
    pub ctx: Contexts,
    rbsp: &'a [u8],
    removed: &'a [usize],
    slice_idx: usize,
    bit_depth: u32,
    slice_qp: i32,
    /// `QpY` of the previous coding unit in decoding order (`qPY_PREV`).
    qp_prev: i32,
    qp_y: i32,
    qg_pred: i32,
    is_cu_qp_delta_coded: bool,
    cu_qp_delta_val: i32,
    log2_min_cu_qp_delta: u32,
    pub cu_transquant_bypass: bool,
    cu_intra: bool,
    chroma_mode: u32,
    /// Scratch for coefficients and residuals (32x32).
    pub coef: Vec<i32>,
    resid: Vec<i32>,
    pred: Vec<i32>,
    scaling: Option<[Vec<Factors>; 4]>,
    x_ctb: usize,
    y_ctb: usize,
    /// POC of each entry of the two reference picture lists, and whether it is a long-term picture.
    pub ref_poc: [Vec<i32>; 2],
    pub ref_lt: [Vec<bool>; 2],
    /// Index into `refs` of the collocated picture.
    pub col_frame: Option<usize>,
    pub no_backward_pred: bool,
    pub cur_poc: i32,
    cu_part: PartMode,
    cu_depth: u32,
    pred_a: Vec<i32>,
    pred_b: Vec<i32>,
}

impl<'a> Dec<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sps: &'a Sps,
        pps: &'a Pps,
        hdr: &'a SliceHeader,
        ref_lists: &'a [Vec<usize>; 2],
        pic: &Picture<'_, super::Surface>,
        refs: &'a [&'a Frame],
        st: &'a mut PicState,
        cur: &'a mut Frame,
        rbsp: &'a [u8],
        removed: &'a [usize],
        slice_idx: usize,
    ) -> Self {
        let slice_qp = 26 + pps.init_qp_minus26 as i32 + hdr.qp_delta as i32;
        let scaling = sps.scaling_list_enabled.then(|| {
            let lists = pps.scaling_list.as_ref().or(sps.scaling_list.as_ref());
            scaling_factors(lists.unwrap_or(&ScalingList::default_lists()))
        });
        let cabac = Cabac::new(rbsp, hdr.data_offset_rbsp);
        let mut ref_poc: [Vec<i32>; 2] = [Vec::new(), Vec::new()];
        let mut ref_lt: [Vec<bool>; 2] = [Vec::new(), Vec::new()];
        for l in 0..2 {
            for &i in &ref_lists[l] {
                ref_poc[l].push(pic.refs[i].poc);
                ref_lt[l].push(pic.refs[i].long_term);
            }
        }
        let col_frame = if hdr.temporal_mvp && hdr.slice_type != SliceType::I {
            let l = if hdr.slice_type == SliceType::B && !hdr.collocated_from_l0 { 1 } else { 0 };
            ref_lists[l].get(hdr.collocated_ref_idx as usize).copied()
        } else {
            None
        };
        let no_backward_pred = ref_poc.iter().flatten().all(|p| *p <= pic.poc);
        Self {
            sps,
            pps,
            hdr,
            ref_lists,
            refs,
            pic: st,
            cur,
            cabac,
            ctx: Contexts::default(),
            rbsp,
            removed,
            slice_idx,
            bit_depth: sps.bit_depth_luma as u32,
            slice_qp,
            qp_prev: slice_qp,
            qp_y: slice_qp,
            qg_pred: slice_qp,
            is_cu_qp_delta_coded: false,
            cu_qp_delta_val: 0,
            log2_min_cu_qp_delta: sps.log2_ctb as u32 - pps.diff_cu_qp_delta_depth as u32,
            cu_transquant_bypass: false,
            cu_intra: false,
            chroma_mode: 1,
            coef: vec![0; 32 * 32],
            resid: vec![0; 32 * 32],
            pred: vec![0; 32 * 32],
            scaling,
            x_ctb: 0,
            y_ctb: 0,
            ref_poc,
            ref_lt,
            col_frame,
            no_backward_pred,
            cur_poc: pic.poc,
            cu_part: PartMode::P2Nx2N,
            cu_depth: 0,
            pred_a: vec![0; 64 * 64],
            pred_b: vec![0; 64 * 64],
        }
    }

    fn init_type(&self) -> usize {
        match self.hdr.slice_type {
            SliceType::I => 0,
            SliceType::P => 1 + self.hdr.cabac_init as usize,
            SliceType::B => 2 - self.hdr.cabac_init as usize,
        }
    }

    fn init_contexts(&mut self) {
        let t = self.init_type();
        self.ctx.init(t, self.slice_qp);
    }

    /// The bytes of the slice data where sub-stream `i` starts (the entry points count escaped bytes).
    fn substream_start(&self, i: usize) -> Option<usize> {
        let mut escaped = self.hdr.data_offset;
        for e in self.hdr.entry_points.iter().take(i) {
            escaped += *e as usize;
        }
        // Take away the emulation prevention bytes in front of that position.
        let removed_before = self.removed.iter().filter(|p| **p < escaped).count();
        let pos = escaped.checked_sub(removed_before)?;
        (pos <= self.rbsp.len()).then_some(pos)
    }

    /// Decode the whole slice segment.
    pub fn run(&mut self, saved_ds: &mut Option<SavedState>) -> Result<()> {
        let p: &'a Pps = self.pps;
        let w_ctb = self.pic.w_ctb;
        let n_ctb = w_ctb * self.pic.h_ctb;
        let mut ts = self.pic.rs_to_ts[self.hdr.segment_address as usize] as usize;
        if !self.hdr.dependent {
            self.pic.cur_slice_addr = self.hdr.segment_address;
        }
        let slice_addr = self.pic.cur_slice_addr;
        let wpp = p.entropy_coding_sync_enabled;
        let tiles = p.tiles_enabled;
        let mut substream = 0usize;
        let first_ts = ts;
        loop {
            let rs = self.pic.ts_to_rs[ts] as usize;
            let (xc, yc) = (rs % w_ctb, rs / w_ctb);
            self.x_ctb = xc;
            self.y_ctb = yc;
            self.pic.ctb_slice_addr[rs] = slice_addr;
            self.pic.ctb_slice[rs] = self.slice_idx as u16;
            let tile_start =
                ts == 0 || self.pic.tile_id_rs[rs] != self.pic.tile_id_rs[self.pic.ts_to_rs[ts - 1] as usize];
            let tile_col0 = self.tile_col0(rs);
            let row_start = wpp && xc == tile_col0;
            // Contexts at the start of a tile, of a WPP row and of a segment (9.3.1).
            if tile_start {
                self.init_contexts();
            } else if row_start {
                let (x0, y0) = ((xc << self.pic.ctb_log2) as i32, (yc << self.pic.ctb_log2) as i32);
                let size = 1i32 << self.pic.ctb_log2;
                if self.pic.available(x0, y0, x0 + size, y0 - size) {
                    match &self.pic.wpp_state {
                        Some(s) => self.ctx = s.clone(),
                        None => self.init_contexts(),
                    }
                } else {
                    self.init_contexts();
                }
            } else if ts == first_ts {
                match (self.hdr.dependent, saved_ds.as_ref()) {
                    (true, Some(s)) => self.ctx = s.clone(),
                    _ => self.init_contexts(),
                }
            }
            if tile_start || row_start || ts == first_ts && !self.hdr.dependent {
                self.qp_prev = self.slice_qp;
            }
            self.coding_tree_unit(xc, yc)?;
            let end = self.cabac.terminate() == 1;
            // The contexts after the second CTB of a row are what the row below starts from (WPP).
            if wpp && xc == tile_col0 + 1 {
                self.pic.wpp_state = Some(self.ctx.clone());
            }
            ts += 1;
            if end {
                if p.dependent_slice_segments_enabled {
                    *saved_ds = Some(self.ctx.clone());
                }
                break;
            }
            if ts >= n_ctb {
                return invalid("slice data runs past the picture");
            }
            let nrs = self.pic.ts_to_rs[ts] as usize;
            let next_tile = tiles && self.pic.tile_id_rs[nrs] != self.pic.tile_id_rs[rs];
            let next_row = wpp && (nrs % w_ctb) == self.tile_col0(nrs);
            if next_tile || next_row {
                // end_of_subset_one_bit, then a new sub-stream from its entry point.
                if self.cabac.terminate() != 1 {
                    return invalid("end_of_subset_one_bit is not 1");
                }
                substream += 1;
                let start = self
                    .substream_start(substream)
                    .ok_or(Error::Invalid("entry point outside the slice data"))?;
                self.cabac.restart(start);
            }
            if self.cabac.overrun() {
                return Err(Error::Truncated);
            }
        }
        Ok(())
    }

    /// The first column of the tile that contains the CTB `rs`.
    fn tile_col0(&self, rs: usize) -> usize {
        let t = self.pic.tile_id_rs[rs] as usize % (self.pic.col_bd.len() - 1);
        self.pic.col_bd[t] as usize
    }

    // ---- the coding tree -------------------------------------------------------------------------------------------------

    fn coding_tree_unit(&mut self, xc: usize, yc: usize) -> Result<()> {
        let l = self.pic.ctb_log2;
        let (x0, y0) = ((xc << l) as i32, (yc << l) as i32);
        if self.hdr.sao_luma || self.hdr.sao_chroma {
            self.sao_params(xc, yc)?;
        } else {
            self.pic.sao[yc * self.pic.w_ctb + xc] = SaoCtb::default();
        }
        self.coding_quadtree(x0, y0, l, 0)
    }

    fn sao_params(&mut self, xc: usize, yc: usize) -> Result<()> {
        let rs = yc * self.pic.w_ctb + xc;
        let mut merge_left = false;
        let mut merge_up = false;
        if xc > 0 {
            let (x0, y0) = ((xc << self.pic.ctb_log2) as i32, (yc << self.pic.ctb_log2) as i32);
            if self.pic.available(x0, y0, x0 - 1, y0) {
                merge_left = self.cabac.decision(&mut self.ctx, ctx::SAO_MERGE) == 1;
            }
        }
        if yc > 0 && !merge_left {
            let (x0, y0) = ((xc << self.pic.ctb_log2) as i32, (yc << self.pic.ctb_log2) as i32);
            if self.pic.available(x0, y0, x0, y0 - 1) {
                merge_up = self.cabac.decision(&mut self.ctx, ctx::SAO_MERGE) == 1;
            }
        }
        if merge_left {
            self.pic.sao[rs] = self.pic.sao[rs - 1];
            return Ok(());
        }
        if merge_up {
            self.pic.sao[rs] = self.pic.sao[rs - self.pic.w_ctb];
            return Ok(());
        }
        let mut s = SaoCtb::default();
        let bit_depth = self.bit_depth;
        let max_off = (1u32 << (bit_depth.min(10) - 5)) - 1;
        for c in 0..3 {
            if (c == 0 && !self.hdr.sao_luma) || (c > 0 && !self.hdr.sao_chroma) {
                continue;
            }
            if c == 2 {
                s.type_idx[2] = s.type_idx[1];
                s.eo_class[2] = s.eo_class[1];
            } else {
                s.type_idx[c] = if self.cabac.decision(&mut self.ctx, ctx::SAO_TYPE) == 0 {
                    0
                } else if self.cabac.bypass() == 0 {
                    1
                } else {
                    2
                };
            }
            if s.type_idx[c] == 0 {
                continue;
            }
            let mut abs = [0i32; 4];
            for a in abs.iter_mut() {
                // TR with cMax = max_off, all bypass.
                let mut v = 0;
                while v < max_off && self.cabac.bypass() == 1 {
                    v += 1;
                }
                *a = v as i32;
            }
            if s.type_idx[c] == 1 {
                let mut signs = [false; 4];
                for (i, sg) in signs.iter_mut().enumerate() {
                    if abs[i] != 0 {
                        *sg = self.cabac.bypass() == 1;
                    }
                }
                s.band_pos[c] = self.cabac.bypass_bits(5) as u8;
                for i in 0..4 {
                    s.offset[c][i] = (if signs[i] { -abs[i] } else { abs[i] }) as i16;
                }
            } else {
                if c < 2 {
                    s.eo_class[c] = self.cabac.bypass_bits(2) as u8;
                }
                for i in 0..4 {
                    s.offset[c][i] = (if i < 2 { abs[i] } else { -abs[i] }) as i16;
                }
            }
        }
        self.pic.sao[rs] = s;
        Ok(())
    }

    pub(super) fn cell_mode(&self, x: i32, y: i32) -> u8 {
        self.cell(x, y).mode
    }

    fn cell(&self, x: i32, y: i32) -> Cell {
        self.pic.cells[(y as usize >> 2) * self.pic.w4 + (x as usize >> 2)]
    }

    fn coding_quadtree(&mut self, x0: i32, y0: i32, log2: u32, depth: u32) -> Result<()> {
        let size = 1i32 << log2;
        let min_cb = self.sps.log2_min_cb as u32;
        let fits = x0 + size <= self.pic.width as i32 && y0 + size <= self.pic.height as i32;
        let split = if fits && log2 > min_cb {
            let mut inc = 0;
            if self.pic.available(x0, y0, x0 - 1, y0) && self.cell(x0 - 1, y0).depth as u32 > depth {
                inc += 1;
            }
            if self.pic.available(x0, y0, x0, y0 - 1) && self.cell(x0, y0 - 1).depth as u32 > depth {
                inc += 1;
            }
            self.cabac.decision(&mut self.ctx, ctx::SPLIT_CU + inc) == 1
        } else {
            log2 > min_cb
        };
        if self.pps.cu_qp_delta_enabled && log2 >= self.log2_min_cu_qp_delta {
            self.is_cu_qp_delta_coded = false;
            self.cu_qp_delta_val = 0;
            self.start_quant_group(x0, y0);
        }
        if split {
            let h = size >> 1;
            let (x1, y1) = (x0 + h, y0 + h);
            self.coding_quadtree(x0, y0, log2 - 1, depth + 1)?;
            if x1 < self.pic.width as i32 {
                self.coding_quadtree(x1, y0, log2 - 1, depth + 1)?;
            }
            if y1 < self.pic.height as i32 {
                self.coding_quadtree(x0, y1, log2 - 1, depth + 1)?;
            }
            if x1 < self.pic.width as i32 && y1 < self.pic.height as i32 {
                self.coding_quadtree(x1, y1, log2 - 1, depth + 1)?;
            }
            Ok(())
        } else {
            self.coding_unit(x0, y0, log2, depth)
        }
    }

    /// 8.6.1: `qPY_PRED` for the quantisation group that starts at (`xq`, `yq`).
    fn start_quant_group(&mut self, xq: i32, yq: i32) {
        let ctb_mask = !((1i32 << self.pic.ctb_log2) - 1);
        let prev = self.qp_prev;
        let a = if self.pic.available(xq, yq, xq - 1, yq)
            && (xq - 1) & ctb_mask == xq & ctb_mask
            && yq & ctb_mask == yq & ctb_mask
        {
            self.cell(xq - 1, yq).qp as i32
        } else {
            prev
        };
        let b = if self.pic.available(xq, yq, xq, yq - 1) && (yq - 1) & ctb_mask == yq & ctb_mask {
            self.cell(xq, yq - 1).qp as i32
        } else {
            prev
        };
        self.qg_pred = (a + b + 1) >> 1;
        self.qp_y = self.qg_pred;
    }

    fn set_cells(&mut self, x0: i32, y0: i32, size: i32, f: impl Fn(&mut Cell)) {
        let (xs, ys) = (x0 as usize >> 2, y0 as usize >> 2);
        let n = (size as usize) >> 2;
        for y in ys..(ys + n).min(self.pic.h4) {
            for x in xs..(xs + n).min(self.pic.w4) {
                f(&mut self.pic.cells[y * self.pic.w4 + x]);
            }
        }
    }

    fn coding_unit(&mut self, x0: i32, y0: i32, log2: u32, depth: u32) -> Result<()> {
        let size = 1i32 << log2;
        self.cu_transquant_bypass = self.pps.transquant_bypass_enabled
            && self.cabac.decision(&mut self.ctx, ctx::TRANSQUANT_BYPASS) == 1;
        if !self.pps.cu_qp_delta_enabled {
            self.qp_y = self.slice_qp;
        }
        let bypass = self.cu_transquant_bypass;
        let mut skip = false;
        if self.hdr.slice_type != SliceType::I {
            let mut inc = 0;
            if self.pic.available(x0, y0, x0 - 1, y0) && self.cell(x0 - 1, y0).skip {
                inc += 1;
            }
            if self.pic.available(x0, y0, x0, y0 - 1) && self.cell(x0, y0 - 1).skip {
                inc += 1;
            }
            skip = self.cabac.decision(&mut self.ctx, ctx::SKIP + inc) == 1;
        }
        // The area is "decoded" from here: neighbouring decisions inside it see the new depth and mode.
        self.set_cells(x0, y0, size, |c| {
            c.depth = depth as u8;
            c.skip = skip;
            c.flags = if bypass { F_BYPASS } else { 0 };
        });
        self.cu_depth = depth;
        self.mark_edges(x0, y0, size, size, true);
        self.mark_edges(x0, y0, size, size, false);
        let cb = (x0, y0, size);
        if skip {
            self.cu_intra = false;
            self.cu_part = PartMode::P2Nx2N;
            self.set_cells(x0, y0, size, |c| c.mode = 2);
            self.prediction_unit(cb, (x0, y0, size, size), 0, PartMode::P2Nx2N, true)?;
            self.finish_cu_qp(x0, y0, size);
            return Ok(());
        }
        let intra = if self.hdr.slice_type != SliceType::I {
            self.cabac.decision(&mut self.ctx, ctx::PRED_MODE) == 1
        } else {
            true
        };
        self.cu_intra = intra;
        let min_cb = self.sps.log2_min_cb as u32;
        let mut nxn = false;
        let mut part = PartMode::P2Nx2N;
        if intra {
            if log2 == min_cb {
                nxn = self.cabac.decision(&mut self.ctx, ctx::PART_MODE) == 0;
            }
            if nxn {
                part = PartMode::PNxN;
            }
        } else {
            part = self.parse_part_mode_inter(log2, min_cb);
        }
        self.cu_part = part;
        self.set_cells(x0, y0, size, |c| c.mode = if intra { 1 } else { 2 });
        if !intra {
            return self.inter_cu(cb, part);
        }
        let mut pcm = false;
        if !nxn
            && self.sps.pcm_enabled
            && log2 >= self.sps.log2_min_pcm_cb as u32
            && log2 <= self.sps.log2_max_pcm_cb as u32
        {
            pcm = self.cabac.terminate() == 1;
        }
        if pcm {
            self.pcm_samples(x0, y0, log2)?;
            self.set_cells(x0, y0, size, |c| c.intra_mode = 1);
            if self.sps.pcm_loop_filter_disabled {
                self.set_cells(x0, y0, size, |c| c.flags |= F_PCM_NOFILTER);
            }
            self.finish_cu_qp(x0, y0, size);
            return Ok(());
        }
        // Intra prediction modes.
        let parts = if nxn { 4 } else { 1 };
        let psize = if nxn { size >> 1 } else { size };
        let mut prev_flag = [false; 4];
        for f in prev_flag.iter_mut().take(parts) {
            *f = self.cabac.decision(&mut self.ctx, ctx::PREV_INTRA_LUMA) == 1;
        }
        let mut modes = [0u32; 4];
        for i in 0..parts {
            let (px, py) = (x0 + (i as i32 & 1) * psize, y0 + (i as i32 >> 1) * psize);
            let mpm_idx;
            let rem;
            if prev_flag[i] {
                let mut v = 0;
                if self.cabac.bypass() == 1 {
                    v = 1;
                    if self.cabac.bypass() == 1 {
                        v = 2;
                    }
                }
                mpm_idx = v;
                rem = 0;
            } else {
                mpm_idx = 0;
                rem = self.cabac.bypass_bits(5);
            }
            let cand = self.mpm_candidates(px, py);
            let mode = if prev_flag[i] {
                cand[mpm_idx as usize]
            } else {
                let mut c = cand;
                c.sort_unstable();
                let mut m = rem;
                for cm in c {
                    if m >= cm {
                        m += 1;
                    }
                }
                m
            };
            modes[i] = mode;
            self.set_cells(px, py, psize, |c| c.intra_mode = mode as u8);
        }
        // intra_chroma_pred_mode
        let chroma_idc = if self.cabac.decision(&mut self.ctx, ctx::INTRA_CHROMA) == 0 {
            4
        } else {
            self.cabac.bypass_bits(2)
        };
        self.chroma_mode = match chroma_idc {
            4 => modes[0],
            i => {
                let m = [0, 26, 10, 1][i as usize];
                if m == modes[0] { 34 } else { m }
            }
        };
        // The transform tree (rqt_root_cbf is implied for intra).
        let max_depth = self.sps.max_th_depth_intra as u32 + nxn as u32;
        self.transform_tree(x0, y0, x0, y0, log2, 0, 0, max_depth, nxn, [true, true])?;
        self.finish_cu_qp(x0, y0, size);
        Ok(())
    }

    /// `part_mode` of an inter coding unit (9.3.3.7).
    fn parse_part_mode_inter(&mut self, log2: u32, min_cb: u32) -> PartMode {
        let ctxs = ctx::PART_MODE;
        if self.cabac.decision(&mut self.ctx, ctxs) == 1 {
            return PartMode::P2Nx2N;
        }
        let bin1 = self.cabac.decision(&mut self.ctx, ctxs + 1) == 1;
        if log2 == min_cb {
            if log2 == 3 {
                return if bin1 { PartMode::P2NxN } else { PartMode::PNx2N };
            }
            if bin1 {
                return PartMode::P2NxN;
            }
            return if self.cabac.decision(&mut self.ctx, ctxs + 2) == 1 {
                PartMode::PNx2N
            } else {
                PartMode::PNxN
            };
        }
        if !self.sps.amp_enabled {
            return if bin1 { PartMode::P2NxN } else { PartMode::PNx2N };
        }
        let bin2 = self.cabac.decision(&mut self.ctx, ctxs + 3) == 1;
        if bin1 {
            if bin2 {
                PartMode::P2NxN
            } else if self.cabac.bypass() == 0 {
                PartMode::P2NxnU
            } else {
                PartMode::P2NxnD
            }
        } else if bin2 {
            PartMode::PNx2N
        } else if self.cabac.bypass() == 0 {
            PartMode::PnLx2N
        } else {
            PartMode::PnRx2N
        }
    }

    /// The rest of an inter coding unit: its prediction units, `rqt_root_cbf` and the transform tree.
    fn inter_cu(&mut self, cb: (i32, i32, i32), part: PartMode) -> Result<()> {
        let (x0, y0, s) = cb;
        let (h2, q) = (s / 2, s / 4);
        let parts: &[(i32, i32, i32, i32)] = match part {
            PartMode::P2Nx2N => &[(0, 0, s, s)],
            PartMode::P2NxN => &[(0, 0, s, h2), (0, h2, s, h2)],
            PartMode::PNx2N => &[(0, 0, h2, s), (h2, 0, h2, s)],
            PartMode::PNxN => &[(0, 0, h2, h2), (h2, 0, h2, h2), (0, h2, h2, h2), (h2, h2, h2, h2)],
            PartMode::P2NxnU => &[(0, 0, s, q), (0, q, s, s - q)],
            PartMode::P2NxnD => &[(0, 0, s, s - q), (0, s - q, s, q)],
            PartMode::PnLx2N => &[(0, 0, q, s), (q, 0, s - q, s)],
            PartMode::PnRx2N => &[(0, 0, s - q, s), (s - q, 0, q, s)],
        };
        let mut merge_first = false;
        for (i, &(dx, dy, w, h)) in parts.iter().enumerate() {
            let m = self.prediction_unit(cb, (x0 + dx, y0 + dy, w, h), i, part, false)?;
            if i == 0 {
                merge_first = m;
            }
        }
        let rqt_root_cbf = if !(part == PartMode::P2Nx2N && merge_first) {
            self.cabac.decision(&mut self.ctx, ctx::RQT_ROOT_CBF) == 1
        } else {
            true
        };
        if rqt_root_cbf {
            let log2 = s.trailing_zeros();
            self.transform_tree(
                x0,
                y0,
                x0,
                y0,
                log2,
                0,
                0,
                self.sps.max_th_depth_inter as u32,
                false,
                [true, true],
            )?;
        }
        self.finish_cu_qp(x0, y0, s);
        Ok(())
    }

    /// One prediction unit: parse its motion (or the merge index), derive the vectors and predict its samples. Returns `merge_flag`.
    fn prediction_unit(
        &mut self,
        cb: (i32, i32, i32),
        pb: (i32, i32, i32, i32),
        part_idx: usize,
        part: PartMode,
        skip: bool,
    ) -> Result<bool> {
        let (xpb, ypb, w, h) = pb;
        let max_cand = 5 - self.hdr.five_minus_max_num_merge_cand as usize;
        let merge = skip || self.cabac.decision(&mut self.ctx, ctx::MERGE_FLAG) == 1;
        let motion = if merge {
            let mut idx = 0usize;
            if max_cand > 1 {
                if self.cabac.decision(&mut self.ctx, ctx::MERGE_IDX) == 1 {
                    idx = 1;
                    while idx < max_cand - 1 && self.cabac.bypass() == 1 {
                        idx += 1;
                    }
                }
            }
            self.merge_motion(cb, pb, part_idx, part, idx)
        } else {
            let b_slice = self.hdr.slice_type == SliceType::B;
            // inter_pred_idc: 0 list 0, 1 list 1, 2 both.
            let idc = if !b_slice {
                0
            } else if w + h != 12
                && self.cabac.decision(&mut self.ctx, ctx::INTER_PRED_IDC + self.cu_depth as usize) == 1
            {
                2
            } else {
                self.cabac.decision(&mut self.ctx, ctx::INTER_PRED_IDC + 4)
            };
            let mut m = MvField::NONE;
            let mut mvd = [Mv::default(); 2];
            let mut mvp_flag = [0usize; 2];
            for l in 0..2usize {
                if (l == 0 && idc == 1) || (l == 1 && idc == 0) {
                    continue;
                }
                let n_ref = self.hdr.num_ref_idx[l] as usize;
                let mut r = 0usize;
                if n_ref > 1 {
                    // TR with cMax = n_ref - 1: two context-coded bins, then bypass.
                    while r < n_ref - 1 {
                        let bit = if r < 2 {
                            self.cabac.decision(&mut self.ctx, ctx::REF_IDX + r)
                        } else {
                            self.cabac.bypass()
                        };
                        if bit == 0 {
                            break;
                        }
                        r += 1;
                    }
                }
                m.ref_idx[l] = r as i8;
                if l == 1 && self.hdr.mvd_l1_zero && idc == 2 {
                    mvd[1] = Mv::default();
                } else {
                    mvd[l] = self.mvd_coding();
                }
                mvp_flag[l] = self.cabac.decision(&mut self.ctx, ctx::MVP_FLAG) as usize;
            }
            for l in 0..2usize {
                if m.ref_idx[l] >= 0 {
                    let mvp = self.amvp(cb, pb, part_idx, l, m.ref_idx[l] as usize, mvp_flag[l]);
                    m.mv[l] = Mv { x: mvp.x.wrapping_add(mvd[l].x), y: mvp.y.wrapping_add(mvd[l].y) };
                    m.ref_poc[l] = self.ref_poc[l][m.ref_idx[l] as usize];
                }
            }
            m
        };
        for l in 0..2 {
            if motion.ref_idx[l] >= 0 && motion.ref_idx[l] as usize >= self.ref_lists[l].len() {
                return invalid("reference index outside the list");
            }
        }
        self.mark_edges(xpb, ypb, w, h, false);
        self.store_motion(xpb, ypb, w, h, &motion);
        self.predict_inter(xpb, ypb, w, h, &motion)?;
        Ok(merge)
    }

    /// `mvd_coding()` (7.3.8.9).
    fn mvd_coding(&mut self) -> Mv {
        let g0x = self.cabac.decision(&mut self.ctx, ctx::MVD_GREATER0) == 1;
        let g0y = self.cabac.decision(&mut self.ctx, ctx::MVD_GREATER0) == 1;
        let g1x = g0x && self.cabac.decision(&mut self.ctx, ctx::MVD_GREATER1) == 1;
        let g1y = g0y && self.cabac.decision(&mut self.ctx, ctx::MVD_GREATER1) == 1;
        let comp = |dec: &mut Self, g0: bool, g1: bool| -> i32 {
            if !g0 {
                return 0;
            }
            let mut abs = 1i32;
            if g1 {
                // abs_mvd_minus2: EG1.
                let mut k = 1u32;
                let mut v = 0i32;
                while dec.cabac.bypass() == 1 && k < 30 {
                    v += 1 << k;
                    k += 1;
                }
                v += dec.cabac.bypass_bits(k) as i32;
                abs = v + 2;
            }
            if dec.cabac.bypass() == 1 { -abs } else { abs }
        };
        let x = comp(self, g0x, g1x);
        let y = comp(self, g0y, g1y);
        Mv { x: x as i16, y: y as i16 }
    }

    fn store_motion(&mut self, x: i32, y: i32, w: i32, h: i32, m: &MvField) {
        let (xs, ys) = (x as usize >> 2, y as usize >> 2);
        let (xe, ye) = ((x + w) as usize >> 2, (y + h) as usize >> 2);
        let mut col = ColMotion::default();
        for l in 0..2 {
            if m.ref_idx[l] >= 0 {
                col.mv[l] = m.mv[l];
                col.ref_poc[l] = m.ref_poc[l];
                col.flags |= 1 << l;
                if self.ref_lt[l][m.ref_idx[l] as usize] {
                    col.flags |= 4 << l;
                }
            }
        }
        for yy in ys..ye {
            for xx in xs..xe {
                self.pic.mvf[yy * self.pic.w4 + xx] = *m;
                self.cur.motion[yy * self.cur.w4 + xx] = col;
            }
        }
    }

    /// Predict the samples of an inter prediction block into the picture (8.5.3.3).
    fn predict_inter(&mut self, x: i32, y: i32, w: i32, h: i32, m: &MvField) -> Result<()> {
        let bd = self.bit_depth;
        let explicit = (self.hdr.slice_type == SliceType::P && self.pps.weighted_pred)
            || (self.hdr.slice_type == SliceType::B && self.pps.weighted_bipred);
        for c in 0..3usize {
            let luma = c == 0;
            let (bx, by, bw, bh) = if luma {
                (x, y, w as usize, h as usize)
            } else {
                (x / 2, y / 2, w as usize / 2, h as usize / 2)
            };
            let mut have = [false; 2];
            for l in 0..2usize {
                if m.ref_idx[l] < 0 {
                    continue;
                }
                let r = self.refs[self.ref_lists[l][m.ref_idx[l] as usize]];
                let mv = m.mv[l];
                let (ix, iy, fx, fy) = if luma {
                    (
                        bx + (mv.x as i32 >> 2),
                        by + (mv.y as i32 >> 2),
                        (mv.x & 3) as usize,
                        (mv.y & 3) as usize,
                    )
                } else {
                    (
                        bx + (mv.x as i32 >> 3),
                        by + (mv.y as i32 >> 3),
                        (mv.x & 7) as usize,
                        (mv.y & 7) as usize,
                    )
                };
                let out = if l == 0 { &mut self.pred_a } else { &mut self.pred_b };
                inter::interpolate(&r.planes[c], ix, iy, fx, fy, bw, bh, luma, bd, out);
                have[l] = true;
            }
            let wts = |dec: &Self, l: usize| -> Option<inter::Weight> {
                let pw = dec.hdr.pred_weights.as_ref()?;
                let e = pw.weights[l].get(dec.hdr_ref_idx(m, l))?;
                let shift = bd as i32 - 8;
                Some(if luma {
                    inter::Weight { w: e.luma_weight, o: e.luma_offset << shift }
                } else {
                    inter::Weight { w: e.chroma_weight[c - 1], o: e.chroma_offset[c - 1] << shift }
                })
            };
            let denom = |dec: &Self| -> u32 {
                dec.hdr
                    .pred_weights
                    .as_ref()
                    .map_or(0, |p| if luma { p.luma_log2_denom as u32 } else { p.chroma_log2_denom as u32 })
            };
            let ex_bi = if explicit && have[0] && have[1] {
                wts(self, 0).zip(wts(self, 1)).map(|(a, b)| (denom(self), a, b))
            } else {
                None
            };
            let ex_uni =
                |dec: &Self, l: usize| if explicit { wts(dec, l).map(|a| (denom(dec), a)) } else { None };
            let ex0 = ex_uni(self, 0);
            let ex1 = ex_uni(self, 1);
            let plane = &mut self.cur.planes[c];
            match (have[0], have[1]) {
                (true, true) => inter::put_bi(
                    plane,
                    bx as usize,
                    by as usize,
                    bw,
                    bh,
                    &self.pred_a,
                    &self.pred_b,
                    bd,
                    ex_bi,
                ),
                (true, false) => {
                    inter::put_uni(plane, bx as usize, by as usize, bw, bh, &self.pred_a, bd, ex0)
                }
                (false, true) => {
                    inter::put_uni(plane, bx as usize, by as usize, bw, bh, &self.pred_b, bd, ex1)
                }
                _ => return invalid("a prediction block with no reference"),
            }
        }
        Ok(())
    }

    fn hdr_ref_idx(&self, m: &MvField, l: usize) -> usize {
        m.ref_idx[l].max(0) as usize
    }

    fn finish_cu_qp(&mut self, x0: i32, y0: i32, size: i32) {
        let q = self.qp_y as i8;
        self.set_cells(x0, y0, size, |c| c.qp = q);
        self.qp_prev = self.qp_y;
    }

    /// Mark the left and top edges of a block as prediction (`pu`) or transform block edges for the deblocking filter.
    fn mark_edges(&mut self, x0: i32, y0: i32, w: i32, h: i32, tu: bool) {
        let (lf, tf) = if tu { (F_TU_LEFT, F_TU_TOP) } else { (F_PU_LEFT, F_PU_TOP) };
        let (xs, ys) = (x0 as usize >> 2, y0 as usize >> 2);
        for y in ys..((y0 + h) as usize >> 2).min(self.pic.h4) {
            self.pic.cells[y * self.pic.w4 + xs].flags |= lf;
        }
        for x in xs..((x0 + w) as usize >> 2).min(self.pic.w4) {
            self.pic.cells[ys * self.pic.w4 + x].flags |= tf;
        }
    }

    /// 8.4.2: the three candidate modes for the prediction block at (`x`, `y`).
    fn mpm_candidates(&self, x: i32, y: i32) -> [u32; 3] {
        let cand = |xn: i32, yn: i32, above: bool| -> u32 {
            if !self.pic.available(x, y, xn, yn) {
                return 1;
            }
            let c = self.cell(xn, yn);
            if c.mode != 1 {
                return 1;
            }
            if above && yn < ((y >> self.pic.ctb_log2) << self.pic.ctb_log2) {
                return 1;
            }
            c.intra_mode as u32
        };
        let a = cand(x - 1, y, false);
        let b = cand(x, y - 1, true);
        if a == b {
            if a < 2 { [0, 1, 26] } else { [a, 2 + ((a + 29) % 32), 2 + ((a - 2 + 1) % 32)] }
        } else {
            let c = if a != 0 && b != 0 {
                0
            } else if a != 1 && b != 1 {
                1
            } else {
                26
            };
            [a, b, c]
        }
    }

    fn pcm_samples(&mut self, x0: i32, y0: i32, log2: u32) -> Result<()> {
        let mut pos = self.cabac.byte_aligned_pos();
        let n = 1usize << log2;
        let (bl, bc) = (self.sps.pcm_bit_depth_luma as u32, self.sps.pcm_bit_depth_chroma as u32);
        let mut bitpos = pos * 8;
        let rbsp = self.rbsp;
        let mut read = |bits: u32| -> Result<u32> {
            let mut v = 0u32;
            for _ in 0..bits {
                let byte = *rbsp.get(bitpos >> 3).ok_or(Error::Truncated)?;
                v = (v << 1) | ((byte >> (7 - (bitpos & 7))) & 1) as u32;
                bitpos += 1;
            }
            Ok(v)
        };
        for c in 0..3 {
            let (w, bits, sx, sy) = if c == 0 {
                (n, bl, x0 as usize, y0 as usize)
            } else {
                (n / 2, bc, x0 as usize / 2, y0 as usize / 2)
            };
            let shift = self.bit_depth - bits;
            let plane = &mut self.cur.planes[c];
            for y in 0..w {
                for x in 0..w {
                    let v = read(bits)? << shift;
                    plane.data[(sy + y) * plane.stride + sx + x] = v as u16;
                }
            }
        }
        pos = bitpos.div_ceil(8);
        self.cabac.restart(pos);
        Ok(())
    }

    // ---- the transform tree ----------------------------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn transform_tree(
        &mut self,
        x0: i32,
        y0: i32,
        xb: i32,
        yb: i32,
        log2: u32,
        depth: u32,
        blk: u32,
        max_depth: u32,
        intra_split: bool,
        parent_cbf: [bool; 2],
    ) -> Result<()> {
        let sps = self.sps;
        let inter_split =
            sps.max_th_depth_inter == 0 && !self.cu_intra && self.cu_part != PartMode::P2Nx2N && depth == 0;
        let split = if log2 <= sps.log2_max_tb as u32
            && log2 > sps.log2_min_tb as u32
            && depth < max_depth
            && !(intra_split && depth == 0)
        {
            self.cabac.decision(&mut self.ctx, ctx::SPLIT_TRANSFORM + (5 - log2) as usize) == 1
        } else {
            log2 > sps.log2_max_tb as u32 || (intra_split && depth == 0) || inter_split
        };
        let mut cbf = [false, false];
        if log2 > 2 {
            for (c, f) in cbf.iter_mut().enumerate() {
                if depth == 0 || parent_cbf[c] {
                    *f = self.cabac.decision(&mut self.ctx, ctx::CBF_CHROMA + depth as usize) == 1;
                }
            }
        } else {
            // 4x4 luma blocks share the chroma of their parent.
            cbf = parent_cbf;
        }
        if split {
            let h = 1i32 << (log2 - 1);
            let (x1, y1) = (x0 + h, y0 + h);
            self.transform_tree(x0, y0, x0, y0, log2 - 1, depth + 1, 0, max_depth, intra_split, cbf)?;
            self.transform_tree(x1, y0, x0, y0, log2 - 1, depth + 1, 1, max_depth, intra_split, cbf)?;
            self.transform_tree(x0, y1, x0, y0, log2 - 1, depth + 1, 2, max_depth, intra_split, cbf)?;
            self.transform_tree(x1, y1, x0, y0, log2 - 1, depth + 1, 3, max_depth, intra_split, cbf)?;
            return Ok(());
        }
        let cbf_luma = if self.cu_intra || depth != 0 || cbf[0] || cbf[1] {
            self.cabac.decision(&mut self.ctx, ctx::CBF_LUMA + (depth == 0) as usize) == 1
        } else {
            true
        };
        self.transform_unit(x0, y0, xb, yb, log2, depth, blk, cbf_luma, cbf)
    }

    #[allow(clippy::too_many_arguments)]
    fn transform_unit(
        &mut self,
        x0: i32,
        y0: i32,
        xb: i32,
        yb: i32,
        log2: u32,
        _depth: u32,
        blk: u32,
        cbf_luma: bool,
        cbf: [bool; 2],
    ) -> Result<()> {
        let size = 1i32 << log2;
        self.mark_edges(x0, y0, size, size, true);
        if cbf_luma {
            self.set_cells(x0, y0, size, |c| c.flags |= F_CBF);
        }
        let chroma_here = log2 > 2 || blk == 3;
        let cbf_chroma = cbf[0] || cbf[1];
        // Chroma of 4x4 luma blocks is carried by the fourth block.
        let any = cbf_luma || (chroma_here && cbf_chroma) || (log2 == 2 && cbf_chroma);
        if any && self.pps.cu_qp_delta_enabled && !self.is_cu_qp_delta_coded {
            self.parse_cu_qp_delta()?;
        }
        // Luma.
        let luma_mode = self.cell(x0, y0).intra_mode as u32;
        if self.cu_intra {
            self.intra_block(0, x0 as usize, y0 as usize, log2, luma_mode)?;
        }
        if cbf_luma {
            let scan_idx = self.scan_idx(log2, 0, luma_mode);
            let res = self.residual_coding(log2, 0, scan_idx)?;
            self.reconstruct(0, x0 as usize, y0 as usize, log2, &res, luma_mode, true)?;
        }
        if chroma_here {
            let (cx, cy, clog2) = if log2 > 2 {
                (x0 as usize / 2, y0 as usize / 2, log2 - 1)
            } else {
                (xb as usize / 2, yb as usize / 2, 2)
            };
            for c in 1..3usize {
                if self.cu_intra {
                    self.intra_block(c, cx, cy, clog2, self.chroma_mode)?;
                }
                if cbf[c - 1] {
                    let scan_idx = self.scan_idx(clog2, c, self.chroma_mode);
                    let res = self.residual_coding(clog2, c, scan_idx)?;
                    self.reconstruct(c, cx, cy, clog2, &res, self.chroma_mode, true)?;
                }
            }
        }
        Ok(())
    }

    fn scan_idx(&self, log2: u32, c_idx: usize, mode: u32) -> usize {
        if self.cu_intra && (log2 == 2 || (log2 == 3 && c_idx == 0)) {
            if (6..=14).contains(&mode) {
                return 2;
            }
            if (22..=30).contains(&mode) {
                return 1;
            }
        }
        0
    }

    fn parse_cu_qp_delta(&mut self) -> Result<()> {
        let mut v = 0i32;
        if self.cabac.decision(&mut self.ctx, ctx::CU_QP_DELTA) == 1 {
            v = 1;
            while v < 5 && self.cabac.decision(&mut self.ctx, ctx::CU_QP_DELTA + 1) == 1 {
                v += 1;
            }
            if v == 5 {
                // EG0 suffix.
                let mut k = 0u32;
                while self.cabac.bypass() == 1 {
                    v += 1 << k;
                    k += 1;
                    if k > 30 {
                        return invalid("cu_qp_delta_abs too large");
                    }
                }
                v += self.cabac.bypass_bits(k) as i32;
            }
        }
        if v > 0 && self.cabac.bypass() == 1 {
            v = -v;
        }
        self.is_cu_qp_delta_coded = true;
        self.cu_qp_delta_val = v;
        let off = 6 * (self.bit_depth as i32 - 8);
        self.qp_y = ((self.qg_pred + v + 52 + 2 * off) % (52 + off)) - off;
        Ok(())
    }

    // ---- intra prediction and reconstruction -----------------------------------------------------------------------------

    /// Predict an `n x n` block of component `c` at (`x`, `y`) in that component's samples into the picture.
    fn intra_block(&mut self, c: usize, x: usize, y: usize, log2: u32, mode: u32) -> Result<()> {
        let n = 1usize << log2;
        let luma = c == 0;
        let sub = if luma { 0 } else { 1 };
        let (xl, yl) = ((x << sub) as i32, (y << sub) as i32);
        let unit = if luma { 4 } else { 2 };
        let plane = &self.cur.planes[c];
        let mut r = Refs::new();
        let mut al = [false; 129];
        let mut at = [false; 129];
        let constrained = self.pps.constrained_intra_pred;
        let ok = |dec: &Self, xn: i32, yn: i32| -> bool {
            if !dec.pic.available(xl, yl, xn, yn) {
                return false;
            }
            !constrained || dec.cell(xn, yn).mode == 1
        };
        // Corner.
        let corner_ok = ok(self, xl - 1, yl - 1);
        al[0] = corner_ok;
        at[0] = corner_ok;
        if corner_ok {
            r.left[0] = plane.data[(y - 1) * plane.stride + x - 1] as i32;
            r.top[0] = r.left[0];
        }
        // Left and below-left, in units.
        for k in 0..(2 * n / unit) {
            let yy = k * unit;
            let a = ok(self, xl - 1, yl + ((yy << sub) as i32));
            for i in 0..unit {
                al[1 + yy + i] = a;
                if a {
                    r.left[1 + yy + i] = plane.data[(y + yy + i) * plane.stride + x - 1] as i32;
                }
            }
        }
        for k in 0..(2 * n / unit) {
            let xx = k * unit;
            let a = ok(self, xl + ((xx << sub) as i32), yl - 1);
            for i in 0..unit {
                at[1 + xx + i] = a;
                if a {
                    r.top[1 + xx + i] = plane.data[(y - 1) * plane.stride + x + xx + i] as i32;
                }
            }
        }
        intra::substitute(&mut r, &al[..=2 * n], &at[..=2 * n], n, self.bit_depth);
        intra::filter(&mut r, n, mode, luma, self.sps.strong_intra_smoothing, self.bit_depth);
        let pred = &mut self.pred;
        intra::predict(pred, &r, n, mode, luma, self.bit_depth);
        let plane = &mut self.cur.planes[c];
        for yy in 0..n {
            for xx in 0..n {
                plane.data[(y + yy) * plane.stride + x + xx] = pred[yy * n + xx] as u16;
            }
        }
        Ok(())
    }

    /// Scale, transform and add the residual of a block to what is in the picture.
    #[allow(clippy::too_many_arguments)]
    fn reconstruct(
        &mut self,
        c: usize,
        x: usize,
        y: usize,
        log2: u32,
        res: &super::residual::Residual,
        _mode: u32,
        intra: bool,
    ) -> Result<()> {
        let n = 1usize << log2;
        let bd = self.bit_depth as i32;
        let off = 6 * (bd - 8);
        let qp = if c == 0 {
            self.qp_y + off
        } else {
            let o = if c == 1 {
                self.pps.cb_qp_offset as i32 + self.hdr.cb_qp_offset as i32
            } else {
                self.pps.cr_qp_offset as i32 + self.hdr.cr_qp_offset as i32
            };
            let qpi = (self.qp_y + o).clamp(-off, 57);
            chroma_qp(qpi) + off
        };
        let intra4 = intra && self.cu_intra;
        if self.cu_transquant_bypass {
            self.resid[..n * n].copy_from_slice(&self.coef[..n * n]);
        } else {
            // 8.6.3 scaling.
            let shift = bd + log2 as i32 - 5;
            let rnd = 1i64 << (shift - 1);
            let scale = LEVEL_SCALE[(qp % 6) as usize] as i64;
            let qshift = qp / 6;
            let matrix_id = if self.cu_intra { c } else { 3 + c };
            let m_table: Option<&Factors> = self
                .scaling
                .as_ref()
                .map(|s| &s[(log2 - 2) as usize][if log2 == 5 { matrix_id / 3 } else { matrix_id }]);
            for yy in 0..=res.max_y {
                for xx in 0..=res.max_x {
                    let i = yy * n + xx;
                    let l = self.coef[i];
                    if l == 0 {
                        continue;
                    }
                    let m = match (&m_table, res.transform_skip) {
                        (Some(t), _) => t[i] as i64,
                        _ => 16,
                    };
                    let v = ((l as i64 * m * (scale << qshift)) + rnd) >> shift;
                    self.coef[i] = v.clamp(-32768, 32767) as i32;
                }
            }
            if res.transform_skip {
                transform::skip(&self.coef[..n * n], &mut self.resid[..n * n], log2, self.bit_depth);
            } else {
                let dst = c == 0 && log2 == 2 && intra4;
                transform::inverse(
                    &self.coef[..n * n],
                    &mut self.resid[..n * n],
                    log2,
                    dst,
                    self.bit_depth,
                    res.max_x,
                    res.max_y,
                );
            }
        }
        let maxv = (1i32 << bd) - 1;
        let plane = &mut self.cur.planes[c];
        for yy in 0..n {
            for xx in 0..n {
                let p = &mut plane.data[(y + yy) * plane.stride + x + xx];
                *p = (*p as i32 + self.resid[yy * n + xx]).clamp(0, maxv) as u16;
            }
        }
        Ok(())
    }
}

/// `ScalingFactor` for the four sizes (4, 8, 16, 32) and six matrices (the 32x32 ones only for the two luma matrices, at 0 and 1).
fn scaling_factors(l: &ScalingList) -> [Vec<Factors>; 4] {
    let s4 = &SCAN[2][0];
    let s8 = &SCAN[3][0];
    let mut f4 = Vec::new();
    for m in 0..6 {
        let mut t = vec![16u8; 16];
        for (i, &(x, y)) in s4.iter().take(16).enumerate() {
            t[y as usize * 4 + x as usize] = l.l4[m][i];
        }
        f4.push(t);
    }
    let mut f8 = Vec::new();
    let mut f16 = Vec::new();
    for m in 0..6 {
        let mut t8 = vec![16u8; 64];
        let mut t16 = vec![16u8; 256];
        for (i, &(x, y)) in s8.iter().enumerate() {
            let (x, y) = (x as usize, y as usize);
            t8[y * 8 + x] = l.l8[m][i];
            for dy in 0..2 {
                for dx in 0..2 {
                    t16[(y * 2 + dy) * 16 + x * 2 + dx] = l.l16[m][i];
                }
            }
        }
        t16[0] = l.dc16[m];
        f8.push(t8);
        f16.push(t16);
    }
    let mut f32v = Vec::new();
    for m in 0..2 {
        let mut t = vec![16u8; 1024];
        for (i, &(x, y)) in s8.iter().enumerate() {
            let (x, y) = (x as usize, y as usize);
            for dy in 0..4 {
                for dx in 0..4 {
                    t[(y * 4 + dy) * 32 + x * 4 + dx] = l.l32[m][i];
                }
            }
        }
        t[0] = l.dc32[m];
        f32v.push(t);
    }
    [f4, f8, f16, f32v]
}
