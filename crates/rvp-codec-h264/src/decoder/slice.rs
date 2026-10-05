//! Slice decoding: the macroblock loop, syntax parsing, residual decoding and reconstruction.
use super::cabac_syntax::Cabac;
use super::entropy::{Cat, Cavlc, Entropy};
use super::mbinfo::*;
use super::picture::{Motion, MotionSlot};
use crate::bitstream::BitReader;
use crate::error::{Error, Result};
use crate::params::{Pps, SliceHeader, SliceType, Sps};
use crate::transform::{self as tr, LevelScale, ZIGZAG_4X4, ZIGZAG_8X8};
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Classified `mb_type`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum MbType {
    /// I_NxN (Intra 4x4 or, with the transform flag, Intra 8x8).
    INxN,
    /// Intra 16x16 with its prediction mode, luma cbp (0 or 15) and chroma cbp.
    I16 { mode: u8, cbp_luma: u8, cbp_chroma: u8 },
    /// I_PCM.
    IPcm,
}

/// One entry of a reference picture list, with what direct and weighted prediction need to know about it.
#[derive(Clone, Copy, Debug)]
pub struct RefInfo {
    /// Index into the DPB slice.
    pub dpb_idx: usize,
    /// `PicOrderCnt` of the reference frame.
    pub poc: i32,
    /// Unique frame id (see `Picture::uid`).
    pub uid: i32,
    /// Marked as long-term.
    pub long: bool,
}

/// Decodes the macroblocks of one slice into the current picture.
pub struct SliceDecoder<'a> {
    #[allow(dead_code)]
    pub(crate) sps: &'a Sps,
    pub(crate) pps: &'a Pps,
    pub(crate) hdr: &'a SliceHeader,
    pub(crate) ls: &'a LevelScale,
    /// The motion data of the picture being parsed.
    pub(crate) cur: &'a mut Motion,
    pub(crate) mbs: &'a mut [MbInfo],
    /// Where the motion of `RefPicList1[0]` (the co-located picture of direct prediction) appears, and the motion once
    /// it has been waited for.
    pub(crate) col_slot: Option<Arc<MotionSlot>>,
    pub(crate) col: Option<Arc<Motion>>,
    /// `RefPicList0` and `RefPicList1`.
    pub(crate) refs: [Vec<RefInfo>; 2],
    /// `PicOrderCnt` of the current picture.
    pub(crate) cur_poc: i32,
    pub(crate) mbw: usize,
    pub(crate) mbh: usize,
    pub(crate) slice_num: u16,
    pub(crate) slice_type: SliceType,
    pub(crate) constrained_intra: bool,
    /// Current `QPY`.
    pub(crate) qp: i32,
    pub(crate) mb_x: usize,
    pub(crate) mb_y: usize,
    pub(crate) mb_addr: usize,
    /// Neighbouring macroblocks A (left), B (above), C (above right), D (above left), if available.
    pub(crate) na: Option<usize>,
    pub(crate) nb: Option<usize>,
    pub(crate) nc: Option<usize>,
    pub(crate) nd: Option<usize>,
    /// CABAC context: the previous macroblock had a non-zero `mb_qp_delta`.
    pub(crate) prev_dqp_nonzero: bool,
    /// Scaled coefficients of the current macroblock: luma 16 blocks of 16 (or 4 of 64), then Cb and Cr 4x16.
    coef: Box<[i32; 384]>,
    /// Bit per luma block (raster) and chroma block (16..24) that has coefficients to add.
    blk_nz: u32,
    /// Where the scaled coefficients and PCM samples of the picture go.
    pub(crate) store: &'a mut Store,
}

/// The data the parsing side hands to reconstruction besides `MbInfo`: scaled coefficients of the blocks flagged in
/// `MbInfo::blk_nz` (in macroblock order) and the samples of I_PCM macroblocks.
#[derive(Default)]
pub struct Store {
    /// Coefficient blocks.
    pub coefs: Vec<i16>,
    /// PCM samples, 384 bytes per macroblock.
    pub pcm: Vec<u8>,
}

