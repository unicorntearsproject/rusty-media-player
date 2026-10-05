//! The reconstruction side of the decoder: everything that touches samples. The parsing side (`Decoder`, `SliceDecoder`)
//! turns NAL units into [`ReconEvent`]s (a [`PicJob`] per picture, plus output and release notices); a
//! [`Reconstructor`] consumes them in order, predicts and adds the residual of every macroblock, conceals what no slice
//! covered, deblocks, and produces the output frames.
//!
//! The two sides share nothing but the events, so the reconstructor can run on a thread of its own: while it works on
//! picture N, the parser is already reading picture N + 1 (see [`ReconExecutor`]).
use super::Frame;
use super::deblock::{self, SliceFilter};
use super::inter::{self, PSTRIDE, Weights};
use super::intra::{self, AV_LEFT, AV_TOP, AV_TOPLEFT};
use super::mbinfo::*;
use super::parse::JobSlot;
use super::picture::{Motion, PlanePic};
use super::slice::RefInfo;
use crate::error::Result;
use crate::params::PredWeightTable;
use crate::transform as tr;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use rvp_core::par::SpinLock;

/// What the reconstruction of a slice's macroblocks needs to know about the slice.
pub struct SliceRecon {
    /// `RefPicList0` and `RefPicList1`; `RefInfo::uid` names the picture (the index field is resolved on arrival).
    pub(crate) refs: [Vec<RefInfo>; 2],
    /// `PicOrderCnt` of the current picture.
    pub(crate) cur_poc: i32,
    /// 0 default weighting, 1 explicit, 2 implicit.
    pub(crate) weight_mode: u8,
    /// Explicit weights, when `weight_mode` is 1.
    pub(crate) pwt: Option<PredWeightTable>,
    /// `constrained_intra_pred_flag`.
    pub(crate) constrained_intra: bool,
    /// Deblocking parameters.
    pub(crate) filter: SliceFilter,
}

/// A parsed picture, ready to be reconstructed.
pub struct PicJob {
    pub(crate) uid: i32,
    pub(crate) mbw: usize,
    pub(crate) mbh: usize,
    /// True if the decoder was reset while this picture was being parsed: there is nothing to reconstruct.
    pub(crate) cancelled: bool,
    pub(crate) mbs: Vec<MbInfo>,
    pub(crate) motion: Arc<Motion>,
    pub(crate) slices: Vec<SliceRecon>,
    /// Scaled coefficients of the blocks flagged in `MbInfo::blk_nz`, in macroblock order.
    pub(crate) coefs: Vec<i16>,
    /// Samples of I_PCM macroblocks.
    pub(crate) pcm: Vec<u8>,
    /// Picture whose samples conceal macroblocks no slice covered (grey if none).
    pub(crate) conceal_src: Option<i32>,
}

/// Where an output frame comes from.
pub struct OutputInfo {
    pub(crate) uid: i32,
    /// Crop rectangle `(x, y, w, h)` in luma samples.
    pub(crate) crop: (usize, usize, usize, usize),
    pub(crate) pts: i64,
    pub(crate) poc: i32,
    pub(crate) full_range: bool,
    pub(crate) matrix: u8,
}

/// A message from the parsing side to the reconstruction side. All of them are processed in the order sent.
pub enum ReconEvent {
    /// Reconstruct and deblock a picture (once its parsing has finished), then keep it as a reference candidate under
    /// its `uid`.
    Picture(Arc<JobSlot>),
    /// A "non-existing" frame of a `frame_num` gap: a copy of the picture `template` (grey if `None`).
    Gap {
        /// Ids of the new pictures.
        uids: Vec<i32>,
        /// Width in macroblocks.
        mbw: usize,
        /// Height in macroblocks.
        mbh: usize,
        /// Picture to copy.
        template: Option<i32>,
    },
    /// Emit the picture `uid` as an output frame.
    Output(OutputInfo),
    /// The picture `uid` is no longer needed.
    Free(i32),
    /// Forget every picture (seek, or a change of picture size).
    Reset,
}

/// Runs reconstruction somewhere else than the parsing thread. A `Decoder` without one reconstructs inline.
pub trait ReconExecutor {
    /// Queue an event. May wait if too many pictures are already queued.
    fn submit(&mut self, ev: ReconEvent);
    /// Move the frames finished so far to the back of `out`.
    fn take_frames(&mut self, out: &mut VecDeque<Frame>);
    /// Events queued or running.
    fn pending(&self) -> usize;
    /// Wait until every queued event has been processed.
    fn wait_idle(&mut self);
}

/// Consumes [`ReconEvent`]s. Owns the picture samples.
#[derive(Default)]
pub struct Reconstructor {
    dpb: Vec<PlanePic>,
    pool: Vec<PlanePic>,
    /// Scratch for the scaled coefficients of one macroblock.
    coef: Option<Box<[i32; 384]>>,
}

