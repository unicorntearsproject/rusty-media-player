//! A tiny H.264 stream generator for tests, built from the crate's own writers: I_PCM pictures give every picture
//! a distinct random content, and P/B macroblocks with zero motion copy (or blend) whole reference pictures, so
//! the decoded output reveals exactly which pictures the reference lists, marking and weighting selected.
#![allow(dead_code)]
use rvp_codec_h264::bitstream::nal::write_nal;
use rvp_codec_h264::bitstream::{BitWriter, NalHeader, NalUnitType};
use rvp_codec_h264::cabac::{CabacEncoder, Contexts, ctx};
use rvp_codec_h264::cavlc::{
    predict_nc,
    tables::{CBP_INTER, CBP_INTRA},
    write_residual_block,
};
use rvp_codec_h264::params::{Pps, ScalingMatrices, SliceHeader, SliceType, Sps, Vui};
use rvp_codec_h264::transform::{self, LevelScale, ZIGZAG_4X4, ZIGZAG_8X8};

/// Small deterministic PRNG (xorshift64*).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { (self.next() >> 11) % n }
    }
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// What a macroblock contains.
#[derive(Clone, Copy, Debug)]
pub enum Mb {
    /// I_PCM with random samples.
    Pcm,
    /// P_Skip or B_Skip.
    Skip,
    /// P_L0_16x16 with a reference index.
    P { ref_idx: u32 },
    /// B 16x16 with prediction modes (bit 0 list 0, bit 1 list 1) and reference indices.
    B { pm: u8, ref0: u32, ref1: u32 },
    /// B_Direct_16x16.
    Direct,
    /// A macroblock with residual coefficients (CAVLC only); see [`Res`].
    Res(Res),
}

/// Kinds of macroblock with random residual data. Prediction is always DC (intra) or zero motion from reference 0.
#[derive(Clone, Copy, Debug)]
pub enum Res {
    /// Intra 16x16 with DC prediction; `ac` selects luma AC blocks (cbp luma 15); chroma cbp 0 to 2.
    I16 { ac: bool, chroma: u8 },
    /// Intra 4x4 (all blocks DC) with a coded block pattern.
    I4 { cbp: u8 },
    /// Intra 8x8 (all blocks DC) with a coded block pattern; needs `transform_8x8_mode_flag`.
    I8 { cbp: u8 },
    /// P_L0_16x16 reference 0 with a coded block pattern and optionally the 8x8 transform.
    P { cbp: u8, t8: bool },
}

/// One slice: header, first macroblock and content.
pub struct SliceSpec {
    pub hdr: SliceHeader,
    pub first_mb: usize,
    pub mbs: Vec<Mb>,
}

/// An Annex B stream under construction.
pub struct Stream {
    pub bytes: Vec<u8>,
    pub sps: Sps,
    pub pps: Pps,
    pub mbw: usize,
    pub mbh: usize,
    pub rng: Rng,
    /// Dequantisation tables for the active scaling matrices (used to keep random coefficients in range).
    pub scale: LevelScale,
    /// Largest level magnitude to generate (tests of escape codes raise it at very low QP).
    pub max_level: i32,
}

pub fn make_sps(
    mbw: u32,
    mbh: u32,
    poc_type: u32,
    log2_frame_num: u32,
    log2_poc_lsb: u32,
    max_refs: u32,
    reorder: u32,
) -> Sps {
    Sps {
        profile_idc: 77,
        constraint_flags: 0,
        level_idc: 40,
        id: 0,
        chroma_format_idc: 1,
        separate_colour_plane: false,
        bit_depth_luma: 8,
        bit_depth_chroma: 8,
        transform_bypass: false,
        scaling: None,
        log2_max_frame_num: log2_frame_num,
        poc_type,
        log2_max_poc_lsb: log2_poc_lsb,
        delta_pic_order_always_zero: false,
        offset_for_non_ref_pic: 0,
        offset_for_top_to_bottom_field: 0,
        offset_for_ref_frame: vec![2],
        max_num_ref_frames: max_refs,
        gaps_in_frame_num_allowed: false,
        pic_width_in_mbs: mbw,
        pic_height_in_map_units: mbh,
        frame_mbs_only: true,
        mb_adaptive_frame_field: false,
        direct_8x8_inference: true,
        crop: None,
        vui: Some(Vui {
            video_format: 5,
            num_reorder_frames: Some(reorder),
            max_dec_frame_buffering: Some(max_refs.max(reorder).max(1)),
            ..Default::default()
        }),
    }
}

