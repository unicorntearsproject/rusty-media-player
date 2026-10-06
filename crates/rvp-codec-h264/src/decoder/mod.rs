//! The H.264 picture decoder.
//!
//! [`Decoder`] takes NAL units (Annex B byte streams, or length-prefixed samples with an avcC record) and
//! produces decoded frames in output order.

mod cabac_syntax;
pub mod deblock;
mod deblock_simd;
mod deblock_tables;
mod direct;
pub mod dpb;
pub mod entropy;
pub mod inter;
mod inter_mb;
mod inter_simd;
pub mod intra;
pub mod mbinfo;
mod mvpred;
pub mod parse;
pub mod picture;
pub mod poc;
pub mod recon;
pub mod slice;

use crate::bitstream::{NalHeader, NalUnitType, nal};
use crate::error::{Error, Result};
use crate::params::{DecRefPicMarking, ParamSets, Pps, SliceHeader, SliceType, Sps};
use crate::transform::LevelScale;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use deblock::SliceFilter;
use parse::{JobSlot, ParseJob, SliceWork, StatsCell};
use picture::{Motion, MotionSlot, Picture, RefState};
use poc::{PocResult, PocState};
use recon::{OutputInfo, ReconEvent, ReconExecutor, Reconstructor, SliceRecon};

/// Largest picture accepted by default, in macroblocks (level 5.1: 4096x2304). Bounds memory for hostile streams.
pub const DEFAULT_MAX_MBS: usize = 36_864;

/// Run the WebAssembly SIMD128 self-tests of the kernels (0 mismatches expected).
#[cfg(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64"))]
pub fn simd_selftest() -> u32 {
    inter_simd::selftest() + deblock_simd::selftest()
}

/// A decoded, cropped output picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Width in luma samples.
    pub width: usize,
    /// Height in luma samples.
    pub height: usize,
    /// Y, Cb, Cr planes (4:2:0, 8 bit), tightly cropped.
    pub planes: [Vec<u8>; 3],
    /// Row strides of the planes in bytes.
    pub strides: [usize; 3],
    /// Timestamp of the access unit that carried this picture.
    pub pts: i64,
    /// Picture order count.
    pub poc: i32,
    /// `video_full_range_flag` from the VUI.
    pub full_range: bool,
    /// `matrix_coefficients` from the VUI (2 if unspecified).
    pub matrix_coefficients: u8,
}

/// Counters for diagnostics and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Pictures finished.
    pub pictures: u64,
    /// Slices decoded without error.
    pub slices: u64,
    /// Slices (or NAL units) that failed.
    pub errors: u64,
    /// Macroblocks that were concealed because no slice covered them or a slice failed.
    pub concealed_mbs: u64,
}

struct CurPic {
    pic: Picture,
    sps: Arc<Sps>,
    hdr: SliceHeader,
    poc: PocResult,
    /// The slices read so far; they are parsed when the picture is complete.
    slices: Vec<SliceWork>,
    /// A slice starting at macroblock 0 has been seen.
    saw_first_mb: bool,
    marking: Option<DecRefPicMarking>,
    pts: i64,
}