impl Reconstructor {
    /// A reconstructor holding no pictures.
    pub fn new() -> Self {
        Self::default()
    }

    /// Process one event; finished output frames are appended to `out`.
    pub fn handle(&mut self, ev: ReconEvent, out: &mut VecDeque<Frame>) {
        match ev {
            ReconEvent::Picture(slot) => {
                let job = slot.wait();
                if !job.cancelled {
                    self.picture(*job);
                }
            }
            ReconEvent::Gap { uids, mbw, mbh, template } => {
                let planes =
                    template.and_then(|t| self.dpb.iter().find(|q| q.uid == t)).map(|t| t.planes.clone());
                for uid in uids {
                    let mut p = self.alloc(mbw, mbh);
                    p.uid = uid;
                    match &planes {
                        Some(t) => p.planes = t.clone(),
                        None => p.planes.iter_mut().for_each(|pl| pl.fill(128)),
                    }
                    self.dpb.push(p);
                }
            }
            ReconEvent::Output(o) => {
                if let Some(p) = self.dpb.iter().find(|p| p.uid == o.uid) {
                    out.push_back(make_frame(p, &o));
                }
            }
            ReconEvent::Free(uid) => {
                if let Some(i) = self.dpb.iter().position(|p| p.uid == uid) {
                    let p = self.dpb.swap_remove(i);
                    self.release(p);
                }
            }
            ReconEvent::Reset => {
                let old = core::mem::take(&mut self.dpb);
                self.pool.clear();
                // Same-size buffers are worth keeping for the next picture.
                self.pool.extend(old.into_iter().take(4));
            }
        }
    }

    fn alloc(&mut self, mbw: usize, mbh: usize) -> PlanePic {
        while let Some(p) = self.pool.pop() {
            if p.fits(mbw, mbh) {
                return p;
            }
        }
        PlanePic::new(mbw, mbh)
    }

    fn release(&mut self, p: PlanePic) {
        if self.pool.len() < 4 {
            self.pool.push(p);
        }
    }

    fn picture(&mut self, mut job: PicJob) {
        let mut pic = self.alloc(job.mbw, job.mbh);
        pic.uid = job.uid;
        let (mbw, mbh) = (job.mbw, job.mbh);
        let strides = pic.strides;
        // Phase A: inter macroblocks, in bands of macroblock rows on the pool. They read only reference pictures and write
        // only their own samples, so the bands are independent.
        let threads = rvp_core::par::threads();
        let band_rows = if threads > 1 && mbw * mbh >= 1024 { mbh.div_ceil(threads * 2).max(2) } else { mbh };
        {
            let [p0, p1, p2] = &mut pic.planes;
            let dpb = &self.dpb;
            let (slices, motion, coefs, pcm) = (&job.slices, &*job.motion, &job.coefs[..], &job.pcm[..]);
            type Band<'p> = ([&'p mut [u8]; 3], &'p mut [MbInfo]);
            let bands: Vec<SpinLock<Option<Band<'_>>>> = p0
                .chunks_mut(16 * strides[0] * band_rows)
                .zip(p1.chunks_mut(8 * strides[1] * band_rows))
                .zip(p2.chunks_mut(8 * strides[2] * band_rows))
                .zip(job.mbs.chunks_mut(mbw * band_rows))
                .map(|(((a, b), c), m)| SpinLock::new(Some(([a, b, c], m))))
                .collect();
            rvp_core::par::for_each(bands.len(), &|i| {
                let Some((planes, mbs)) = bands[i].lock().take() else { return };
                let Some(first) = slices.first() else { return };
                let mut coef = Box::new([0i32; 384]);
                let first_mb = i * band_rows * mbw;
                let r0 = i * band_rows;
                // The slice and flags of each macroblock, read before the reconstructor borrows the band's array.
                let ids: Vec<(u16, u16)> = mbs.iter().map(|m| (m.slice, m.flags)).collect();
                let mut mr = MbRecon::new(
                    planes,
                    strides,
                    [r0 * 16, r0 * 8, r0 * 8],
                    mbs,
                    first_mb,
                    motion,
                    dpb,
                    first,
                    coefs,
                    pcm,
                    mbw,
                    mbh,
                    &mut coef,
                );
                let mut cur_slice = 1u16;
                for (k, &(s, flags)) in ids.iter().enumerate() {
                    if s == 0 || flags & F_INTRA != 0 {
                        continue;
                    }
                    if s != cur_slice {
                        cur_slice = s;
                        mr.set_slice(&slices[s as usize - 1]);
                    }
                    let _ = mr.recon_mb(first_mb + k);
                }
            });
        }
        // Phase B: intra and PCM macroblocks in raster order (they read their neighbours' samples).
        {
            let ids: Vec<(u16, u16)> = job.mbs.iter().map(|m| (m.slice, m.flags)).collect();
            if !job.slices.is_empty() && ids.iter().any(|&(s, f)| s != 0 && f & F_INTRA != 0) {
                let mut coef = self.coef.take().unwrap_or_else(|| Box::new([0; 384]));
                let [p0, p1, p2] = &mut pic.planes;
                let mut mr = MbRecon::new(
                    [&mut p0[..], &mut p1[..], &mut p2[..]],
                    strides,
                    [0; 3],
                    &mut job.mbs,
                    0,
                    &job.motion,
                    &self.dpb,
                    &job.slices[0],
                    &job.coefs,
                    &job.pcm,
                    mbw,
                    mbh,
                    &mut coef,
                );
                let mut cur_slice = 1u16;
                for (addr, &(s, flags)) in ids.iter().enumerate() {
                    if s == 0 || flags & F_INTRA == 0 {
                        continue;
                    }
                    if s != cur_slice {
                        cur_slice = s;
                        mr.set_slice(&job.slices[s as usize - 1]);
                    }
                    // A macroblock whose prediction mode is impossible for its neighbours is left as it is.
                    let _ = mr.recon_mb(addr);
                }
                drop(mr);
                self.coef = Some(coef);
            }
        }
        // Conceal macroblocks no slice covered.
        if job.mbs.iter().any(|m| m.slice == 0) {
            let src = job.conceal_src.and_then(|u| self.dpb.iter().find(|p| p.uid == u));
            conceal(&mut pic, &job.mbs, src, job.mbw);
        }
        let filters: Vec<SliceFilter> = job.slices.iter().map(|s| s.filter).collect();
        deblock::deblock_picture(&mut pic, &job.motion, &job.mbs, &filters);
        self.dpb.push(pic);
    }
}