impl Stream {
    pub fn new(sps: Sps, pps: Pps, seed: u64) -> Self {
        let scale = LevelScale::new(&ScalingMatrices::from_pps(sps.scaling.as_ref(), pps.scaling.as_ref()));
        let mut s = Self {
            bytes: Vec::new(),
            mbw: sps.pic_width_in_mbs as usize,
            mbh: sps.pic_height_in_map_units as usize * if sps.frame_mbs_only { 1 } else { 2 },
            sps,
            pps,
            rng: Rng(seed | 1),
            scale,
            max_level: 40,
        };
        let sps_bytes = s.sps.write();
        write_nal(NalHeader { ref_idc: 3, unit_type: NalUnitType::Sps }, &sps_bytes, true, &mut s.bytes);
        let pps_bytes = s.pps.write();
        write_nal(NalHeader { ref_idc: 3, unit_type: NalUnitType::Pps }, &pps_bytes, true, &mut s.bytes);
        s
    }

    /// Re-emit the PPS (after changing `self.pps`).
    pub fn emit_pps(&mut self) {
        self.scale =
            LevelScale::new(&ScalingMatrices::from_pps(self.sps.scaling.as_ref(), self.pps.scaling.as_ref()));
        let b = self.pps.write();
        write_nal(NalHeader { ref_idc: 3, unit_type: NalUnitType::Pps }, &b, true, &mut self.bytes);
    }

    fn pcm_bytes(&mut self) -> Vec<u8> {
        // Smooth-ish random content so interpolation and deblocking behave like in real video, but distinct per MB.
        // A narrow range of levels keeps most macroblock edges below the deblocking thresholds, so the filter runs.
        let base = if self.rng.chance(15) {
            self.rng.below(256) as i32
        } else {
            100 + self.rng.range(-14, 14) as i32
        };
        let slope = self.rng.range(-2, 2) as i32;
        (0..384)
            .map(|i| (base + slope * (i % 16) + self.rng.range(-8, 8) as i32).clamp(0, 255) as u8)
            .collect()
    }