/// The decoder.
pub struct Decoder {
    sets: ParamSets,
    length_size: usize,
    sps: Option<Arc<Sps>>,
    dpb: Vec<Picture>,
    cur: Option<CurPic>,
    out: VecDeque<Frame>,
    poc: PocState,
    prev_ref_frame_num: u32,
    max_long_term_idx: i32,
    next_uid: i32,
    decode_counter: u64,
    scale: Option<(Arc<Sps>, Arc<Pps>, Arc<LevelScale>)>,
    scratch: Vec<u8>,
    stats: Arc<StatsCell>,
    max_mbs: usize,
    /// Counts resets (seeks): parsing jobs of an earlier count are abandoned.
    epoch: Arc<core::sync::atomic::AtomicU64>,
    /// The first error of a picture parsed inline since a decode call began.
    parse_error: Option<Error>,
    /// Parses pictures on other threads, if the host provided a runner; otherwise each picture is parsed inline.
    runner: Option<Box<dyn parse::ParseRunner>>,
    /// Reconstruction when it runs inline (no executor).
    recon: Reconstructor,
    /// Reconstruction on another thread, if the host provided one.
    exec: Option<Box<dyn ReconExecutor>>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

/// True if `b` starts a different primary coded picture than `a` (7.4.1.2.4).
fn is_new_picture(a: &SliceHeader, b: &SliceHeader, sps: &Sps) -> bool {
    a.frame_num != b.frame_num
        || a.pps_id != b.pps_id
        || a.field_pic != b.field_pic
        || a.bottom_field != b.bottom_field
        || (a.nal_ref_idc != b.nal_ref_idc && (a.nal_ref_idc == 0 || b.nal_ref_idc == 0))
        || (sps.poc_type == 0
            && (a.pic_order_cnt_lsb != b.pic_order_cnt_lsb
                || a.delta_pic_order_cnt_bottom != b.delta_pic_order_cnt_bottom))
        || (sps.poc_type == 1
            && (a.delta_pic_order_cnt[0] != b.delta_pic_order_cnt[0]
                || a.delta_pic_order_cnt[1] != b.delta_pic_order_cnt[1]))
        || a.idr != b.idr
        || (a.idr && b.idr && a.idr_pic_id != b.idr_pic_id)
}

impl Decoder {
    /// A decoder with no parameter sets.
    pub fn new() -> Self {
        Self {
            sets: ParamSets::new(),
            length_size: 4,
            sps: None,
            dpb: Vec::new(),
            cur: None,
            out: VecDeque::new(),
            poc: PocState::default(),
            prev_ref_frame_num: 0,
            max_long_term_idx: -1,
            next_uid: 1,
            decode_counter: 0,
            scale: None,
            scratch: Vec::new(),
            stats: Arc::new(StatsCell::default()),
            max_mbs: DEFAULT_MAX_MBS,
            epoch: Arc::new(core::sync::atomic::AtomicU64::new(0)),
            parse_error: None,
            runner: None,
            recon: Reconstructor::new(),
            exec: None,
        }
    }

    /// The pictures currently held: `(poc, is_reference, waiting_for_output)`. For tests and debugging.
    pub fn dpb_snapshot(&self) -> Vec<(i32, bool, bool)> {
        self.dpb.iter().map(|p| (p.poc, p.is_ref(), p.needed_for_output)).collect()
    }

    /// Limit the picture size (in macroblocks) this decoder will allocate; larger streams fail with `Unsupported`.
    pub fn set_max_mbs(&mut self, mbs: usize) {
        self.max_mbs = mbs;
    }

    /// Reconstruct pictures with `exec` (typically on a thread of its own) instead of inline, so that parsing the
    /// next picture overlaps the reconstruction of this one. Must be set before the first picture.
    pub fn set_recon_executor(&mut self, exec: Box<dyn ReconExecutor>) {
        self.exec = Some(exec);
    }

    /// Parse pictures with `runner` (typically on worker threads) instead of inline, so that several pictures are parsed
    /// at once. Needs a [`ReconExecutor`] as well (reconstruction waits for the parsing of its picture). Must be set
    /// before the first picture.
    pub fn set_parse_runner(&mut self, runner: Box<dyn parse::ParseRunner>) {
        self.runner = Some(runner);
    }

    /// Reconstruction events still queued or running (always 0 without an executor).
    pub fn pending(&self) -> usize {
        self.exec.as_ref().map_or(0, |e| e.pending())
    }

    /// Hand an event to the reconstruction side (inline, or through the executor).
    fn emit(&mut self, ev: ReconEvent) {
        match &mut self.exec {
            Some(e) => e.submit(ev),
            None => self.recon.handle(ev, &mut self.out),
        }
    }

    /// Diagnostic counters.
    pub fn stats(&self) -> Stats {
        use core::sync::atomic::Ordering::Relaxed;
        Stats {
            pictures: self.stats.pictures.load(Relaxed),
            slices: self.stats.slices.load(Relaxed),
            errors: self.stats.errors.load(Relaxed),
            concealed_mbs: self.stats.concealed_mbs.load(Relaxed),
        }
    }