/// Fill macroblocks that were never decoded from `src` (or leave them grey).
fn conceal(pic: &mut PlanePic, mbs: &[MbInfo], src: Option<&PlanePic>, mbw: usize) {
    for (addr, m) in mbs.iter().enumerate() {
        if m.slice != 0 {
            continue;
        }
        let (mx, my) = (addr % mbw, addr / mbw);
        for p in 0..3 {
            let (size, stride) = if p == 0 { (16, pic.strides[0]) } else { (8, pic.strides[p]) };
            for y in 0..size {
                let o = (my * size + y) * stride + mx * size;
                match src {
                    Some(s) => {
                        let (dst, from) = (&mut pic.planes[p][o..o + size], &s.planes[p][o..o + size]);
                        dst.copy_from_slice(from);
                    }
                    None => pic.planes[p][o..o + size].fill(128),
                }
            }
        }
    }
}

/// Crop a picture into an output frame.
fn make_frame(pic: &PlanePic, o: &OutputInfo) -> Frame {
    let (cx, cy, cw, ch) = o.crop;
    let mut planes: [Vec<u8>; 3] = [Vec::with_capacity(cw * ch), Vec::new(), Vec::new()];
    for y in 0..ch {
        let at = (cy + y) * pic.strides[0] + cx;
        planes[0].extend_from_slice(&pic.planes[0][at..at + cw]);
    }
    let (ccx, ccy, ccw, cch) = (cx / 2, cy / 2, cw.div_ceil(2), ch.div_ceil(2));
    for p in 1..3 {
        planes[p].reserve(ccw * cch);
        for y in 0..cch {
            let at = (ccy + y) * pic.strides[p] + ccx;
            planes[p].extend_from_slice(&pic.planes[p][at..at + ccw]);
        }
    }
    Frame {
        width: cw,
        height: ch,
        planes,
        strides: [cw, ccw, ccw],
        pts: o.pts,
        poc: o.poc,
        full_range: o.full_range,
        matrix_coefficients: o.matrix,
    }
}

/// Reconstructs macroblocks of one slice of a picture, into a band of rows of the picture (or all of it).
pub(crate) struct MbRecon<'a> {
    /// The samples of the band: Y, Cb and Cr from picture line `row0[c]` on.
    planes: [&'a mut [u8]; 3],
    strides: [usize; 3],
    row0: [usize; 3],
    motion: &'a Motion,
    /// The macroblock infos of the band; `mb_off` is the address of the first.
    mbs: &'a mut [MbInfo],
    mb_off: usize,
    /// The pictures `refs` index into.
    dpb: &'a [PlanePic],
    refs: [Vec<RefInfo>; 2],
    slice: &'a SliceRecon,
    coefs: &'a [i16],
    pcm: &'a [u8],
    mbw: usize,
    #[allow(dead_code)]
    mbh: usize,
    /// Scaled coefficients of the current macroblock: luma 16 blocks of 16 (or 4 of 64), then Cb and Cr 4x16.
    coef: &'a mut [i32; 384],
    /// Bit per luma block (raster), chroma block (16..24) and 8x8 block (24..28) that has coefficients to add.
    blk_nz: u32,
    mb_x: usize,
    mb_y: usize,
    mb_addr: usize,
    /// Neighbouring macroblocks A (left), B (above), C (above right), D (above left), if available.
    na: Option<usize>,
    nb: Option<usize>,
    nc: Option<usize>,
    nd: Option<usize>,
}