    pub fn write_slice(&mut self, nal_ref_idc: u8, idr: bool, spec: &SliceSpec) {
        let mut hdr = spec.hdr.clone();
        hdr.nal_ref_idc = nal_ref_idc;
        hdr.idr = idr;
        hdr.first_mb_in_slice = spec.first_mb as u32;
        let mut w = BitWriter::new();
        hdr.write(&self.sps, &self.pps, &mut w);
        let is_intra = hdr.slice_type.is_intra();
        let is_b = hdr.slice_type == SliceType::B;
        let (n0, n1) = (hdr.num_ref_idx_l0_active, hdr.num_ref_idx_l1_active);
        if self.pps.cabac {
            self.cabac_slice_data(&mut w, &hdr, spec);
        } else {
            let mut run = 0u32;
            let mut infos: Vec<[u8; 24]> = vec![[0; 24]; spec.mbs.len()];
            let mut qp = hdr.slice_qp(&self.pps);
            for (i, mb) in spec.mbs.iter().enumerate() {
                if let Mb::Skip = mb {
                    run += 1;
                    continue;
                }
                if !is_intra {
                    w.put_ue(run);
                    run = 0;
                }
                match *mb {
                    Mb::Res(res) => {
                        assert!(!is_b, "residual macroblocks are generated for I and P slices");
                        self.write_res_mb(&mut w, res, is_intra, n0, i, spec, &mut infos, &mut qp);
                    }
                    Mb::Pcm => {
                        infos[i] = [16; 24];
                        w.put_ue(if is_intra {
                            25
                        } else if is_b {
                            23 + 25
                        } else {
                            5 + 25
                        });
                        w.align_zero();
                        let b = self.pcm_bytes();
                        w.put_bytes(&b);
                    }
                    Mb::P { ref_idx } => {
                        w.put_ue(0);
                        if n0 > 1 {
                            w.put_te(ref_idx, n0 - 1);
                        }
                        w.put_se(0);
                        w.put_se(0);
                        w.put_ue(0); // coded_block_pattern 0 (inter)
                    }
                    Mb::B { pm, ref0, ref1 } => {
                        w.put_ue(pm as u32); // B_L0_16x16 = 1, B_L1_16x16 = 2, B_Bi_16x16 = 3
                        if pm & 1 != 0 && n0 > 1 {
                            w.put_te(ref0, n0 - 1);
                        }
                        if pm & 2 != 0 && n1 > 1 {
                            w.put_te(ref1, n1 - 1);
                        }
                        if pm & 1 != 0 {
                            w.put_se(0);
                            w.put_se(0);
                        }
                        if pm & 2 != 0 {
                            w.put_se(0);
                            w.put_se(0);
                        }
                        w.put_ue(0);
                    }
                    Mb::Direct => {
                        w.put_ue(0);
                        w.put_ue(0);
                    }
                    Mb::Skip => unreachable!(),
                }
            }
            if run > 0 {
                w.put_ue(run);
            }
            w.put_trailing_bits();
        }
        let rbsp = w.into_bytes();
        let unit_type = if idr { NalUnitType::IdrSlice } else { NalUnitType::Slice };
        write_nal(NalHeader { ref_idc: nal_ref_idc, unit_type }, &rbsp, true, &mut self.bytes);
    }

    /// Predicted `nC` for block `(bx, by)` of component `comp` (0 luma, 1 Cb, 2 Cr) of macroblock `i` of the slice.
    fn nc_for(
        &self,
        infos: &[[u8; 24]],
        spec: &SliceSpec,
        i: usize,
        comp: usize,
        bx: usize,
        by: usize,
    ) -> i32 {
        let (base, w) = if comp == 0 { (0, 4) } else { (16 + (comp - 1) * 4, 2) };
        let addr = spec.first_mb + i;
        let left = addr % self.mbw > 0 && i >= 1;
        let up = addr >= self.mbw && addr - self.mbw >= spec.first_mb;
        let na = if bx > 0 {
            Some(infos[i][base + by * w + bx - 1])
        } else if left {
            Some(infos[i - 1][base + by * w + w - 1])
        } else {
            None
        };
        let nb = if by > 0 {
            Some(infos[i][base + (by - 1) * w + bx])
        } else if up {
            Some(infos[i - self.mbw][base + (w - 1) * w + bx])
        } else {
            None
        };
        predict_nc(na, nb)
    }

    /// Random coefficient levels for a block of `n` scan positions whose dequantised magnitude sum stays within
    /// `limit` (`dq(level, scan_index)` gives the dequantised value), so no decoder overflows.
    fn gen_levels(&mut self, n: usize, limit: i64, dq: &dyn Fn(i32, usize) -> i32) -> Vec<i32> {
        let mut lv = vec![0i32; n];
        let density = self.rng.below(100);
        for (i, l) in lv.iter_mut().enumerate() {
            // Low frequencies are likelier, like real data.
            let p = (density / 2).saturating_sub(i as u64 * 40 / n as u64);
            if self.rng.chance(p) {
                let big = self.rng.chance(15);
                let mag =
                    if big { self.rng.range(1, self.max_level as i64) } else { self.rng.range(1, 3) } as i32;
                *l = if self.rng.chance(50) { mag } else { -mag };
            }
        }
        loop {
            let sum: i64 = lv.iter().enumerate().map(|(i, &l)| (dq(l, i) as i64).abs()).sum();
            if sum <= limit {
                return lv;
            }
            for l in lv.iter_mut() {
                *l /= 2;
            }
        }
    }

