//! A tiny H.264 stream generator for tests, built from the crate's own writers: I_PCM pictures give every picture
//! a distinct random content, and P/B macroblocks with zero motion copy (or blend) whole reference pictures, so
//! the decoded output reveals exactly which pictures the reference lists, marking and weighting selected.
#![allow(dead_code)]
use rvp_codec_h264::bitstream::nal::write_nal;
use rvp_codec_h264::bitstream::{BitWriter, NalHeader, NalUnitType};
use rvp_codec_h264::cabac::{CabacEncoder, Contexts, ctx};
use rvp_codec_h264::params::{Pps, SliceHeader, SliceType, Sps, Vui};

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
        let mut s = Self {
            bytes: Vec::new(),
            mbw: sps.pic_width_in_mbs as usize,
            mbh: sps.pic_height_in_map_units as usize,
            sps,
            pps,
            rng: Rng(seed | 1),
        };
        let sps_bytes = s.sps.write();
        write_nal(NalHeader { ref_idc: 3, unit_type: NalUnitType::Sps }, &sps_bytes, true, &mut s.bytes);
        let pps_bytes = s.pps.write();
        write_nal(NalHeader { ref_idc: 3, unit_type: NalUnitType::Pps }, &pps_bytes, true, &mut s.bytes);
        s
    }

    /// Re-emit the PPS (after changing `self.pps`).
    pub fn emit_pps(&mut self) {
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
            for mb in &spec.mbs {
                if let Mb::Skip = mb {
                    run += 1;
                    continue;
                }
                if !is_intra {
                    w.put_ue(run);
                    run = 0;
                }
                match *mb {
                    Mb::Pcm => {
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