const COEF_CB: usize = 256;

impl<'a> MbRecon<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        planes: [&'a mut [u8]; 3],
        strides: [usize; 3],
        row0: [usize; 3],
        mbs: &'a mut [MbInfo],
        mb_off: usize,
        motion: &'a Motion,
        dpb: &'a [PlanePic],
        slice: &'a SliceRecon,
        coefs: &'a [i16],
        pcm: &'a [u8],
        mbw: usize,
        mbh: usize,
        coef: &'a mut [i32; 384],
    ) -> Self {
        let refs = Self::resolve(dpb, slice);
        Self {
            planes,
            strides,
            row0,
            motion,
            mbs,
            mb_off,
            dpb,
            refs,
            slice,
            coefs,
            pcm,
            mbw,
            mbh,
            coef,
            blk_nz: 0,
            mb_x: 0,
            mb_y: 0,
            mb_addr: 0,
            na: None,
            nb: None,
            nc: None,
            nd: None,
        }
    }

    /// The reference lists of `slice` name pictures by id; find them in `dpb`.
    fn resolve(dpb: &[PlanePic], slice: &SliceRecon) -> [Vec<RefInfo>; 2] {
        let r = |r: &RefInfo| RefInfo { dpb_idx: dpb.iter().position(|p| p.uid == r.uid).unwrap_or(0), ..*r };
        [slice.refs[0].iter().map(r).collect(), slice.refs[1].iter().map(r).collect()]
    }

    /// Continue with the macroblocks of another slice.
    fn set_slice(&mut self, slice: &'a SliceRecon) {
        self.refs = Self::resolve(self.dpb, slice);
        self.slice = slice;
    }

    #[inline]
    fn intra_ok(&self, n: Option<usize>) -> bool {
        match n {
            Some(a) => !self.slice.constrained_intra || self.mbs[a - self.mb_off].is_intra(),
            None => false,
        }
    }

    /// Reconstruct macroblock `addr`.
    fn recon_mb(&mut self, addr: usize) -> Result<()> {
        self.mb_addr = addr;
        self.mb_x = addr % self.mbw;
        self.mb_y = addr / self.mbw;
        let m = self.mbs[addr - self.mb_off];
        if m.flags & F_INTRA == 0 {
            // Inter macroblocks need no neighbours.
            self.load_coefs(&m);
            self.mc_mb()?;
            self.add_inter_residual(m.flags & F_T8X8 != 0);
            return Ok(());
        }
        let (x, y, w) = (self.mb_x, self.mb_y, self.mbw);
        let s = m.slice;
        let ok = |mbs: &[MbInfo], a: usize| mbs[a - self.mb_off].slice == s;
        self.na = (x > 0 && ok(self.mbs, addr - 1)).then(|| addr - 1);
        self.nb = (y > 0 && ok(self.mbs, addr - w)).then(|| addr - w);
        self.nc = (y > 0 && x + 1 < w && ok(self.mbs, addr - w + 1)).then(|| addr - w + 1);
        self.nd = (y > 0 && x > 0 && ok(self.mbs, addr - w - 1)).then(|| addr - w - 1);
        if m.flags & F_PCM != 0 {
            return self.recon_pcm(m.coef_off as usize);
        }
        self.load_coefs(&m);
        if m.flags & F_I16 != 0 {
            self.recon_intra16(m.i16_mode)?;
        } else {
            self.recon_intra_nxn(m.flags & F_T8X8 != 0)?;
        }
        self.recon_chroma_intra(m.chroma_mode)
    }

    /// Unpack the macroblock's coefficient blocks from the store into `coef`.
    fn load_coefs(&mut self, m: &MbInfo) {
        self.blk_nz = m.blk_nz;
        let mut off = m.coef_off as usize;
        let mut bits = m.blk_nz;
        while bits != 0 {
            let b = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let (base, n) = if b < 16 {
                (b * 16, 16)
            } else if b < 24 {
                (COEF_CB + (b - 16) * 16, 16)
            } else {
                ((b - 24) * 64, 64)
            };
            for (d, s) in self.coef[base..base + n].iter_mut().zip(&self.coefs[off..off + n]) {
                *d = *s as i32;
            }
            off += n;
        }
    }

    fn recon_pcm(&mut self, off: usize) -> Result<()> {
        let buf = &self.pcm[off..off + 384];
        let (mx, my) = (self.mb_x, self.mb_y);
        let ys = self.strides[0];
        for y in 0..16 {
            let o = (my * 16 + y) * ys + mx * 16;
            self.planes[0][o..o + 16].copy_from_slice(&buf[y * 16..y * 16 + 16]);
        }
        for c in 0..2 {
            let cs = self.strides[1 + c];
            for y in 0..8 {
                let o = (my * 8 + y) * cs + mx * 8;
                let s = 256 + c * 64 + y * 8;
                self.planes[1 + c][o..o + 8].copy_from_slice(&buf[s..s + 8]);
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------------------
    // Inter prediction

    /// True if all 4x4 blocks of the rectangle share the same motion.
    fn uniform(&self, bx: usize, by: usize, w: usize, h: usize) -> bool {
        let w4 = self.mbw * 4;
        let key = |x: usize, y: usize| {
            let (gx, gy) = (self.mb_x * 4 + x, self.mb_y * 4 + y);
            let i8 = (gy >> 1) * (self.mbw * 2) + (gx >> 1);
            (
                self.motion.ref_idx[0][i8],
                self.motion.ref_idx[1][i8],
                self.motion.mv[0][gy * w4 + gx],
                self.motion.mv[1][gy * w4 + gx],
            )
        };
        let k0 = key(bx, by);
        (by..by + h).all(|y| (bx..bx + w).all(|x| key(x, y) == k0))
    }

    /// Motion-compensate the whole macroblock into the current picture.
    pub(crate) fn mc_mb(&mut self) -> Result<()> {
        if self.uniform(0, 0, 4, 4) {
            self.mbs[self.mb_addr - self.mb_off].flags |= F_UNIFORM;
            self.mc_block(0, 0, 4, 4);
            return Ok(());
        }
        for q in 0..4 {
            let (qx, qy) = ((q & 1) * 2, (q >> 1) * 2);
            if self.uniform(qx, qy, 2, 2) {
                self.mc_block(qx, qy, 2, 2);
            } else {
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    self.mc_block(qx + dx, qy + dy, 1, 1);
                }
            }
        }
        Ok(())
    }

    /// Weighted-prediction parameters for component `comp` (0 luma, 1 Cb, 2 Cr) and the references in use.
    fn weights(&self, comp: usize, r: [i32; 2]) -> Option<Weights> {
        match self.slice.weight_mode {
            1 => {
                let t = self.slice.pwt.as_ref()?;
                let mut wt = Weights {
                    log_wd: if comp == 0 { t.luma_log2_denom } else { t.chroma_log2_denom },
                    w: [0; 2],
                    o: [0; 2],
                };
                for l in 0..2 {
                    if r[l] >= 0 {
                        let e = t.entries[l].get(r[l] as usize)?;
                        if comp == 0 {
                            wt.w[l] = e.luma_weight;
                            wt.o[l] = e.luma_offset;
                        } else {
                            wt.w[l] = e.chroma_weight[comp - 1];
                            wt.o[l] = e.chroma_offset[comp - 1];
                        }
                    }
                }
                Some(wt)
            }
            2 if r[0] >= 0 && r[1] >= 0 => {
                let w1 = self.implicit_w1(r[0] as usize, r[1] as usize);
                Some(Weights { log_wd: 5, w: [64 - w1, w1], o: [0, 0] })
            }
            _ => None,
        }
    }

    /// Implicit bi-prediction weight `w1` for reference indices `(i0, i1)` (8.4.2.3.1).
    fn implicit_w1(&self, i0: usize, i1: usize) -> i32 {
        let (a, b): (&RefInfo, &RefInfo) = (&self.refs[0][i0], &self.refs[1][i1]);
        if a.long || b.long {
            return 32;
        }
        let tb = (self.slice.cur_poc - a.poc).clamp(-128, 127);
        let td = (b.poc - a.poc).clamp(-128, 127);
        if td == 0 {
            return 32;
        }
        let tx = (16384 + (td / 2).abs()) / td;
        let dsf = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
        let w1 = dsf >> 2;
        if !(-64..=128).contains(&w1) { 32 } else { w1 }
    }

    /// Motion-compensate one block of `w` x `h` 4x4 units at `(bx, by)` of the current macroblock.
    fn mc_block(&mut self, bx: usize, by: usize, w4: usize, h4: usize) {
        let w4g = self.mbw * 4;
        let (gx, gy) = (self.mb_x * 4 + bx, self.mb_y * 4 + by);
        let i8 = (gy >> 1) * (self.mbw * 2) + (gx >> 1);
        let r = [self.motion.ref_idx[0][i8] as i32, self.motion.ref_idx[1][i8] as i32];
        let mvs = [self.motion.mv[0][gy * w4g + gx], self.motion.mv[1][gy * w4g + gx]];
        let (x, y) = ((gx * 4) as i32, (gy * 4) as i32);
        let (w, h) = (w4 * 4, h4 * 4);
        let dpb = self.dpb;
        let (pw, ph) = ((self.mbw * 16) as i32, (self.mbh * 16) as i32);
        // One list and no weights: the prediction is the block, so interpolate straight into the picture.
        let single = match (r[0] >= 0, r[1] >= 0) {
            (true, false) => Some(0),
            (false, true) => Some(1),
            _ => None,
        };
        let mut pred = [[0u8; PSTRIDE * 16]; 2];
        // Luma.
        let wt = self.weights(0, r);
        let direct = single.filter(|_| wt.is_none());
        let stride = self.strides[0];
        for l in 0..2 {
            if r[l] < 0 {
                continue;
            }
            let rp = &dpb[self.refs[l][r[l] as usize].dpb_idx];
            let mv = mvs[l];
            let (mx, my) = (x + (mv[0] as i32 >> 2), y + (mv[1] as i32 >> 2));
            let (fx, fy) = ((mv[0] & 3) as usize, (mv[1] & 3) as usize);
            if direct.is_some() {
                let dst = &mut self.planes[0][(y as usize - self.row0[0]) * stride + x as usize..];
                inter::mc_luma(&rp.planes[0], rp.strides[0], pw, ph, mx, my, fx, fy, w, h, dst, stride);
            } else {
                inter::mc_luma(
                    &rp.planes[0],
                    rp.strides[0],
                    pw,
                    ph,
                    mx,
                    my,
                    fx,
                    fy,
                    w,
                    h,
                    &mut pred[l],
                    PSTRIDE,
                );
            }
        }
        if direct.is_none() {
            let dst = &mut self.planes[0][(y as usize - self.row0[0]) * stride + x as usize..];
            let (p0, p1) = (
                if r[0] >= 0 { Some(&pred[0][..]) } else { None },
                if r[1] >= 0 { Some(&pred[1][..]) } else { None },
            );
            inter::combine(dst, stride, w, h, p0, p1, wt.as_ref());
        }
        // Chroma.
        let (cw, ch) = (w / 2, h / 2);
        let (cx, cy) = (x / 2, y / 2);
        for comp in 1..3 {
            let wt = self.weights(comp, r);
            let direct = single.filter(|_| wt.is_none());
            let stride = self.strides[comp];
            for l in 0..2 {
                if r[l] < 0 {
                    continue;
                }
                let rp = &dpb[self.refs[l][r[l] as usize].dpb_idx];
                let mv = mvs[l];
                let (mx, my) = (cx + (mv[0] as i32 >> 3), cy + (mv[1] as i32 >> 3));
                let (fx, fy) = ((mv[0] & 7) as i32, (mv[1] & 7) as i32);
                if direct.is_some() {
                    let dst =
                        &mut self.planes[comp][(cy as usize - self.row0[comp]) * stride + cx as usize..];
                    inter::mc_chroma(
                        &rp.planes[comp],
                        rp.strides[comp],
                        pw / 2,
                        ph / 2,
                        mx,
                        my,
                        fx,
                        fy,
                        cw,
                        ch,
                        dst,
                        stride,
                    );
                } else {
                    inter::mc_chroma(
                        &rp.planes[comp],
                        rp.strides[comp],
                        pw / 2,
                        ph / 2,
                        mx,
                        my,
                        fx,
                        fy,
                        cw,
                        ch,
                        &mut pred[l],
                        PSTRIDE,
                    );
                }
            }
            if direct.is_none() {
                let dst = &mut self.planes[comp][(cy as usize - self.row0[comp]) * stride + cx as usize..];
                let (p0, p1) = (
                    if r[0] >= 0 { Some(&pred[0][..]) } else { None },
                    if r[1] >= 0 { Some(&pred[1][..]) } else { None },
                );
                inter::combine(dst, stride, cw, ch, p0, p1, wt.as_ref());
            }
        }
    }

    /// Add the residual of an inter macroblock to its prediction.
    pub(crate) fn add_inter_residual(&mut self, t8: bool) {
        let (mx, my) = (self.mb_x * 16, self.mb_y * 16);
        if t8 {
            for i8 in 0..4 {
                self.add_luma_block8(i8, mx + (i8 & 1) * 8, my + (i8 >> 1) * 8);
            }
        } else {
            for r in 0..16 {
                self.add_luma_block(r, mx + (r & 3) * 4, my + (r >> 2) * 4);
            }
        }
        self.add_chroma_blocks();
    }

    // ---------------------------------------------------------------------------------------------------
    // Residual and intra prediction

    /// Apply the 4x4 residual of luma block `r` (raster index) at plane position `(x, y)`.
    pub(crate) fn add_luma_block(&mut self, r: usize, x: usize, y: usize) {
        if self.blk_nz & (1 << r) == 0 {
            return;
        }
        let stride = self.strides[0];
        let c = &mut self.coef[r * 16..r * 16 + 16];
        let dst = &mut self.planes[0][(y - self.row0[0]) * stride + x..];
        if c[1..].iter().all(|&v| v == 0) {
            tr::add_dc_4x4(c[0], dst, stride);
            c[0] = 0;
        } else {
            let mut b = [0i32; 16];
            b.copy_from_slice(c);
            tr::idct4x4(&mut b);
            tr::add_residual_4x4(&b, dst, stride);
            c.fill(0);
        }
    }

    /// Apply the 8x8 residual of luma block `i8` at plane position `(x, y)`.
    pub(crate) fn add_luma_block8(&mut self, i8: usize, x: usize, y: usize) {
        if self.blk_nz & (1 << (24 + i8)) == 0 {
            return;
        }
        let stride = self.strides[0];
        let c = &mut self.coef[i8 * 64..i8 * 64 + 64];
        let mut b = [0i32; 64];
        b.copy_from_slice(c);
        tr::idct8x8(&mut b);
        tr::add_residual_8x8(&b, &mut self.planes[0][(y - self.row0[0]) * stride + x..], stride);
        c.fill(0);
    }

    pub(crate) fn add_chroma_blocks(&mut self) {
        for comp in 0..2 {
            let stride = self.strides[1 + comp];
            for blk in 0..4 {
                if self.blk_nz & (1 << (16 + comp * 4 + blk)) == 0 {
                    continue;
                }
                let base = COEF_CB + comp * 64 + blk * 16;
                let (x, y) = (self.mb_x * 8 + (blk & 1) * 4, self.mb_y * 8 + (blk >> 1) * 4);
                let c = &mut self.coef[base..base + 16];
                let dst = &mut self.planes[1 + comp][(y - self.row0[1 + comp]) * stride + x..];
                if c[1..].iter().all(|&v| v == 0) {
                    tr::add_dc_4x4(c[0], dst, stride);
                } else {
                    let mut b = [0i32; 16];
                    b.copy_from_slice(c);
                    tr::idct4x4(&mut b);
                    tr::add_residual_4x4(&b, dst, stride);
                }
                c.fill(0);
            }
        }
    }

    fn recon_intra_nxn(&mut self, t8: bool) -> Result<()> {
        let stride = self.strides[0];
        let (mx, my) = (self.mb_x * 16, self.mb_y * 16);
        if t8 {
            for i8 in 0..4 {
                let (bx, by) = ((i8 & 1) * 2, (i8 >> 1) * 2);
                let (x, y) = (mx + bx * 4, my + by * 4);
                let mode = self.mbs[self.mb_addr - self.mb_off].ipm[by * 4 + bx] as u8;
                let (mut top, mut left, tl, avail) = self.gather_8x8(bx, by, x, y);
                let (ft, fl, ftl);
                (ft, fl, ftl) = intra::filter_8x8_refs(&top, &left, tl, avail);
                top = ft;
                left = fl;
                let dst = &mut self.planes[0][(y - self.row0[0]) * stride + x..];
                intra::predict_nxn::<8>(dst, stride, mode, &top, &left, ftl, avail)?;
                self.add_luma_block8(i8, x, y);
            }
        } else {
            for blk in 0..16 {
                let (bx, by) = blk_xy(blk);
                let (x, y) = (mx + bx * 4, my + by * 4);
                let mode = self.mbs[self.mb_addr - self.mb_off].ipm[by * 4 + bx] as u8;
                let (top, left, tl, avail) = self.gather_4x4(bx, by, x, y);
                let dst = &mut self.planes[0][(y - self.row0[0]) * stride + x..];
                intra::predict_nxn::<4>(dst, stride, mode, &top, &left, tl, avail)?;
                self.add_luma_block(by * 4 + bx, x, y);
            }
        }
        Ok(())
    }

    /// Neighbouring samples and availability for the 4x4 block at `(bx, by)`, picture position `(x, y)`.
    fn gather_4x4(&self, bx: usize, by: usize, x: usize, y: usize) -> ([u8; 8], [u8; 4], u8, u8) {
        let stride = self.strides[0];
        let p = &self.planes[0];
        let left_ok = bx > 0 || self.intra_ok(self.na);
        let top_ok = by > 0 || self.intra_ok(self.nb);
        let tl_ok = match (bx > 0, by > 0) {
            (true, true) => true,
            (false, true) => self.intra_ok(self.na),
            (true, false) => self.intra_ok(self.nb),
            (false, false) => self.intra_ok(self.nd),
        };
        let tr_ok = if by == 0 {
            if bx < 3 { self.intra_ok(self.nb) } else { self.intra_ok(self.nc) }
        } else if bx == 3 {
            false
        } else {
            blk_order(bx + 1, by - 1) < blk_order(bx, by)
        };
        let mut top = [128u8; 8];
        let mut left = [128u8; 4];
        let mut tl = 128;
        let mut avail = 0;
        if top_ok {
            avail |= AV_TOP;
            let o = (y - 1) * stride + x;
            top[..4].copy_from_slice(&p[o..o + 4]);
            if tr_ok {
                top[4..].copy_from_slice(&p[o + 4..o + 8]);
            } else {
                let v = top[3];
                top[4..].fill(v);
            }
        }
        if left_ok {
            avail |= AV_LEFT;
            for i in 0..4 {
                left[i] = p[(y + i) * stride + x - 1];
            }
        }
        if tl_ok {
            avail |= AV_TOPLEFT;
            tl = p[(y - 1) * stride + x - 1];
        }
        (top, left, tl, avail)
    }

    fn gather_8x8(&self, bx: usize, by: usize, x: usize, y: usize) -> ([u8; 16], [u8; 8], u8, u8) {
        let stride = self.strides[0];
        let p = &self.planes[0];
        let left_ok = bx > 0 || self.intra_ok(self.na);
        let top_ok = by > 0 || self.intra_ok(self.nb);
        let tl_ok = match (bx > 0, by > 0) {
            (true, true) => true,
            (false, true) => self.intra_ok(self.na),
            (true, false) => self.intra_ok(self.nb),
            (false, false) => self.intra_ok(self.nd),
        };
        // Top right: block 0 uses the macroblock above, block 1 the one above right, block 2 is inside, 3 is not available.
        let tr_ok = match (bx, by) {
            (0, 0) => self.intra_ok(self.nb),
            (2, 0) => self.intra_ok(self.nc),
            (0, 2) => true,
            _ => false,
        };
        let mut top = [128u8; 16];
        let mut left = [128u8; 8];
        let mut tl = 128;
        let mut avail = 0;
        if top_ok {
            avail |= AV_TOP;
            let o = (y - 1) * stride + x;
            top[..8].copy_from_slice(&p[o..o + 8]);
            if tr_ok {
                top[8..].copy_from_slice(&p[o + 8..o + 16]);
            } else {
                let v = top[7];
                top[8..].fill(v);
            }
        }
        if left_ok {
            avail |= AV_LEFT;
            for i in 0..8 {
                left[i] = p[(y + i) * stride + x - 1];
            }
        }
        if tl_ok {
            avail |= AV_TOPLEFT;
            tl = p[(y - 1) * stride + x - 1];
        }
        (top, left, tl, avail)
    }

    fn recon_intra16(&mut self, mode: u8) -> Result<()> {
        let stride = self.strides[0];
        let (mx, my) = (self.mb_x * 16, self.mb_y * 16);
        let top_ok = self.intra_ok(self.nb);
        let left_ok = self.intra_ok(self.na);
        let tl_ok = self.intra_ok(self.nd);
        let mut top = [128u8; 16];
        let mut left = [128u8; 16];
        let mut tl = 128;
        let mut avail = 0;
        {
            let p = &self.planes[0];
            if top_ok {
                avail |= AV_TOP;
                top.copy_from_slice(&p[(my - 1) * stride + mx..(my - 1) * stride + mx + 16]);
            }
            if left_ok {
                avail |= AV_LEFT;
                for i in 0..16 {
                    left[i] = p[(my + i) * stride + mx - 1];
                }
            }
            if tl_ok {
                avail |= AV_TOPLEFT;
                tl = p[(my - 1) * stride + mx - 1];
            }
        }
        intra::predict_16x16(&mut self.planes[0][my * stride + mx..], stride, mode, &top, &left, tl, avail)?;
        for r in 0..16 {
            self.add_luma_block(r, mx + (r & 3) * 4, my + (r >> 2) * 4);
        }
        Ok(())
    }

    fn recon_chroma_intra(&mut self, mode: u8) -> Result<()> {
        let top_ok = self.intra_ok(self.nb);
        let left_ok = self.intra_ok(self.na);
        let tl_ok = self.intra_ok(self.nd);
        let (mx, my) = (self.mb_x * 8, self.mb_y * 8);
        for comp in 0..2 {
            let stride = self.strides[1 + comp];
            let mut top = [128u8; 8];
            let mut left = [128u8; 8];
            let mut tl = 128;
            let mut avail = 0;
            {
                let p = &self.planes[1 + comp];
                if top_ok {
                    avail |= AV_TOP;
                    top.copy_from_slice(&p[(my - 1) * stride + mx..(my - 1) * stride + mx + 8]);
                }
                if left_ok {
                    avail |= AV_LEFT;
                    for i in 0..8 {
                        left[i] = p[(my + i) * stride + mx - 1];
                    }
                }
                if tl_ok {
                    avail |= AV_TOPLEFT;
                    tl = p[(my - 1) * stride + mx - 1];
                }
            }
            intra::predict_chroma(
                &mut self.planes[1 + comp][my * stride + mx..],
                stride,
                mode,
                &top,
                &left,
                tl,
                avail,
            )?;
        }
        self.add_chroma_blocks();
        Ok(())
    }
}