    /// Write one macroblock with residual data (CAVLC). Updates `infos[i]` (coefficient counts) and `qp`.
    #[allow(clippy::too_many_arguments)]
    fn write_res_mb(
        &mut self,
        w: &mut BitWriter,
        res: Res,
        intra_slice: bool,
        n0: u32,
        i: usize,
        spec: &SliceSpec,
        infos: &mut [[u8; 24]],
        qp: &mut i32,
    ) {
        let t8_mode = self.pps.transform_8x8_mode;
        let intra = !matches!(res, Res::P { .. });
        let intra_off = if intra_slice { 0 } else { 5 };
        let (cbp_luma, cbp_chroma, i16, t8) = match res {
            Res::I16 { ac, chroma } => (if ac { 15 } else { 0 }, chroma, true, false),
            Res::I4 { cbp } => (cbp & 15, cbp >> 4, false, false),
            Res::I8 { cbp } => (cbp & 15, cbp >> 4, false, true),
            Res::P { cbp, t8 } => (cbp & 15, cbp >> 4, false, t8 && t8_mode && cbp & 15 != 0),
        };
        // Macroblock prediction part.
        match res {
            Res::I16 { .. } => {
                w.put_ue(intra_off + 1 + 2 + 4 * cbp_chroma as u32 + if cbp_luma != 0 { 12 } else { 0 });
                w.put_ue(0); // intra_chroma_pred_mode: DC
            }
            Res::I4 { .. } | Res::I8 { .. } => {
                w.put_ue(intra_off);
                if t8_mode {
                    w.put_bit(t8);
                }
                for _ in 0..if t8 { 4 } else { 16 } {
                    w.put_bit(true); // prev_intra_pred_mode_flag: use the predicted (DC) mode
                }
                w.put_ue(0);
            }
            Res::P { .. } => {
                w.put_ue(0);
                if n0 > 1 {
                    w.put_te(0, n0 - 1);
                }
                w.put_se(0);
                w.put_se(0);
            }
        }
        let cbp = cbp_luma | cbp_chroma << 4;
        if !i16 {
            let table = if intra { &CBP_INTRA } else { &CBP_INTER };
            let code = table.iter().position(|&c| c == cbp).expect("cbp in table") as u32;
            w.put_ue(code);
            if !intra && t8_mode && cbp_luma != 0 {
                w.put_bit(t8);
            }
        }
        if cbp == 0 && !i16 {
            return;
        }
        // mb_qp_delta
        let dqp = self.rng.range(-5, 5).clamp(-(*qp as i64), 51 - *qp as i64) as i32;
        w.put_se(dqp);
        *qp += dqp;
        let qp = *qp;
        let (per, rem) = ((qp / 6) as u32, (qp % 6) as usize);
        let l4 = if intra { 0 } else { 3 };
        let l8 = if intra { 0 } else { 1 };
        let ls = self.scale.clone();
        let offs = [self.pps.chroma_qp_index_offset, self.pps.second_chroma_qp_index_offset];
        let mut nz = [0u8; 24];
        // Luma.
        if i16 {
            // DC levels: bound the result of the inverse Hadamard so DC contributions stay small.
            let ls00 = ls.l4[l4][rem][0];
            let lv = loop {
                let cand = self.gen_levels(16, i64::MAX, &|l, _| l);
                let mut c = [0i32; 16];
                for k in 0..16 {
                    c[ZIGZAG_4X4[k] as usize] = cand[k];
                }
                transform::luma_dc_dequant(&mut c, qp, ls00);
                if c.iter().all(|v| v.abs() <= 3000) {
                    break cand;
                }
                // Too large: try again with a smaller level cap.
                self.max_level = (self.max_level / 2).max(2);
            };
            let nc = self.nc_for(infos, spec, i, 0, 0, 0);
            write_residual_block(w, nc, 16, &lv);
        }
        for i8x8 in 0..4usize {
            if cbp_luma & (1 << i8x8) == 0 {
                continue;
            }
            let blocks: Vec<Vec<i32>> = if t8 {
                let mut lv8 = self.gen_levels(64, 10000, &|l, k| {
                    transform::dequant_8x8(l, ls.l8[l8][rem][ZIGZAG_8X8[k] as usize], per)
                });
                if lv8.iter().all(|&v| v == 0) && std::env::var_os("RVP_SYNTH_ZERO8").is_none() {
                    lv8[self.rng.below(64) as usize] = 1;
                }
                (0..4).map(|b| (0..16).map(|k| lv8[4 * k + b]).collect()).collect()
            } else {
                (0..4)
                    .map(|_| {
                        if i16 {
                            self.gen_levels(15, 9000, &|l, k| {
                                transform::dequant_4x4(l, ls.l4[l4][rem][ZIGZAG_4X4[k + 1] as usize], per)
                            })
                        } else {
                            self.gen_levels(16, 12000, &|l, k| {
                                transform::dequant_4x4(l, ls.l4[l4][rem][ZIGZAG_4X4[k] as usize], per)
                            })
                        }
                    })
                    .collect()
            };
            for (i4, lv) in blocks.iter().enumerate() {
                let idx = i8x8 * 4 + i4;
                let (bx, by) = ((idx >> 2 & 1) * 2 + (idx & 1), (idx >> 3 & 1) * 2 + (idx >> 1 & 1));
                // The block's own count is not visible to itself: use the counts written so far.
                let mut cur = [0u8; 24];
                cur.copy_from_slice(&nz);
                infos[i] = cur;
                let nc = self.nc_for(infos, spec, i, 0, bx, by);
                let total = write_residual_block(w, nc, if i16 { 15 } else { 16 }, lv);
                nz[by * 4 + bx] = total as u8;
            }
        }
        // Chroma (4:2:0).
        if cbp_chroma != 0 {
            let mut dc: [Vec<i32>; 2] = [vec![], vec![]];
            for comp in 0..2 {
                let qpc = transform::chroma_qp(qp, offs[comp]);
                let ls00 = ls.l4[l4 + 1 + comp][(qpc % 6) as usize][0];
                dc[comp] = loop {
                    let cand = self.gen_levels(4, i64::MAX, &|l, _| l);
                    let mut c = [cand[0], cand[1], cand[2], cand[3]];
                    transform::chroma_dc_dequant(&mut c, qpc, ls00);
                    if c.iter().all(|v| v.abs() <= 3000) {
                        break cand;
                    }
                    self.max_level = (self.max_level / 2).max(2);
                };
            }
            for lv in &dc {
                write_residual_block(w, -1, 4, lv);
            }
            if cbp_chroma == 2 {
                for comp in 0..2 {
                    let qpc = transform::chroma_qp(qp, offs[comp]);
                    let (cper, crem) = ((qpc / 6) as u32, (qpc % 6) as usize);
                    for blk in 0..4usize {
                        let lv = self.gen_levels(15, 9000, &|l, k| {
                            transform::dequant_4x4(
                                l,
                                ls.l4[l4 + 1 + comp][crem][ZIGZAG_4X4[k + 1] as usize],
                                cper,
                            )
                        });
                        let mut cur = [0u8; 24];
                        cur.copy_from_slice(&nz);
                        infos[i] = cur;
                        let nc = self.nc_for(infos, spec, i, 1 + comp, blk & 1, blk >> 1);
                        let total = write_residual_block(w, nc, 15, &lv);
                        nz[16 + comp * 4 + blk] = total as u8;
                    }
                }
            }
        }
        infos[i] = nz;
    }