const COEF_CB: usize = 256;

impl<'a> SliceDecoder<'a> {
    /// Create a decoder for one slice.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sps: &'a Sps,
        pps: &'a Pps,
        hdr: &'a SliceHeader,
        ls: &'a LevelScale,
        cur: &'a mut Motion,
        mbs: &'a mut [MbInfo],
        col_slot: Option<Arc<MotionSlot>>,
        refs: [Vec<RefInfo>; 2],
        cur_poc: i32,
        slice_num: u16,
        store: &'a mut Store,
    ) -> Self {
        Self {
            col_slot,
            col: None,
            refs,
            cur_poc,
            sps,
            pps,
            hdr,
            ls,
            mbw: sps.width_mbs(),
            mbh: sps.height_mbs(),
            cur,
            mbs,
            slice_num,
            slice_type: hdr.slice_type,
            constrained_intra: pps.constrained_intra_pred,
            qp: hdr.slice_qp(pps),
            mb_x: 0,
            mb_y: 0,
            mb_addr: 0,
            na: None,
            nb: None,
            nc: None,
            nd: None,
            prev_dqp_nonzero: false,
            coef: Box::new([0; 384]),
            blk_nz: 0,
            store,
        }
    }

    // ---------------------------------------------------------------------------------------------------
    // Slice loops

    /// Decode a CAVLC slice. `r` is positioned at the start of `slice_data()`.
    pub fn decode_cavlc(&mut self, r: BitReader<'_>) -> Result<()> {
        let mut ent = Cavlc { r };
        let total = self.mbw * self.mbh;
        let mut addr = self.hdr.first_mb_in_slice as usize;
        let intra_slice = self.slice_type.is_intra();
        let mut skip_run: i64 = -1;
        loop {
            if addr >= total {
                return Err(Error::Invalid("slice runs past the end of the picture"));
            }
            if !intra_slice && skip_run < 0 {
                let run = ent.r.read_ue()? as i64;
                skip_run = run;
            }
            if skip_run > 0 {
                self.start_mb(addr);
                self.decode_skip_mb()?;
                skip_run -= 1;
                addr += 1;
                if skip_run == 0 {
                    if !ent.r.more_rbsp_data() {
                        return Ok(());
                    }
                    // The next macroblock is coded: no new skip run is read before it.
                } else if addr >= total {
                    return Ok(());
                }
                continue;
            }
            skip_run = -1;
            self.start_mb(addr);
            self.decode_mb(&mut ent)?;
            addr += 1;
            if ent.r.overrun() {
                return Err(Error::Truncated);
            }
            if !ent.r.more_rbsp_data() {
                return Ok(());
            }
        }
    }

    /// Decode a CABAC slice. `data` is the slice RBSP and `start` the byte offset of the first
    /// `slice_data()` byte after `cabac_alignment_one_bit`.
    pub fn decode_cabac(&mut self, data: &[u8], start: usize) -> Result<()> {
        let intra_slice = self.slice_type.is_intra();
        let init_idc = if intra_slice { None } else { Some(self.hdr.cabac_init_idc) };
        let mut ent = Cabac::new(data, start, self.qp, init_idc);
        let total = self.mbw * self.mbh;
        let mut addr = self.hdr.first_mb_in_slice as usize;
        loop {
            if addr >= total {
                return Err(Error::Invalid("slice runs past the end of the picture"));
            }
            self.start_mb(addr);
            if !intra_slice && ent.skip_flag(self) {
                self.decode_skip_mb()?;
            } else {
                self.decode_mb(&mut ent)?;
            }
            addr += 1;
            if ent.end_of_slice() {
                return Ok(());
            }
            if ent.dec.overrun() {
                return Err(Error::Truncated);
            }
        }
    }

    // ---------------------------------------------------------------------------------------------------
    // Macroblock setup and neighbour access

    /// Position on macroblock `addr`, compute neighbour availability, and reset its info.
    pub(crate) fn start_mb(&mut self, addr: usize) {
        self.mb_addr = addr;
        self.mb_x = addr % self.mbw;
        self.mb_y = addr / self.mbw;
        let (x, y, w) = (self.mb_x, self.mb_y, self.mbw);
        let s = self.slice_num;
        let ok = |mbs: &[MbInfo], a: usize| mbs[a].slice == s;
        self.na = (x > 0 && ok(self.mbs, addr - 1)).then(|| addr - 1);
        self.nb = (y > 0 && ok(self.mbs, addr - w)).then(|| addr - w);
        self.nc = (y > 0 && x + 1 < w && ok(self.mbs, addr - w + 1)).then(|| addr - w + 1);
        self.nd = (y > 0 && x > 0 && ok(self.mbs, addr - w - 1)).then(|| addr - w - 1);
        let mut m = MbInfo::EMPTY;
        m.slice = s;
        self.mbs[addr] = m;
        self.blk_nz = 0;
    }

    /// Coefficient count of the block to the left of block `(bx, by)` of component `comp` (0 luma, 1 Cb, 2 Cr).
    pub(crate) fn nz_left(&self, comp: usize, bx: usize, by: usize) -> Option<u8> {
        let (base, w) = if comp == 0 { (0, 4) } else { (16 + (comp - 1) * 4, 2) };
        if bx > 0 {
            Some(self.mbs[self.mb_addr].nz[base + by * w + bx - 1])
        } else {
            self.na.map(|a| self.mbs[a].nz[base + by * w + w - 1])
        }
    }

    /// Coefficient count of the block above block `(bx, by)` of component `comp`.
    pub(crate) fn nz_above(&self, comp: usize, bx: usize, by: usize) -> Option<u8> {
        let (base, w) = if comp == 0 { (0, 4) } else { (16 + (comp - 1) * 4, 2) };
        if by > 0 {
            Some(self.mbs[self.mb_addr].nz[base + (by - 1) * w + bx])
        } else {
            self.nb.map(|a| self.mbs[a].nz[base + (w - 1) * w + bx])
        }
    }

    // ---------------------------------------------------------------------------------------------------
    // Macroblock layer

    fn classify(&self, intra_raw: u32) -> Result<MbType> {
        Ok(match intra_raw {
            0 => MbType::INxN,
            1..=24 => {
                let i = intra_raw - 1;
                MbType::I16 {
                    mode: (i % 4) as u8,
                    cbp_chroma: ((i / 4) % 3) as u8,
                    cbp_luma: if i >= 12 { 15 } else { 0 },
                }
            }
            25 => MbType::IPcm,
            _ => return Err(Error::Invalid("mb_type out of range")),
        })
    }

    /// Decode one non-skipped macroblock.
    pub(crate) fn decode_mb<E: Entropy>(&mut self, ent: &mut E) -> Result<()> {
        let raw = ent.mb_type(self)?;
        let intra_raw = match self.slice_type {
            SliceType::I | SliceType::Si => raw,
            SliceType::P | SliceType::Sp => {
                if raw < 5 {
                    return self.decode_inter_mb(ent, raw);
                }
                raw - 5
            }
            SliceType::B => {
                if raw < 23 {
                    return self.decode_inter_mb(ent, raw);
                }
                raw - 23
            }
        };
        let mt = self.classify(intra_raw)?;
        let addr = self.mb_addr;
        self.mbs[addr].flags = F_INTRA;
        match mt {
            MbType::IPcm => self.decode_pcm(ent),
            MbType::INxN => {
                let t8 = self.pps.transform_8x8_mode && ent.transform_8x8_flag(self)?;
                self.mbs[addr].flags |= if t8 { F_I8 | F_T8X8 } else { F_I4 };
                let n = if t8 { 4 } else { 16 };
                for i in 0..n {
                    let (flag, rem) = ent.intra_pred_mode()?;
                    let (bx, by) = if t8 { ((i & 1) * 2, (i >> 1) * 2) } else { blk_xy(i) };
                    let pred = self.pred_intra_mode(bx, by);
                    let mode = if flag {
                        pred
                    } else if (rem as i32) < pred {
                        rem as i32
                    } else {
                        rem as i32 + 1
                    };
                    let m = &mut self.mbs[addr];
                    let size = if t8 { 2 } else { 1 };
                    for yy in 0..size {
                        for xx in 0..size {
                            m.ipm[(by + yy) * 4 + bx + xx] = mode as i8;
                        }
                    }
                }
                let chroma_mode = ent.intra_chroma_pred_mode(self)?;
                self.mbs[addr].chroma_mode = chroma_mode;
                let cbp = ent.coded_block_pattern(self, true)?;
                self.mbs[addr].cbp = cbp;
                if cbp != 0 {
                    self.read_qp_delta(ent)?;
                    self.read_residual(ent, true, cbp, t8, false)?;
                } else {
                    self.prev_dqp_nonzero = false;
                }
                self.finish_qp();
                self.store_coefs();
                Ok(())
            }
            MbType::I16 { mode, cbp_luma, cbp_chroma } => {
                self.mbs[addr].flags |= F_I16;
                self.mbs[addr].i16_mode = mode;
                let chroma_mode = ent.intra_chroma_pred_mode(self)?;
                self.mbs[addr].chroma_mode = chroma_mode;
                let cbp = cbp_luma | (cbp_chroma << 4);
                self.mbs[addr].cbp = cbp;
                self.read_qp_delta(ent)?;
                self.read_residual(ent, true, cbp, false, true)?;
                self.finish_qp();
                self.store_coefs();
                Ok(())
            }
        }
    }

    pub(crate) fn read_qp_delta<E: Entropy>(&mut self, ent: &mut E) -> Result<()> {
        let dqp = ent.mb_qp_delta(self)?;
        if !(-26..=25).contains(&dqp) {
            return Err(Error::Invalid("mb_qp_delta out of range"));
        }
        self.prev_dqp_nonzero = dqp != 0;
        self.qp = (self.qp + dqp + 52) % 52;
        Ok(())
    }

    /// Record the QP of the macroblock for deblocking and later chroma QP derivation.
    pub(crate) fn finish_qp(&mut self) {
        let qp = self.qp;
        let (o0, o1) = (self.pps.chroma_qp_index_offset, self.pps.second_chroma_qp_index_offset);
        let m = &mut self.mbs[self.mb_addr];
        m.qp = qp as i8;
        m.qpc = [tr::chroma_qp(qp, o0) as i8, tr::chroma_qp(qp, o1) as i8];
    }

    fn decode_pcm<E: Entropy>(&mut self, ent: &mut E) -> Result<()> {
        let mut buf = [0u8; 384];
        ent.pcm_samples(&mut buf)?;
        // The samples go to the picture's PCM store; the reconstruction side copies them in.
        let off = self.store.pcm.len() as u32;
        self.store.pcm.extend_from_slice(&buf);
        self.mbs[self.mb_addr].coef_off = off;
        let (o0, o1) = (self.pps.chroma_qp_index_offset, self.pps.second_chroma_qp_index_offset);
        let m = &mut self.mbs[self.mb_addr];
        m.flags = F_INTRA | F_PCM;
        m.cbp = 0x2F;
        m.nz = [16; 24];
        m.nzmask = 0xFFFF;
        m.qp = 0;
        m.qpc = [tr::chroma_qp(0, o0) as i8, tr::chroma_qp(0, o1) as i8];
        m.cbf_dc = 7;
        self.prev_dqp_nonzero = false;
        Ok(())
    }

    /// `predIntra4x4PredMode` / `predIntra8x8PredMode` for the block whose top-left 4x4 block is `(bx, by)`.
    fn pred_intra_mode(&self, bx: usize, by: usize) -> i32 {
        let cur = &self.mbs[self.mb_addr];
        let neighbour = |n: Option<usize>, idx: usize| -> Option<i32> {
            let a = n?;
            let m = &self.mbs[a];
            if self.constrained_intra && !m.is_intra() {
                return None;
            }
            Some(if m.ipm[idx] < 0 { 2 } else { m.ipm[idx] as i32 })
        };
        let a = if bx > 0 { Some(cur.ipm[by * 4 + bx - 1] as i32) } else { neighbour(self.na, by * 4 + 3) };
        let b = if by > 0 { Some(cur.ipm[(by - 1) * 4 + bx] as i32) } else { neighbour(self.nb, 12 + bx) };
        match (a, b) {
            (Some(a), Some(b)) => a.min(b),
            _ => 2,
        }
    }

    /// Hand the macroblock's scaled coefficients to the store (and clear them) once it is parsed.
    pub(crate) fn store_coefs(&mut self) {
        let addr = self.mb_addr;
        let mut bits = self.blk_nz;
        self.mbs[addr].blk_nz = bits;
        self.mbs[addr].coef_off = self.store.coefs.len() as u32;
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
            let blk = &mut self.coef[base..base + n];
            // Scaled coefficients are saturated to 16 bits by the dequantiser.
            self.store.coefs.extend(blk.iter().map(|&v| v as i16));
            blk.fill(0);
        }
        self.blk_nz = 0;
    }

    // ---------------------------------------------------------------------------------------------------
    // Residual

    /// Parse the residual of the current macroblock into scaled coefficients.
    pub(crate) fn read_residual<E: Entropy>(
        &mut self,
        ent: &mut E,
        intra: bool,
        cbp: u8,
        t8: bool,
        i16: bool,
    ) -> Result<()> {
        let addr = self.mb_addr;
        let qp = self.qp;
        let (qp_per, qp_rem) = ((qp / 6) as u32, (qp % 6) as usize);
        let l4 = if intra { 0 } else { 3 };
        let l8 = if intra { 0 } else { 1 };
        let mut lv = [0i32; 16];
        if i16 {
            let n = ent.residual_block(self, Cat::LumaDc16, 0, 16, &mut lv)?;
            if n > 0 {
                self.mbs[addr].cbf_dc |= 1;
                let mut c = [0i32; 16];
                for k in 0..16 {
                    c[ZIGZAG_4X4[k] as usize] = lv[k];
                }
                tr::luma_dc_dequant(&mut c, qp, self.ls.l4[l4][qp_rem][0]);
                for by in 0..4 {
                    for bx in 0..4 {
                        let v = c[by * 4 + bx];
                        if v != 0 {
                            self.coef[(by * 4 + bx) * 16] = v;
                            self.blk_nz |= 1 << (by * 4 + bx);
                        }
                    }
                }
            }
        }
        for i8x8 in 0..4 {
            if cbp & (1 << i8x8) == 0 {
                continue;
            }
            if t8 {
                let mut lv8 = [0i32; 64];
                let mut total = 0usize;
                if E::IS_CABAC {
                    let (mut sp, mut sv) = ([0u8; 64], [0i32; 64]);
                    total = ent.residual_sparse(self, Cat::Luma8x8, i8x8, 64, &mut sp, &mut sv)?;
                    let (bx, by) = ((i8x8 & 1) * 2, (i8x8 >> 1) * 2);
                    let cnt = total.min(255) as u8;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        self.mbs[addr].nz[(by + dy) * 4 + bx + dx] = cnt;
                    }
                    if total > 0 {
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            self.mbs[addr].nzmask |= 1 << ((by + dy) * 4 + bx + dx);
                        }
                        self.blk_nz |= 1 << (i8x8 + 24);
                        let base = i8x8 * 64;
                        for j in 0..total {
                            let p = ZIGZAG_8X8[sp[j] as usize] as usize;
                            self.coef[base + p] = tr::dequant_8x8(sv[j], self.ls.l8[l8][qp_rem][p], qp_per);
                        }
                    }
                    continue;
                } else {
                    for i4 in 0..4 {
                        let (bx, by) = blk_xy(i8x8 * 4 + i4);
                        let n = ent.residual_block(self, Cat::Luma4x4, by * 4 + bx, 16, &mut lv)?;
                        self.mbs[addr].nz[by * 4 + bx] = n as u8;
                        total += n;
                        for k in 0..16 {
                            lv8[4 * k + i4] = lv[k];
                        }
                    }
                }
                if total > 0 {
                    let (bx, by) = ((i8x8 & 1) * 2, (i8x8 >> 1) * 2);
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        self.mbs[addr].nzmask |= 1 << ((by + dy) * 4 + bx + dx);
                    }
                    self.blk_nz |= 1 << (i8x8 + 24); // 8x8 block present (bits 24..28)
                    let base = i8x8 * 64;
                    for k in 0..64 {
                        if lv8[k] != 0 {
                            let pos = ZIGZAG_8X8[k] as usize;
                            self.coef[base + pos] =
                                tr::dequant_8x8(lv8[k], self.ls.l8[l8][qp_rem][pos], qp_per);
                        }
                    }
                }
            } else {
                for i4 in 0..4 {
                    let (bx, by) = blk_xy(i8x8 * 4 + i4);
                    let r = by * 4 + bx;
                    let (cat, max, start) = if i16 { (Cat::LumaAc16, 15, 1) } else { (Cat::Luma4x4, 16, 0) };
                    let (mut sp, mut sv) = ([0u8; 64], [0i32; 64]);
                    let n = ent.residual_sparse(self, cat, r, max, &mut sp, &mut sv)?;
                    self.mbs[addr].nz[r] = n as u8;
                    if n > 0 {
                        self.mbs[addr].nzmask |= 1 << r;
                        self.blk_nz |= 1 << r;
                        for j in 0..n {
                            let pos = ZIGZAG_4X4[sp[j] as usize + start] as usize;
                            self.coef[r * 16 + pos] =
                                tr::dequant_4x4(sv[j], self.ls.l4[l4][qp_rem][pos], qp_per);
                        }
                    }
                }
            }
        }
        // Chroma (4:2:0).
        let cbp_c = cbp >> 4;
        if cbp_c != 0 {
            let offs = [self.pps.chroma_qp_index_offset, self.pps.second_chroma_qp_index_offset];
            for comp in 0..2 {
                let qpc = tr::chroma_qp(qp, offs[comp]);
                let list = l4 + 1 + comp;
                let n = ent.residual_block(self, Cat::ChromaDc, comp, 4, &mut lv)?;
                if n > 0 {
                    self.mbs[addr].cbf_dc |= 2 << comp;
                    let mut c = [lv[0], lv[1], lv[2], lv[3]];
                    tr::chroma_dc_dequant(&mut c, qpc, self.ls.l4[list][(qpc % 6) as usize][0]);
                    for blk in 0..4 {
                        if c[blk] != 0 {
                            self.coef[COEF_CB + comp * 64 + blk * 16] = c[blk];
                            self.blk_nz |= 1 << (16 + comp * 4 + blk);
                        }
                    }
                }
            }
            if cbp_c == 2 {
                for comp in 0..2 {
                    let qpc = tr::chroma_qp(qp, offs[comp]);
                    let (per, rem) = ((qpc / 6) as u32, (qpc % 6) as usize);
                    let list = l4 + 1 + comp;
                    for blk in 0..4 {
                        let (mut sp, mut sv) = ([0u8; 64], [0i32; 64]);
                        let n =
                            ent.residual_sparse(self, Cat::ChromaAc, comp * 4 + blk, 15, &mut sp, &mut sv)?;
                        self.mbs[addr].nz[16 + comp * 4 + blk] = n as u8;
                        if n > 0 {
                            self.blk_nz |= 1 << (16 + comp * 4 + blk);
                            for j in 0..n {
                                let pos = ZIGZAG_4X4[sp[j] as usize + 1] as usize;
                                self.coef[COEF_CB + comp * 64 + blk * 16 + pos] =
                                    tr::dequant_4x4(sv[j], self.ls.l4[list][rem][pos], per);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
