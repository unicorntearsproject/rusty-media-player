//! Synthetic streams: reference management, POC types, output order, weighted prediction, multiple slices and
//! deblocking decisions that x264 never produces, checked against ffmpeg. See `common/synth.rs`.
mod common;

use common::synth::*;
use rvp_codec_h264::decoder::Decoder;
use rvp_codec_h264::params::{
    DecRefPicMarking, Mmco, Pps, PredWeightTable, RefListMod, SliceHeader, SliceType, WeightEntry,
};
use std::path::PathBuf;
use std::process::{Command, Stdio};

static FFMPEG_LOG: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// `RVP_SYNTH_NO=weights,mod,mmco,slices,b,cabac,deblock,idr,poc,pcm,skip` switches features off to localise a failure.
fn no(feature: &str) -> bool {
    std::env::var("RVP_SYNTH_NO").map(|v| v.split(',').any(|f| f == feature)).unwrap_or(false)
}

/// What ffmpeg decodes from a raw Annex B stream.
fn ffmpeg_frames(bytes: &[u8], tag: &str) -> Vec<u8> {
    let path: PathBuf = std::env::temp_dir().join(format!("rvp-synth-{}-{tag}.h264", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new("ffmpeg")
        .args(["-v", "warning", "-f", "h264", "-i"])
        .arg(&path)
        .args(["-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .stderr(Stdio::piped())
        .output()
        .expect("run ffmpeg");
    let _ = std::fs::remove_file(&path);
    assert!(out.status.success(), "ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr));
    *FFMPEG_LOG.lock().unwrap() = String::from_utf8_lossy(&out.stderr).into_owned();
    out.stdout
}

/// Decode with our decoder; frames as one byte string in output order.
fn our_frames(bytes: &[u8]) -> (Vec<u8>, usize, rvp_codec_h264::decoder::Stats) {
    let mut dec = Decoder::new();
    let _ = dec.decode_annexb(bytes, 0);
    let _ = dec.flush();
    let mut out = Vec::new();
    let mut n = 0;
    while let Some(f) = dec.next_frame() {
        for p in &f.planes {
            out.extend_from_slice(p);
        }
        n += 1;
    }
    (out, n, dec.stats())
}

/// Require our output to equal ffmpeg's for this stream, naming the seed on failure.
fn check_stream(bytes: &[u8], tag: &str, frame_bytes: usize) {
    let want = ffmpeg_frames(bytes, tag);
    let (got, n, stats) = our_frames(bytes);
    assert_eq!(stats.errors, 0, "{tag}: decoder reported errors {stats:?}");
    assert_eq!(n * frame_bytes, got.len());
    if want.len() / frame_bytes != n {
        std::fs::write(std::env::temp_dir().join(format!("rvp-synth-fail-{tag}.h264")), bytes).ok();
        let log = FFMPEG_LOG.lock().unwrap().clone();
        panic!(
            "{tag}: frame count (ffmpeg {}, ours {n}); ffmpeg said:\n{}",
            want.len() / frame_bytes,
            &log[..log.len().min(1500)]
        );
    }
    if got != want {
        let bad: Vec<usize> = (0..n)
            .filter(|&i| {
                got[i * frame_bytes..(i + 1) * frame_bytes] != want[i * frame_bytes..(i + 1) * frame_bytes]
            })
            .collect();
        let f = bad[0];
        std::fs::write(std::env::temp_dir().join(format!("rvp-synth-fail-{tag}.h264")), bytes).ok();
        let a = &got[f * frame_bytes..(f + 1) * frame_bytes];
        let b = &want[f * frame_bytes..(f + 1) * frame_bytes];
        let luma = frame_bytes * 2 / 3;
        let first = (0..luma).find(|&i| a[i] != b[i]).unwrap_or(0);
        panic!(
            "{tag}: output frame {f} of {n} differs from ffmpeg (all differing: {bad:?}; first luma byte {first}; stream kept in the temp dir)"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------
// A model of the reference marking, enough to generate valid streams.

#[derive(Clone, Copy, Debug)]
struct ShortRef {
    frame_num: u32,
}

#[derive(Clone, Copy, Debug)]
struct LongRef {
    idx: u32,
}

#[derive(Default, Clone)]
struct Model {
    shorts: Vec<ShortRef>,
    longs: Vec<LongRef>,
    max_long_idx: i32,
}

impl Model {
    fn count(&self) -> usize {
        self.shorts.len() + self.longs.len()
    }
}

struct Gen {
    s: Stream,
    model: Model,
    max_fn: u32,
    max_poc_lsb: i32,
    prev_ref_fn: u32,
    prev_fn: u32,
    fn_offset: i64,
    idr_id: u32,
    cabac: bool,
    poc_type: u32,
    pending_frames: usize,
    frame_bytes: usize,
    /// Temporal direct mode: refs are never dropped and list 0 holds all of them, so MapColToList0 always succeeds.
    temporal: bool,
}

impl Gen {
    fn frame_num_wrap(&self, fnum: u32, cur: u32) -> i64 {
        if fnum > cur { fnum as i64 - self.max_fn as i64 } else { fnum as i64 }
    }

    /// Random marking operations valid for the current model; returns the ops and applies them to the model.
    fn gen_marking(&mut self, cur_fn: u32, allow_mmco5: bool, max_refs: usize) -> DecRefPicMarking {
        let mut ops: Vec<Mmco> = Vec::new();
        let mut m = self.s.rng.below(100);
        let mut model = self.model.clone();
        let mut cur_long: Option<u32> = None;
        let mut n_ops = self.s.rng.range(1, 3);
        if allow_mmco5 && m < 5 {
            ops.push(Mmco::ClearAll);
            model = Model { max_long_idx: -1, ..Default::default() };
            n_ops = 0;
            m = 100;
        }
        let _ = m;
        for _ in 0..n_ops {
            match self.s.rng.below(5) {
                0 if !model.shorts.is_empty() => {
                    let i = self.s.rng.below(model.shorts.len() as u64) as usize;
                    let pn = self.frame_num_wrap(model.shorts[i].frame_num, cur_fn);
                    let diff = cur_fn as i64 - pn; // CurrPicNum - picNumX = difference_of_pic_nums_minus1 + 1
                    ops.push(Mmco::ShortTermUnused { difference_of_pic_nums_minus1: (diff - 1) as u32 });
                    model.shorts.remove(i);
                }
                1 if !model.longs.is_empty() => {
                    let i = self.s.rng.below(model.longs.len() as u64) as usize;
                    ops.push(Mmco::LongTermUnused { long_term_pic_num: model.longs[i].idx });
                    model.longs.remove(i);
                }
                2 if !model.shorts.is_empty() && model.max_long_idx >= 0 => {
                    let i = self.s.rng.below(model.shorts.len() as u64) as usize;
                    let idx = self.s.rng.below(model.max_long_idx as u64 + 1) as u32;
                    let pn = self.frame_num_wrap(model.shorts[i].frame_num, cur_fn);
                    let diff = cur_fn as i64 - pn;
                    ops.push(Mmco::AssignLongTerm {
                        difference_of_pic_nums_minus1: (diff - 1) as u32,
                        long_term_frame_idx: idx,
                    });
                    model.longs.retain(|l| l.idx != idx);
                    model.shorts.remove(i);
                    model.longs.push(LongRef { idx });
                }
                3 => {
                    // Long-term frame indices stay below 2: ffmpeg looks long-term references up by array position
                    // (`long_ref[0..long_ref_count]`), so with sparse indices it treats distinct long-term pictures
                    // as one in the deblocking filter, where the specification (and this decoder) tell them apart.
                    let v = self.s.rng.range(0, 2) as i32; // max_long_term_frame_idx_plus1
                    ops.push(Mmco::MaxLongTermIdx { max_long_term_frame_idx_plus1: v as u32 });
                    model.max_long_idx = v - 1;
                    model.longs.retain(|l| (l.idx as i32) < v);
                }
                4 if model.max_long_idx >= 0 && cur_long.is_none() => {
                    // Marking the current picture long-term must be the last operation (it would otherwise
                    // be affected by later operations on the same index).
                    cur_long = Some(self.s.rng.below(model.max_long_idx as u64 + 1) as u32);
                }
                _ => {}
            }
        }
        // Keep the total (with the current picture) within max_num_ref_frames.
        loop {
            let total = model.count() + 1;
            if total <= max_refs {
                break;
            }
            if let Some(i) = (0..model.shorts.len())
                .min_by_key(|&i| self.frame_num_wrap(model.shorts[i].frame_num, cur_fn))
            {
                let pn = self.frame_num_wrap(model.shorts[i].frame_num, cur_fn);
                ops.push(Mmco::ShortTermUnused {
                    difference_of_pic_nums_minus1: (cur_fn as i64 - pn - 1) as u32,
                });
                model.shorts.remove(i);
            } else if !model.longs.is_empty() {
                ops.push(Mmco::LongTermUnused { long_term_pic_num: model.longs[0].idx });
                model.longs.remove(0);
            } else {
                break;
            }
        }
        match cur_long {
            Some(idx) => {
                ops.push(Mmco::CurrentLongTerm { long_term_frame_idx: idx });
                model.longs.retain(|l| l.idx != idx);
                model.longs.push(LongRef { idx });
            }
            None => {
                // After MMCO 5 the picture counts as having frame_num 0.
                let cleared = ops.iter().any(|o| matches!(o, Mmco::ClearAll));
                model.shorts.push(ShortRef { frame_num: if cleared { 0 } else { cur_fn } });
            }
        }
        self.model = model;
        DecRefPicMarking { adaptive: true, ops, ..Default::default() }
    }

    fn sliding_window(&mut self, cur_fn: u32, max_refs: usize) -> DecRefPicMarking {
        if self.model.count() >= max_refs {
            if let Some(i) = (0..self.model.shorts.len())
                .min_by_key(|&i| self.frame_num_wrap(self.model.shorts[i].frame_num, cur_fn))
            {
                self.model.shorts.remove(i);
            }
        }
        self.model.shorts.push(ShortRef { frame_num: cur_fn });
        DecRefPicMarking::default()
    }

    /// Random reference list modification ops for a list of `n` active entries.
    fn gen_list_mod(&mut self, cur_fn: u32, n_active: u32) -> Option<Vec<RefListMod>> {
        if self.s.rng.chance(60) || self.model.count() == 0 || no("mod") {
            return None;
        }
        let mut ops = Vec::new();
        let mut pred = cur_fn as i64;
        let n = self.s.rng.range(1, 3.min(n_active as i64));
        for _ in 0..n {
            let k = self.s.rng.below(self.model.count() as u64) as usize;
            if k < self.model.shorts.len() {
                let pn = self.frame_num_wrap(self.model.shorts[k].frame_num, cur_fn);
                let no_wrap = pn.rem_euclid(self.max_fn as i64);
                // Choose subtract or add at random; abs_diff in 1..=MaxPicNum.
                let (idc, abs) = if self.s.rng.chance(50) {
                    let d = (pred - no_wrap).rem_euclid(self.max_fn as i64);
                    (0, if d == 0 { self.max_fn as i64 } else { d })
                } else {
                    let d = (no_wrap - pred).rem_euclid(self.max_fn as i64);
                    (1, if d == 0 { self.max_fn as i64 } else { d })
                };
                ops.push(RefListMod { idc, value: (abs - 1) as u32 });
                pred = no_wrap;
            } else {
                let l = self.model.longs[k - self.model.shorts.len()];
                ops.push(RefListMod { idc: 2, value: l.idx });
            }
        }
        Some(ops)
    }

    fn weights(&mut self, n0: u32, n1: u32, b: bool) -> PredWeightTable {
        let luma_d = self.s.rng.range(0, 6) as u32;
        let chroma_d = self.s.rng.range(0, 6) as u32;
        let mut t =
            PredWeightTable { luma_log2_denom: luma_d, chroma_log2_denom: chroma_d, ..Default::default() };
        for (list, n) in [(0usize, n0), (1, if b { n1 } else { 0 })] {
            for _ in 0..n {
                let lf = self.s.rng.chance(60);
                let cf = self.s.rng.chance(60);
                t.entries[list].push(WeightEntry {
                    luma_flag: lf,
                    luma_weight: if lf { self.s.rng.range(-10, 40) as i32 } else { 1 << luma_d },
                    luma_offset: if lf { self.s.rng.range(-25, 25) as i32 } else { 0 },
                    chroma_flag: cf,
                    chroma_weight: if cf {
                        [self.s.rng.range(-10, 40) as i32, self.s.rng.range(-10, 40) as i32]
                    } else {
                        [1 << chroma_d; 2]
                    },
                    chroma_offset: if cf {
                        [self.s.rng.range(-25, 25) as i32, self.s.rng.range(-25, 25) as i32]
                    } else {
                        [0; 2]
                    },
                });
            }
        }
        t
    }

    /// Emit one picture. `kind` is the slice type; `poc` its display order count (in units of one frame = 2).
    fn picture(
        &mut self,
        kind: SliceType,
        poc: i32,
        idr: bool,
        ref_idc: u8,
        allow_mmco5: bool,
        max_refs: usize,
    ) {
        let pps = self.s.pps.clone();
        let cur_fn = if idr { 0 } else { (self.prev_ref_fn + 1) % self.max_fn };
        // Frame number offset for POC type 1/2 bookkeeping.
        if idr {
            self.fn_offset = 0;
        } else if self.prev_fn > cur_fn {
            self.fn_offset += self.max_fn as i64;
        }
        let mut hdr = SliceHeader::new(kind, idr);
        hdr.pps_id = 0;
        hdr.frame_num = cur_fn;
        hdr.idr_pic_id = self.idr_id;
        match self.poc_type {
            0 => hdr.pic_order_cnt_lsb = (poc as i64).rem_euclid(self.max_poc_lsb as i64) as u32,
            1 => {
                // expectedPOC with one offset_for_ref_frame of 2 and offset_for_non_ref_pic -1.
                let mut abs = self.fn_offset + cur_fn as i64;
                if ref_idc == 0 && abs > 0 {
                    abs -= 1;
                }
                let mut expected = if idr { 0 } else { 2 * abs };
                if ref_idc == 0 {
                    expected += self.s.sps.offset_for_non_ref_pic as i64;
                }
                hdr.delta_pic_order_cnt[0] = (poc as i64 - expected) as i32;
            }
            _ => {}
        }
        hdr.direct_spatial_mv_pred = !(self.temporal && kind == SliceType::B);
        let avail = self.model.count() as u32;
        let (mut n0, mut n1) = (0u32, 0u32);
        if kind != SliceType::I {
            n0 = self.s.rng.range(1, avail.clamp(1, 5) as i64) as u32;
            if kind == SliceType::B {
                n1 = self.s.rng.range(1, avail.clamp(1, 5) as i64) as u32;
            }
            hdr.num_ref_idx_override = n0 != pps.num_ref_idx_l0_default
                || (kind == SliceType::B && n1 != pps.num_ref_idx_l1_default);
            hdr.num_ref_idx_l0_active = n0;
            hdr.num_ref_idx_l1_active = n1;
            if !hdr.num_ref_idx_override {
                hdr.num_ref_idx_l0_active = pps.num_ref_idx_l0_default;
                hdr.num_ref_idx_l1_active = if kind == SliceType::B { pps.num_ref_idx_l1_default } else { 0 };
                n0 = hdr.num_ref_idx_l0_active;
                n1 = hdr.num_ref_idx_l1_active;
            }
            // Entries must exist: never use more references than the model holds.
            if n0 > avail || n1 > avail {
                hdr.num_ref_idx_override = true;
                n0 = n0.min(avail.max(1));
                n1 = n1.min(avail.max(1));
                hdr.num_ref_idx_l0_active = n0;
                hdr.num_ref_idx_l1_active = if kind == SliceType::B { n1 } else { 0 };
            }
            if self.temporal && kind == SliceType::B {
                n0 = avail;
                hdr.num_ref_idx_override = true;
                hdr.num_ref_idx_l0_active = n0;
            }
            hdr.ref_list_mod[0] =
                if self.temporal { None } else { self.gen_list_mod(cur_fn, hdr.num_ref_idx_l0_active) };
            if kind == SliceType::B {
                hdr.ref_list_mod[1] =
                    if self.temporal { None } else { self.gen_list_mod(cur_fn, hdr.num_ref_idx_l1_active) };
            }
            if !no("weights") && (pps.weighted_pred && kind == SliceType::P)
                || (pps.weighted_bipred_idc == 1 && kind == SliceType::B)
            {
                hdr.pred_weight_table = Some(self.weights(n0, n1, kind == SliceType::B));
            }
        }
        if !self.cabac || true {
            hdr.cabac_init_idc = self.s.rng.below(3) as u32;
        }
        hdr.slice_qp_delta = self.s.rng.range(-6, 10) as i32;
        // Marking.
        let mut mmco5 = false;
        if ref_idc != 0 {
            let marking = if idr {
                self.model =
                    Model { shorts: vec![ShortRef { frame_num: 0 }], longs: vec![], max_long_idx: -1 };
                DecRefPicMarking::default()
            } else if (self.s.rng.chance(45) && !no("mmco") && !self.temporal)
                || (self.model.shorts.is_empty() && self.model.count() >= max_refs)
            {
                let m = self.gen_marking(cur_fn, allow_mmco5, max_refs);
                mmco5 = m.ops.iter().any(|o| matches!(o, Mmco::ClearAll));
                m
            } else {
                self.sliding_window(cur_fn, max_refs)
            };
            hdr.dec_ref_pic_marking = Some(marking);
        } else if idr {
            unreachable!("IDR pictures are reference pictures");
        }
        if std::env::var_os("RVP_SYNTH_TRACE").is_some() {
            eprintln!(
                "pic {:?} poc {poc} fn {cur_fn} ref_idc {ref_idc} idr {idr} n0 {n0} n1 {n1} mod {:?} marking {:?} model {:?} / {:?}",
                kind, hdr.ref_list_mod, hdr.dec_ref_pic_marking, self.model.shorts, self.model.longs
            );
        }
        // Slices: split the macroblocks.
        let total = self.s.mbw * self.s.mbh;
        let n_slices = if no("slices") { 1 } else { self.s.rng.range(1, 3.min(total as i64)) as usize };
        let mut bounds: Vec<usize> = vec![0, total];
        while bounds.len() < n_slices + 1 {
            let c = self.s.rng.range(1, total as i64 - 1) as usize;
            if !bounds.contains(&c) {
                bounds.push(c);
            }
        }
        bounds.sort();
        for w in bounds.windows(2) {
            let (first, end) = (w[0], w[1]);
            let mut sh = hdr.clone();
            sh.disable_deblocking_filter_idc = if no("deblock") { 1 } else { self.s.rng.below(3) as u32 };
            sh.slice_alpha_c0_offset_div2 = self.s.rng.range(-3, 3) as i32;
            sh.slice_beta_offset_div2 = self.s.rng.range(-3, 3) as i32;
            if sh.disable_deblocking_filter_idc == 1 {
                sh.slice_alpha_c0_offset_div2 = 0;
                sh.slice_beta_offset_div2 = 0;
            }
            let mut slice_kind = kind;
            // A P or B picture may contain an I slice.
            if kind != SliceType::I && self.s.rng.chance(8) && first != 0 {
                slice_kind = SliceType::I;
                sh = SliceHeader {
                    slice_type: SliceType::I,
                    ref_list_mod: [None, None],
                    pred_weight_table: None,
                    num_ref_idx_override: false,
                    num_ref_idx_l0_active: 0,
                    num_ref_idx_l1_active: 0,
                    ..sh
                };
            }
            let mut mbs = Vec::new();
            for _ in first..end {
                mbs.push(self.random_mb(slice_kind, n0, n1));
            }
            if std::env::var_os("RVP_SYNTH_TRACE").is_some() {
                eprintln!(
                    "   slice first {first} type {:?} qp_delta {} dbk idc {} a {} b {} weights {} mbs {:?}",
                    sh.slice_type,
                    sh.slice_qp_delta,
                    sh.disable_deblocking_filter_idc,
                    sh.slice_alpha_c0_offset_div2,
                    sh.slice_beta_offset_div2,
                    sh.pred_weight_table.is_some(),
                    mbs
                );
            }
            self.s.write_slice(ref_idc, idr, &SliceSpec { hdr: sh, first_mb: first, mbs });
        }
        // State for the next picture.
        self.prev_fn = cur_fn;
        if ref_idc != 0 {
            self.prev_ref_fn = cur_fn;
        }
        if mmco5 {
            self.prev_ref_fn = 0;
            self.prev_fn = 0;
            self.fn_offset = 0;
        }
        let _ = (self.pending_frames, self.frame_bytes);
    }

    fn random_mb(&mut self, kind: SliceType, n0: u32, n1: u32) -> Mb {
        let r = self.s.rng.below(100);
        match kind {
            SliceType::I => Mb::Pcm,
            SliceType::P => {
                if r < 25 {
                    Mb::Skip
                } else if r < 85 && !self.cabac {
                    Mb::P { ref_idx: self.s.rng.below(n0 as u64) as u32 }
                } else if r < 85 {
                    Mb::Skip
                } else {
                    Mb::Pcm
                }
            }
            _ => {
                if self.cabac {
                    return if r < 70 { Mb::Skip } else { Mb::Pcm };
                }
                if r < 20 {
                    Mb::Skip
                } else if r < 30 {
                    Mb::Direct
                } else if r < 85 {
                    let pm = 1 + self.s.rng.below(3) as u8;
                    Mb::B {
                        pm,
                        ref0: self.s.rng.below(n0 as u64) as u32,
                        ref1: self.s.rng.below(n1 as u64) as u32,
                    }
                } else {
                    Mb::Pcm
                }
            }
        }
    }
}

/// Generate a random valid stream from `seed`; `temporal` selects the temporal-direct variant.
fn random_stream(seed: u64, temporal: bool, interlace_capable: bool) -> (Vec<u8>, usize) {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    for _ in 0..4 {
        r.next();
    }
    let mbw = r.range(2, 4) as u32;
    let mut mbh = r.range(2, 3) as u32;
    if interlace_capable {
        mbh = 4; // two map units: frame_mbs_only_flag 0 doubles the height
    }
    let use_b = (temporal || r.chance(60)) && !no("b");
    let poc_type = if no("poc") {
        0
    } else if use_b {
        if r.chance(30) { 1 } else { 0 }
    } else {
        r.below(3) as u32
    };
    let max_refs = if temporal { 14 } else { r.range(1, 5) as u32 };
    let log2_fn = r.range(4, 6) as u32;
    let log2_poc = r.range(6, 8) as u32;
    let reorder = if use_b { 2 } else { 0 };
    let mut sps = make_sps(mbw, mbh, poc_type, log2_fn, log2_poc, max_refs, reorder);
    if interlace_capable {
        // Frame pictures only (field_pic_flag 0, no MBAFF): decodes like progressive video.
        sps.frame_mbs_only = false;
        sps.pic_height_in_map_units = mbh / 2;
    }
    sps.offset_for_non_ref_pic = -1;
    sps.offset_for_ref_frame = vec![2];
    sps.vui.as_mut().unwrap().max_dec_frame_buffering = Some((max_refs + reorder + 1).min(16));
    let mut pps = Pps::new(0, 0);
    pps.deblocking_filter_control_present = true;
    pps.cabac = r.chance(25) && !no("cabac");
    pps.weighted_pred = !pps.cabac && r.chance(40) && !no("weights");
    pps.weighted_bipred_idc = if pps.cabac || no("weights") { 0 } else { r.below(3) as u32 };
    pps.num_ref_idx_l0_default = r.range(1, 3) as u32;
    pps.num_ref_idx_l1_default = r.range(1, 3) as u32;
    pps.pic_init_qp = r.range(20, 36) as i32;
    pps.chroma_qp_index_offset = r.range(-4, 4) as i32;
    pps.second_chroma_qp_index_offset = pps.chroma_qp_index_offset;
    pps.constrained_intra_pred = r.chance(30);
    let cabac = pps.cabac;
    let stream = Stream::new(sps.clone(), pps, r.next());
    let frame_bytes = (mbw * 16 * mbh * 16 * 3 / 2) as usize;
    let mut g = Gen {
        s: stream,
        model: Model { max_long_idx: -1, ..Default::default() },
        max_fn: 1 << log2_fn,
        max_poc_lsb: 1 << log2_poc,
        prev_ref_fn: 0,
        prev_fn: 0,
        fn_offset: 0,
        idr_id: 0,
        cabac,
        poc_type,
        pending_frames: 0,
        frame_bytes,
        temporal,
    };
    let max_refs = max_refs as usize;
    g.picture(SliceType::I, 0, true, 3, false, max_refs);
    let mut next_poc = 2;
    let n_pics = if temporal { r.range(6, 11) } else { r.range(12, 40) };
    let mut count = 1;
    while count < n_pics {
        let roll = r.below(100);
        if roll < 4 && !no("idr") && !temporal {
            // New IDR period.
            g.idr_id = (g.idr_id + 1) % 8;
            g.picture(SliceType::I, 0, true, 3, false, max_refs);
            next_poc = 2;
            count += 1;
        } else if roll < 10 && !temporal {
            let ref_idc = if poc_type == 2 { 2 } else { r.below(3) as u8 };
            g.picture(SliceType::I, next_poc, false, ref_idc, false, max_refs);
            next_poc += 2;
            count += 1;
        } else if use_b && g.model.count() > 0 && r.chance(65) {
            let nb = r.range(1, 2) as i32;
            g.picture(SliceType::P, next_poc + 2 * nb, false, 2, false, max_refs);
            // B pictures in a random order; each non-reference or reference.
            let mut order: Vec<i32> = (0..nb).collect();
            if r.chance(50) {
                order.reverse();
            }
            for k in order {
                let ref_idc = if r.chance(30) { 1 } else { 0 };
                if g.model.count() == 0 {
                    break;
                }
                g.picture(SliceType::B, next_poc + 2 * k, false, ref_idc, false, max_refs);
            }
            next_poc += 2 * (nb + 1);
            count += 1 + nb as i64;
        } else {
            let ref_idc = if poc_type == 2 || r.chance(80) { 2 } else { 0 };
            // MMCO 5 only in P-only streams (it resets POC, so nothing may be waiting to be output before it).
            let allow5 = !use_b && ref_idc != 0;
            if g.model.count() == 0 {
                g.picture(SliceType::I, next_poc, false, 2, false, max_refs);
            } else {
                g.picture(SliceType::P, next_poc, false, ref_idc, allow5, max_refs);
            }
            next_poc += 2;
            count += 1;
        }
    }
    (g.s.bytes, frame_bytes)
}

#[test]
fn random_reference_streams_match_ffmpeg() {
    if common::skip() {
        return;
    }
    let n: u64 = std::env::var("RVP_SYNTH_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let start: u64 = std::env::var("RVP_SYNTH_START").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut failures = Vec::new();
    for seed in start..start + n {
        let (bytes, fb) = random_stream(seed, false, false);
        let r = std::panic::catch_unwind(|| check_stream(&bytes, &format!("seed{seed}"), fb));
        if let Err(e) = r {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            failures.push(format!("seed {seed}: {msg}"));
        }
    }
    assert!(failures.is_empty(), "{} of {n} streams differ:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn temporal_direct_streams_match_ffmpeg() {
    if common::skip() {
        return;
    }
    let n: u64 = std::env::var("RVP_SYNTH_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
    let start: u64 = std::env::var("RVP_SYNTH_START").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut failures = Vec::new();
    for seed in start..start + n {
        let (bytes, fb) = random_stream(seed, true, false);
        let r = std::panic::catch_unwind(|| check_stream(&bytes, &format!("temporal{seed}"), fb));
        if let Err(e) = r {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            failures.push(format!("seed {seed}: {msg}"));
        }
    }
    assert!(failures.is_empty(), "{} of {n} streams differ:\n{}", failures.len(), failures.join("\n"));
}

/// Hand-built experiment: two reference pictures, one B picture whose two macroblocks have the given contents; the
/// output of both decoders must agree (used to localise deblocking differences).
fn experiment(left: Mb, right: Mb, long_term: bool, seed: u64) -> Result<(), String> {
    let mut sps = make_sps(2, 1, 0, 4, 6, 2, 1);
    sps.vui.as_mut().unwrap().max_dec_frame_buffering = Some(4);
    let mut pps = Pps::new(0, 0);
    pps.deblocking_filter_control_present = true;
    pps.pic_init_qp = 34;
    let mut s = Stream::new(sps, pps, seed);
    let pcm = |s: &mut Stream, idr: bool, fnum: u32, poc: u32, marking: DecRefPicMarking, kind: SliceType| {
        let mut h = SliceHeader::new(kind, idr);
        h.frame_num = fnum;
        h.pic_order_cnt_lsb = poc;
        h.dec_ref_pic_marking = Some(marking);
        s.write_slice(2, idr, &SliceSpec { hdr: h, first_mb: 0, mbs: vec![Mb::Pcm, Mb::Pcm] });
    };
    pcm(&mut s, true, 0, 0, DecRefPicMarking::default(), SliceType::I);
    let marking = if long_term {
        DecRefPicMarking {
            adaptive: true,
            ops: vec![
                Mmco::MaxLongTermIdx { max_long_term_frame_idx_plus1: 2 },
                Mmco::CurrentLongTerm { long_term_frame_idx: 1 },
            ],
            ..Default::default()
        }
    } else {
        DecRefPicMarking::default()
    };
    pcm(&mut s, false, 1, 8, marking, SliceType::I);
    let mut h = SliceHeader::new(SliceType::B, false);
    h.frame_num = 2;
    h.pic_order_cnt_lsb = 4;
    h.num_ref_idx_override = true;
    h.num_ref_idx_l0_active = 2;
    h.num_ref_idx_l1_active = 2;
    h.slice_qp_delta = 6;
    s.write_slice(0, false, &SliceSpec { hdr: h, first_mb: 0, mbs: vec![left, right] });
    let fb = 32 * 16 * 3 / 2;
    std::panic::catch_unwind(|| check_stream(&s.bytes, "experiment", fb))
        .map_err(|e| e.downcast_ref::<String>().cloned().unwrap_or_default())
}

#[test]
#[ignore]
fn bs_experiments() {
    for long_term in [false, true] {
        for (name, left, right) in [
            ("explicit/explicit", Mb::B { pm: 3, ref0: 1, ref1: 0 }, Mb::B { pm: 3, ref0: 0, ref1: 0 }),
            ("explicit same refs", Mb::B { pm: 3, ref0: 0, ref1: 0 }, Mb::B { pm: 3, ref0: 1, ref1: 1 }),
            ("l0 vs l1", Mb::B { pm: 1, ref0: 0, ref1: 0 }, Mb::B { pm: 2, ref0: 0, ref1: 0 }),
            ("skip/explicit", Mb::Skip, Mb::B { pm: 3, ref0: 0, ref1: 0 }),
            ("direct/explicit", Mb::Direct, Mb::B { pm: 3, ref0: 1, ref1: 0 }),
        ] {
            for seed in 1..8 {
                let r = experiment(left, right, long_term, seed);
                println!(
                    "long_term {long_term} {name} seed {seed}: {}",
                    match r {
                        Ok(()) => "ok".to_string(),
                        Err(e) => e.chars().take(120).collect(),
                    }
                );
            }
        }
    }
}

#[test]
fn frame_pictures_of_an_interlace_capable_stream_decode() {
    if common::skip() {
        return;
    }
    for seed in 1..40u64 {
        let (bytes, fb) = random_stream(seed, false, true);
        check_stream(&bytes, &format!("interlace-capable{seed}"), fb);
    }
}

// ---------------------------------------------------------------------------------------------------------
// Residual data: CAVLC blocks of every shape, 4x4 and 8x8 transforms and scaling matrices at SPS and PPS level.

fn rand_scaling(r: &mut Rng, count: usize) -> rvp_codec_h264::params::ScalingSyntax {
    use rvp_codec_h264::params::scaling::ListSpec;
    let lists = (0..count)
        .map(|i| match r.below(10) {
            0..=3 => ListSpec::NotPresent,
            4..=5 => ListSpec::UseDefault,
            _ => {
                let n = if i < 6 { 16 } else { 64 };
                let base = r.range(4, 24) as u8;
                ListSpec::Explicit(
                    (0..n).map(|k| (base as i64 + r.range(0, 24) * (k as i64) / n as i64) as u8).collect(),
                )
            }
        })
        .collect();
    rvp_codec_h264::params::ScalingSyntax { lists }
}

/// A High-profile stream of macroblocks with random residuals (intra DC prediction, zero-motion P).
fn residual_stream(seed: u64) -> (Vec<u8>, usize) {
    let mut r = Rng(seed.wrapping_mul(0xD6E8_FEB8_6659_FD93) | 1);
    for _ in 0..4 {
        r.next();
    }
    let mbw = r.range(2, 4) as u32;
    let mbh = r.range(2, 3) as u32;
    let mut sps = make_sps(mbw, mbh, 0, 6, 8, 2, 0);
    sps.profile_idc = 100;
    if r.chance(50) {
        sps.scaling = Some(rand_scaling(&mut r, 8));
    }
    let mut pps = Pps::new(0, 0);
    pps.has_extension = true;
    pps.transform_8x8_mode = r.chance(60);
    if r.chance(50) {
        pps.scaling = Some(rand_scaling(&mut r, 6 + 2 * pps.transform_8x8_mode as usize));
    }
    pps.deblocking_filter_control_present = true;
    pps.constrained_intra_pred = r.chance(30);
    pps.pic_init_qp = r.range(8, 40) as i32;
    pps.chroma_qp_index_offset = r.range(-6, 6) as i32;
    pps.second_chroma_qp_index_offset = r.range(-6, 6) as i32;
    pps.num_ref_idx_l0_default = 1;
    let t8 = pps.transform_8x8_mode;
    let mut s = Stream::new(sps, pps, r.next());
    s.max_level = if r.chance(20) { 2500 } else { 40 };
    let total = (mbw * mbh) as usize;
    let n_pics = r.range(3, 7);
    let mut frame_num = 0u32;
    for pic in 0..n_pics {
        let kind = if pic == 0 || r.chance(15) { SliceType::I } else { SliceType::P };
        let idr = pic == 0;
        let mut h = SliceHeader::new(kind, idr);
        h.frame_num = frame_num;
        h.pic_order_cnt_lsb = (2 * pic) as u32;
        h.slice_qp_delta = r.range(-4, 6) as i32;
        h.disable_deblocking_filter_idc = r.below(3) as u32;
        h.slice_alpha_c0_offset_div2 = r.range(-3, 3) as i32;
        h.slice_beta_offset_div2 = r.range(-3, 3) as i32;
        if h.disable_deblocking_filter_idc == 1 {
            h.slice_alpha_c0_offset_div2 = 0;
            h.slice_beta_offset_div2 = 0;
        }
        h.dec_ref_pic_marking = Some(DecRefPicMarking::default());
        let two = kind == SliceType::P && pic >= 2;
        if two {
            h.num_ref_idx_override = true;
            h.num_ref_idx_l0_active = 2;
        } else if kind == SliceType::P {
            h.num_ref_idx_l0_active = 1;
        }
        let n_slices = r.range(1, 2) as usize;
        let split = if n_slices == 2 { r.range(1, total as i64 - 1) as usize } else { total };
        let mut ranges = vec![(0, split)];
        if n_slices == 2 {
            ranges.push((split, total));
        }
        for (first, end) in ranges {
            let mbs: Vec<Mb> = (first..end)
                .map(|_| {
                    let roll = r.below(100);
                    let cbp = r.below(48) as u8;
                    let intra_kind = |r: &mut Rng| match r.below(100) {
                        0..=39 => Mb::Res(Res::I16 { ac: r.chance(50), chroma: r.below(3) as u8 }),
                        40..=64 => Mb::Res(Res::I4 { cbp }),
                        65..=89 if t8 => Mb::Res(Res::I8 { cbp }),
                        65..=89 => Mb::Res(Res::I4 { cbp }),
                        _ => Mb::Pcm,
                    };
                    if kind == SliceType::I {
                        return intra_kind(&mut r);
                    }
                    match roll {
                        0..=44 => Mb::Res(Res::P { cbp, t8: r.chance(60) }),
                        45..=58 => Mb::Skip,
                        59..=68 => Mb::P { ref_idx: r.below(if two { 2 } else { 1 }) as u32 },
                        _ => intra_kind(&mut r),
                    }
                })
                .collect();
            if std::env::var_os("RVP_SYNTH_TRACE").is_some() {
                eprintln!(
                    "pic {pic} {:?} first {first} qp_delta {} dbk idc {} a {} b {} mbs {:?}",
                    kind,
                    h.slice_qp_delta,
                    h.disable_deblocking_filter_idc,
                    h.slice_alpha_c0_offset_div2,
                    h.slice_beta_offset_div2,
                    mbs
                );
            }
            s.write_slice(2, idr, &SliceSpec { hdr: h.clone(), first_mb: first, mbs });
        }
        frame_num = (frame_num + 1) % 64;
    }
    (s.bytes, (mbw * 16 * mbh * 16 * 3 / 2) as usize)
}

#[test]
fn residual_streams_with_scaling_matrices_match_ffmpeg() {
    if common::skip() {
        return;
    }
    let n: u64 = std::env::var("RVP_SYNTH_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let start: u64 = std::env::var("RVP_SYNTH_START").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut failures = Vec::new();
    for seed in start..start + n {
        let (bytes, fb) = residual_stream(seed);
        let r = std::panic::catch_unwind(|| check_stream(&bytes, &format!("residual{seed}"), fb));
        if let Err(e) = r {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            failures.push(format!("seed {seed}: {msg}"));
        }
    }
    assert!(failures.is_empty(), "{} of {n} streams differ:\n{}", failures.len(), failures.join("\n"));
}