    /// Load parameter sets and the NAL length size from an `avcC` record.
    pub fn set_avcc(&mut self, extra: &[u8]) -> Result<()> {
        let c = nal::AvcConfig::parse(extra).ok_or(Error::Invalid("bad avcC record"))?;
        self.length_size = c.length_size;
        for n in c.sps.iter().chain(c.pps.iter()) {
            // Errors in individual sets are not fatal; the stream usually repeats them.
            let _ = self.decode_nal(n, 0);
        }
        Ok(())
    }

    /// Decode one length-prefixed sample (an access unit in MP4/Matroska form).
    pub fn decode_sample(&mut self, data: &[u8], pts: i64) -> Result<()> {
        let mut first_err = None;
        for n in nal::split_length_prefixed(data, self.length_size) {
            if let Err(e) = self.decode_nal(n, pts) {
                first_err.get_or_insert(e);
            }
        }
        // An access unit is complete at the end of a sample.
        if let Err(e) = self.finish_picture() {
            first_err.get_or_insert(e);
        }
        if let Some(e) = self.parse_error.take() {
            first_err.get_or_insert(e);
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Decode an Annex B byte stream chunk (complete NAL units with start codes).
    pub fn decode_annexb(&mut self, data: &[u8], pts: i64) -> Result<()> {
        let mut first_err = None;
        for n in nal::split_annexb(data) {
            if let Err(e) = self.decode_nal(n, pts) {
                first_err.get_or_insert(e);
            }
        }
        if let Some(e) = self.parse_error.take() {
            first_err.get_or_insert(e);
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Take the next picture in output order, if one is ready.
    pub fn next_frame(&mut self) -> Option<Frame> {
        if let Some(e) = &mut self.exec {
            e.take_frames(&mut self.out);
        }
        self.out.pop_front()
    }

    /// End of stream: finish the current picture and make every held picture available, in output order.
    pub fn flush(&mut self) -> Result<()> {
        let r = self.finish_picture();
        while self.bump() {}
        if let Some(e) = &mut self.exec {
            e.wait_idle();
        }
        match self.parse_error.take() {
            Some(e) => Err(e),
            None => r,
        }
    }

    /// Forget all pictures and state (after a seek), keeping the parameter sets.
    pub fn reset(&mut self) {
        self.cur = None;
        self.dpb.clear();
        // Pictures still being parsed belong to the old position.
        self.epoch.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        self.emit(ReconEvent::Reset);
        if let Some(e) = &mut self.exec {
            // Frames the reconstruction side finished meanwhile belong to the old position.
            e.wait_idle();
            e.take_frames(&mut self.out);
        }
        self.out.clear();
        self.poc = PocState::default();
        self.prev_ref_frame_num = 0;
        self.max_long_term_idx = -1;
    }

    /// Decode one NAL unit (header byte included, no start code).
    pub fn decode_nal(&mut self, nal_unit: &[u8], pts: i64) -> Result<()> {
        let hdr = NalHeader::parse(nal_unit).ok_or(Error::Invalid("bad NAL header"))?;
        let payload = &nal_unit[1..];
        match hdr.unit_type {
            NalUnitType::Sps => {
                self.finish_picture()?;
                let mut buf = core::mem::take(&mut self.scratch);
                nal::unescape(payload, &mut buf);
                let r = Sps::parse(&buf);
                self.scratch = buf;
                self.sets.insert_sps(r?);
                Ok(())
            }
            NalUnitType::Pps => {
                self.finish_picture()?;
                let mut buf = core::mem::take(&mut self.scratch);
                nal::unescape(payload, &mut buf);
                let r = Pps::parse(&buf, &self.sets);
                self.scratch = buf;
                self.sets.insert_pps(r?);
                Ok(())
            }
            NalUnitType::Slice | NalUnitType::IdrSlice => {
                // The slice's bytes go on to the parsing of the picture, so the buffer is not reused.
                let mut buf = Vec::with_capacity(payload.len());
                nal::unescape(payload, &mut buf);
                let r = self.decode_slice(hdr, buf, pts);
                if r.is_err() {
                    self.stats.errors.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                }
                r
            }
            NalUnitType::Aud | NalUnitType::Sei | NalUnitType::EndOfSequence | NalUnitType::EndOfStream => {
                self.finish_picture()
            }
            NalUnitType::Partition(_) => Err(Error::Unsupported("data partitioning")),
            _ => Ok(()),
        }
    }

    // ---------------------------------------------------------------------------------------------------

    fn decode_slice(&mut self, nal: NalHeader, rbsp: Vec<u8>, pts: i64) -> Result<()> {
        let (hdr, sps, pps) = SliceHeader::parse(&rbsp, nal, &self.sets)?;
        // Frame pictures of a stream that merely allows interlace (frame_mbs_only_flag 0 without MBAFF) decode like
        // progressive ones; field pictures and macroblock-adaptive frame/field pairs are not supported.
        if hdr.field_pic || sps.mb_adaptive_frame_field {
            return Err(Error::Unsupported("interlaced video (PAFF/MBAFF)"));
        }
        if sps.width_mbs() * sps.height_mbs() > self.max_mbs {
            return Err(Error::Unsupported("picture larger than the decoder's size limit"));
        }
        if sps.chroma_format_idc != 1 || sps.separate_colour_plane {
            return Err(Error::Unsupported("chroma format other than 4:2:0"));
        }
        if sps.bit_depth_luma != 8 || sps.bit_depth_chroma != 8 || sps.transform_bypass {
            return Err(Error::Unsupported("bit depth above 8 or lossless coding"));
        }
        if pps.num_slice_groups > 1 {
            return Err(Error::Unsupported("slice groups (FMO)"));
        }
        if matches!(hdr.slice_type, SliceType::Sp | SliceType::Si) {
            return Err(Error::Unsupported("SP/SI slices"));
        }
        if hdr.redundant_pic_cnt > 0 {
            return Ok(());
        }
        if let Some(cur) = &self.cur {
            // 7.4.1.2.4 can miss a new picture (for example POC type 2 after a picture with MMCO 5 that has the same
            // frame_num). A slice that restarts at macroblock 0 when that macroblock is already decoded starts a new one.
            let restarts = hdr.first_mb_in_slice == 0 && cur.saw_first_mb;
            if is_new_picture(&cur.hdr, &hdr, &cur.sps) || restarts {
                self.finish_picture()?;
            }
        }
        if self.cur.is_none() {
            self.start_picture(&hdr, &sps, pts)?;
        }
        let cur_poc = self.cur.as_ref().map(|c| c.pic.poc).ok_or(Error::Invalid("no current picture"))?;
        let refs = self.make_ref_lists(&hdr, &sps, cur_poc);
        // Level scale tables for this (SPS, PPS) pair.
        let stale = match &self.scale {
            Some((s, p, _)) => !(Arc::ptr_eq(s, &sps) && Arc::ptr_eq(p, &pps)),
            None => true,
        };
        if stale {
            let m = crate::params::ScalingMatrices::from_pps(sps.scaling.as_ref(), pps.scaling.as_ref());
            self.scale = Some((sps.clone(), pps.clone(), Arc::new(LevelScale::new(&m))));
        }
        let ls = self.scale.as_ref().map(|s| s.2.clone()).ok_or(Error::Invalid("no scale tables"))?;
        // Direct prediction in a B slice reads the motion of RefPicList1[0].
        let col = (hdr.slice_type == SliceType::B)
            .then(|| refs[1].first().map(|r| self.dpb[r.dpb_idx].motion.clone()))
            .flatten();
        let Some(cur) = self.cur.as_mut() else { return Err(Error::Invalid("no current picture")) };
        cur.saw_first_mb |= hdr.first_mb_in_slice == 0;
        let recon = SliceRecon {
            refs: refs.clone(),
            cur_poc,
            weight_mode: match hdr.slice_type {
                SliceType::P | SliceType::Sp => pps.weighted_pred as u8,
                SliceType::B => pps.weighted_bipred_idc as u8,
                _ => 0,
            },
            pwt: hdr.pred_weight_table.clone(),
            constrained_intra: pps.constrained_intra_pred,
            filter: SliceFilter {
                disable_idc: hdr.disable_deblocking_filter_idc as u8,
                offset_a: (hdr.slice_alpha_c0_offset_div2 * 2) as i8,
                offset_b: (hdr.slice_beta_offset_div2 * 2) as i8,
            },
        };
        cur.slices.push(SliceWork { rbsp, hdr, sps, pps, ls, refs, cur_poc, col, recon });
        Ok(())
    }

    /// Reference lists for a slice, with missing entries concealed by an existing picture (or a grey one).
    fn make_ref_lists(&mut self, hdr: &SliceHeader, sps: &Sps, cur_poc: i32) -> [Vec<slice::RefInfo>; 2] {
        if hdr.slice_type.is_intra() {
            return [Vec::new(), Vec::new()];
        }
        let lists = dpb::build_ref_lists(&self.dpb, hdr, sps, cur_poc);
        let any = lists.iter().flat_map(|l| l.iter().flatten()).next().copied();
        let needs_fallback = lists.iter().any(|l| l.iter().any(|e| e.is_none()));
        let fallback = if !needs_fallback {
            0
        } else if let Some(i) = any.or_else(|| (0..self.dpb.len()).max_by_key(|&i| self.dpb[i].decode_order))
        {
            i
        } else {
            let mut p = Picture::new(sps.width_mbs(), sps.height_mbs());
            p.motion = MotionSlot::filled(Motion::new(sps.width_mbs(), sps.height_mbs()));
            p.uid = self.next_uid;
            self.next_uid = self.next_uid.wrapping_add(1).max(1);
            self.emit(ReconEvent::Gap {
                uids: alloc::vec![p.uid],
                mbw: sps.width_mbs(),
                mbh: sps.height_mbs(),
                template: None,
            });
            p.ref_state = RefState::Short;
            p.non_existing = true;
            p.frame_num = hdr.frame_num.wrapping_sub(1);
            p.poc = cur_poc;
            self.dpb.push(p);
            self.dpb.len() - 1
        };
        let info = |i: usize| slice::RefInfo {
            dpb_idx: i,
            poc: self.dpb[i].poc,
            uid: self.dpb[i].uid,
            long: self.dpb[i].ref_state == RefState::Long,
        };
        let mut out: [Vec<slice::RefInfo>; 2] = [Vec::new(), Vec::new()];
        for l in 0..2 {
            out[l] = lists[l].iter().map(|e| info(e.unwrap_or(fallback))).collect();
        }
        out
    }

    fn release(&mut self, p: Picture) {
        self.emit(ReconEvent::Free(p.uid));
    }

    /// Make `sps` the active sequence parameter set, flushing if the picture format changed.
    fn activate(&mut self, sps: &Arc<Sps>) {
        let same = match &self.sps {
            Some(a) => **a == **sps,
            None => false,
        };
        if !same {
            let size_changed = match &self.sps {
                Some(a) => {
                    a.width_mbs() != sps.width_mbs()
                        || a.height_mbs() != sps.height_mbs()
                        || a.max_num_ref_frames != sps.max_num_ref_frames
                }
                None => true,
            };
            if size_changed {
                while self.bump() {}
                self.dpb.clear();
                self.emit(ReconEvent::Reset);
                self.poc = PocState::default();
                self.prev_ref_frame_num = 0;
                self.max_long_term_idx = -1;
            }
        }
        self.sps = Some(sps.clone());
    }

    fn start_picture(&mut self, hdr: &SliceHeader, sps: &Arc<Sps>, pts: i64) -> Result<()> {
        self.activate(sps);
        let (mbw, mbh) = (sps.width_mbs(), sps.height_mbs());
        let max_frame_num = 1u32 << sps.log2_max_frame_num;
        // Gaps in frame_num: insert "non-existing" frames (also used to conceal lost pictures).
        if !hdr.idr
            && hdr.frame_num != self.prev_ref_frame_num
            && hdr.frame_num != (self.prev_ref_frame_num + 1) % max_frame_num
        {
            self.fill_frame_num_gap(sps, hdr.frame_num, max_frame_num);
        }
        let poc = self.poc.compute(sps, hdr);
        let mut pic = Picture::new(mbw, mbh);
        pic.uid = self.next_uid;
        self.next_uid = self.next_uid.wrapping_add(1).max(1);
        pic.poc = poc.poc;
        pic.frame_num = hdr.frame_num;
        pic.pts = pts;
        self.decode_counter += 1;
        pic.decode_order = self.decode_counter;
        self.cur = Some(CurPic {
            pic,
            sps: sps.clone(),
            hdr: hdr.clone(),
            poc,
            slices: Vec::new(),
            saw_first_mb: false,
            marking: hdr.dec_ref_pic_marking.clone(),
            pts,
        });
        Ok(())
    }

    fn fill_frame_num_gap(&mut self, sps: &Arc<Sps>, frame_num: u32, max_frame_num: u32) {
        let (mbw, mbh) = (sps.width_mbs(), sps.height_mbs());
        let count = (frame_num + max_frame_num - self.prev_ref_frame_num - 1) % max_frame_num;
        let count = count.min(sps.max_num_ref_frames.max(1));
        // Only the last `count` missing frame numbers matter for the reference set.
        let start = (frame_num + max_frame_num - count) % max_frame_num;
        let template = self.dpb.iter().filter(|p| p.is_ref()).max_by_key(|p| p.decode_order).map(|p| p.uid);
        // All the copies are made at once: the loop below may drop the picture they copy from.
        let uids: Vec<i32> = (0..count)
            .map(|_| {
                let u = self.next_uid;
                self.next_uid = self.next_uid.wrapping_add(1).max(1);
                u
            })
            .collect();
        self.emit(ReconEvent::Gap { uids: uids.clone(), mbw, mbh, template });
        for k in 0..count {
            let fnum = (start + k) % max_frame_num;
            let mut pic = Picture::new(mbw, mbh);
            pic.motion = MotionSlot::filled(Motion::new(mbw, mbh));
            pic.uid = uids[k as usize];
            pic.frame_num = fnum;
            pic.non_existing = true;
            self.decode_counter += 1;
            pic.decode_order = self.decode_counter;
            // POC of a non-existing frame is only meaningful for POC types 1 and 2; keep the previous.
            pic.poc = self.dpb.iter().map(|p| p.poc).max().unwrap_or(0);
            let ctx = dpb::MarkCtx {
                max_refs: sps.max_num_ref_frames.max(1) as usize,
                frame_num: fnum,
                max_frame_num,
            };
            dpb::sliding_window(&mut self.dpb, &ctx);
            pic.ref_state = RefState::Short;
            self.dpb.push(pic);
            self.prune();
            self.prev_ref_frame_num = fnum;
            self.poc.prev_frame_num = fnum;
        }
    }

    /// Remove pictures that are neither references nor waiting for output.
    fn prune(&mut self) {
        let mut i = 0;
        while i < self.dpb.len() {
            if !self.dpb[i].is_ref() && !self.dpb[i].needed_for_output {
                let p = self.dpb.swap_remove(i);
                self.release(p);
            } else {
                i += 1;
            }
        }
    }

    /// Finish the current picture: conceal, deblock, mark, store and output.
    pub fn finish_picture(&mut self) -> Result<()> {
        let Some(mut cur) = self.cur.take() else { return Ok(()) };
        let sps = cur.sps.clone();
        // Macroblocks no slice covers are concealed by the reconstruction side, from the newest picture we hold.
        let conceal_src = self
            .dpb
            .iter()
            .filter(|p| p.is_ref() || p.needed_for_output)
            .max_by_key(|p| p.decode_order)
            .map(|p| p.uid);
        let slot = JobSlot::new();
        let job = ParseJob {
            uid: cur.pic.uid,
            mbw: sps.width_mbs(),
            mbh: sps.height_mbs(),
            slices: core::mem::take(&mut cur.slices),
            conceal_src,
            motion_slot: cur.pic.motion.clone(),
            out: slot.clone(),
            stats: self.stats.clone(),
            epoch: self.epoch.clone(),
            my_epoch: self.epoch.load(core::sync::atomic::Ordering::Acquire),
        };
        // Parsed on another thread if there is a runner; the reconstruction side waits for the result.
        let mut parse_err: Option<Error> = None;
        match &mut self.runner {
            Some(r) => r.spawn(Box::new(move || {
                let _ = job.run();
            })),
            None => parse_err = job.run().err(),
        }
        self.emit(ReconEvent::Picture(slot));
        self.stats.pictures.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        let h = &cur.hdr;
        let max_frame_num = 1u32 << sps.log2_max_frame_num;
        let mut mmco5 = false;
        let mut pic = cur.pic;
        pic.needed_for_output = true;
        pic.pts = cur.pts;
        if h.nal_ref_idc != 0 {
            let marking = cur.marking.clone().unwrap_or_default();
            let ctx = dpb::MarkCtx {
                max_refs: sps.max_num_ref_frames.max(1) as usize,
                frame_num: h.frame_num,
                max_frame_num,
            };
            mmco5 = dpb::mark_current(
                &mut self.dpb,
                &mut pic,
                &marking,
                h.idr,
                &ctx,
                &mut self.max_long_term_idx,
            );
        } else {
            pic.ref_state = RefState::Unused;
        }
        // POC state for the next picture.
        let mut poc_top = cur.poc.top;
        if mmco5 {
            let t = cur.poc.top.min(cur.poc.bottom);
            poc_top = cur.poc.top - t;
            pic.poc = 0;
            pic.frame_num = 0;
        }
        self.poc.prev_frame_num = if mmco5 { 0 } else { h.frame_num };
        self.poc.prev_frame_num_offset = if mmco5 { 0 } else { cur.poc.frame_num_offset };
        if h.nal_ref_idc != 0 {
            self.prev_ref_frame_num = if mmco5 { 0 } else { h.frame_num };
            if sps.poc_type == 0 {
                if mmco5 {
                    self.poc.prev_msb = 0;
                    self.poc.prev_lsb = poc_top;
                } else {
                    self.poc.prev_msb = cur.poc.top - h.pic_order_cnt_lsb as i32;
                    self.poc.prev_lsb = h.pic_order_cnt_lsb as i32;
                }
            }
        }
        // Output. Pictures that are no longer references and were already output free their slots.
        if h.idr || mmco5 {
            while self.bump() {}
        }
        self.prune();
        let dpb_size = sps.dpb_frames();
        if !pic.is_ref() {
            // A non-reference picture that would be output first goes out immediately when there is no room.
            let waiting_lower = self.dpb.iter().any(|p| p.needed_for_output && p.poc < pic.poc);
            if self.dpb.len() >= dpb_size && !waiting_lower {
                let ev = output_event(&pic, &sps);
                self.emit(ev);
                pic.needed_for_output = false;
                self.release(pic);
                self.note_parse_error(parse_err);
                return Ok(());
            }
        }
        while self.dpb.len() >= dpb_size {
            if !self.bump() {
                break;
            }
        }
        self.dpb.push(pic);
        let limit = sps.reorder_frames();
        while self.dpb.iter().filter(|p| p.needed_for_output).count() > limit {
            if !self.bump() {
                break;
            }
        }
        self.note_parse_error(parse_err);
        Ok(())
    }

    /// Remember the first error of a picture parsed inline, for the caller of `decode_sample` / `decode_annexb` /
    /// `flush` (parsing on other threads cannot report one).
    fn note_parse_error(&mut self, e: Option<Error>) {
        if self.parse_error.is_none() {
            self.parse_error = e;
        }
    }

    /// Output the waiting picture with the smallest POC. Returns false if none is waiting.
    fn bump(&mut self) -> bool {
        let Some(i) =
            (0..self.dpb.len()).filter(|&i| self.dpb[i].needed_for_output).min_by_key(|&i| self.dpb[i].poc)
        else {
            return false;
        };
        let sps = self.sps.clone();
        if let Some(sps) = sps {
            let ev = output_event(&self.dpb[i], &sps);
            self.emit(ev);
        }
        self.dpb[i].needed_for_output = false;
        if !self.dpb[i].is_ref() {
            let p = self.dpb.swap_remove(i);
            self.release(p);
        }
        true
    }
}

/// The event that makes the reconstruction side emit `pic` as an output frame.
fn output_event(pic: &Picture, sps: &Sps) -> ReconEvent {
    let (full_range, matrix) = match &sps.vui {
        Some(v) => (v.full_range, v.colour.map(|c| c.2).unwrap_or(2)),
        None => (false, 2),
    };
    ReconEvent::Output(OutputInfo {
        uid: pic.uid,
        crop: sps.crop_rect(),
        pts: pic.pts,
        poc: pic.poc,
        full_range,
        matrix,
    })
}