    /// CABAC slice data for I_PCM and skipped macroblocks only.
    fn cabac_slice_data(&mut self, w: &mut BitWriter, hdr: &SliceHeader, spec: &SliceSpec) {
        // cabac_alignment_one_bit
        while !w.is_byte_aligned() {
            w.put_bit(true);
        }
        let intra = hdr.slice_type.is_intra();
        let is_b = hdr.slice_type == SliceType::B;
        let qp = hdr.slice_qp(&self.pps);
        let mut ctxs = Contexts::new(qp, if intra { None } else { Some(hdr.cabac_init_idc) });
        let mut enc = CabacEncoder::new();
        let first = spec.first_mb;
        let mbw = self.mbw;
        // Per-MB state of this slice: skipped?
        let mut skipped = vec![false; spec.mbs.len()];
        let n = spec.mbs.len();
        for (i, mb) in spec.mbs.iter().enumerate() {
            assert!(matches!(mb, Mb::Pcm | Mb::Skip), "CABAC synth supports only PCM and skip");
            let addr = first + i;
            let avail_a = addr % mbw > 0 && i >= 1;
            let avail_b = addr >= mbw && addr - mbw >= first;
            let a_idx = i.wrapping_sub(1);
            let b_idx = i.wrapping_sub(mbw);
            if !intra {
                let skip_cond = |avail: bool, idx: usize| (avail && !skipped[idx]) as usize;
                let inc = skip_cond(avail_a, a_idx) + skip_cond(avail_b, b_idx);
                let base = if is_b { ctx::MB_SKIP_B } else { ctx::MB_SKIP_P };
                let is_skip = matches!(mb, Mb::Skip);
                enc.decision(&mut ctxs, base + inc, is_skip as u32);
                skipped[i] = is_skip;
            }
            if let Mb::Pcm = mb {
                // mb_type: I_PCM
                let inc_i = avail_a as usize + avail_b as usize; // neighbours are PCM or skipped (not I_NxN)
                if intra {
                    enc.decision(&mut ctxs, ctx::MB_TYPE_I + inc_i, 1);
                } else if is_b {
                    // prefix 1 1 1 1 0 1, then the I suffix
                    let cond = |avail: bool, idx: usize| (avail && !skipped_direct(spec, idx)) as usize;
                    let b = ctx::MB_TYPE_B;
                    enc.decision(&mut ctxs, b + cond(avail_a, a_idx) + cond(avail_b, b_idx), 1);
                    enc.decision(&mut ctxs, b + 3, 1);
                    enc.decision(&mut ctxs, b + 4, 1);
                    enc.decision(&mut ctxs, b + 5, 1);
                    enc.decision(&mut ctxs, b + 5, 0);
                    enc.decision(&mut ctxs, b + 5, 1);
                } else {
                    enc.decision(&mut ctxs, ctx::MB_TYPE_P, 1);
                }
                if !intra {
                    enc.decision(
                        &mut ctxs,
                        if is_b { ctx::MB_TYPE_B_INTRA } else { ctx::MB_TYPE_P_INTRA },
                        1,
                    );
                }
                enc.terminate(1);
                let bytes = self.pcm_bytes();
                let wtr = enc.writer();
                wtr.align_zero();
                wtr.put_bytes(&bytes);
                enc.restart();
            }
            enc.terminate((i + 1 == n) as u32);
        }
        // The encoder's flush already wrote the rbsp_stop_one_bit; finish() pads the last byte with zeros.
        let bytes = enc.finish();
        w.put_bytes(&bytes);
    }
}

fn skipped_direct(spec: &SliceSpec, idx: usize) -> bool {
    matches!(spec.mbs.get(idx), Some(Mb::Skip) | Some(Mb::Direct))
}
