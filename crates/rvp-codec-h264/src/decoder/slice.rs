//! Slice decoding: the macroblock loop, syntax parsing, residual decoding and reconstruction.
use super::entropy::{Cat, Cavlc, Entropy};
use super::intra::{self, AV_LEFT, AV_TOP, AV_TOPLEFT};
use super::mbinfo::*;
use super::picture::Picture;
use crate::bitstream::BitReader;
use crate::error::{Error, Result};
use crate::params::{Pps, SliceHeader, SliceType, Sps};
use crate::transform::{self as tr, LevelScale, ZIGZAG_4X4, ZIGZAG_8X8};
use alloc::boxed::Box;

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

/// Decodes the macroblocks of one slice into the current picture.
pub struct SliceDecoder<'a> {
    #[allow(dead_code)]
    pub(crate) sps: &'a Sps,
    pub(crate) pps: &'a Pps,
    pub(crate) hdr: &'a SliceHeader,
    pub(crate) ls: &'a LevelScale,
    pub(crate) cur: &'a mut Picture,
    pub(crate) mbs: &'a mut [MbInfo],
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
        cur: &'a mut Picture,
        mbs: &'a mut [MbInfo],
        slice_num: u16,
    ) -> Self {
        Self {
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

    #[inline]
    fn intra_ok(&self, n: Option<usize>) -> bool {
        match n {
            Some(a) => !self.constrained_intra || self.mbs[a].is_intra(),
            None => false,
        }
    }

    // ---------------------------------------------------------------------------------------------------
    // Macroblock layer

    fn classify(&self, raw: u32) -> Result<MbType> {
        let intra_raw = match self.slice_type {
            SliceType::I | SliceType::Si => raw,
            SliceType::P | SliceType::Sp => {
                if raw < 5 {
                    return Err(Error::Unsupported("inter macroblocks"));
                }
                raw - 5
            }
            SliceType::B => {
                if raw < 23 {
                    return Err(Error::Unsupported("inter macroblocks"));
                }
                raw - 23
            }
        };
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
        let mt = self.classify(raw)?;
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
                self.recon_intra_nxn(t8)?;
                self.recon_chroma_intra(chroma_mode)?;
                Ok(())
            }
            MbType::I16 { mode, cbp_luma, cbp_chroma } => {
                self.mbs[addr].flags |= F_I16;
                let chroma_mode = ent.intra_chroma_pred_mode(self)?;
                self.mbs[addr].chroma_mode = chroma_mode;
                let cbp = cbp_luma | (cbp_chroma << 4);
                self.mbs[addr].cbp = cbp;
                self.read_qp_delta(ent)?;
                self.read_residual(ent, true, cbp, false, true)?;
                self.finish_qp();
                self.recon_intra16(mode)?;
                self.recon_chroma_intra(chroma_mode)?;
                Ok(())
            }
        }
    }

    /// A skipped macroblock (P_Skip or B_Skip).
    pub(crate) fn decode_skip_mb(&mut self) -> Result<()> {
        Err(Error::Unsupported("skipped macroblocks"))
    }

    fn read_qp_delta<E: Entropy>(&mut self, ent: &mut E) -> Result<()> {
        let dqp = ent.mb_qp_delta(self)?;
        if !(-26..=25).contains(&dqp) {
            return Err(Error::Invalid("mb_qp_delta out of range"));
        }
        self.prev_dqp_nonzero = dqp != 0;
        self.qp = (self.qp + dqp + 52) % 52;
        Ok(())
    }

    /// Record the QP of the macroblock for deblocking and later chroma QP derivation.
    fn finish_qp(&mut self) {
        let qp = self.qp;
        let (o0, o1) = (self.pps.chroma_qp_index_offset, self.pps.second_chroma_qp_index_offset);
        let m = &mut self.mbs[self.mb_addr];
        m.qp = qp as i8;
        m.qpc = [tr::chroma_qp(qp, o0) as i8, tr::chroma_qp(qp, o1) as i8];
    }

    fn decode_pcm<E: Entropy>(&mut self, ent: &mut E) -> Result<()> {
        let mut buf = [0u8; 384];
        ent.pcm_samples(&mut buf)?;
        let (mx, my) = (self.mb_x, self.mb_y);
        let ys = self.cur.strides[0];
        for y in 0..16 {
            let o = (my * 16 + y) * ys + mx * 16;
            self.cur.planes[0][o..o + 16].copy_from_slice(&buf[y * 16..y * 16 + 16]);
        }
        for c in 0..2 {
            let cs = self.cur.strides[1 + c];
            for y in 0..8 {
                let o = (my * 8 + y) * cs + mx * 8;
                let s = 256 + c * 64 + y * 8;
                self.cur.planes[1 + c][o..o + 8].copy_from_slice(&buf[s..s + 8]);
            }
        }
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

    // ---------------------------------------------------------------------------------------------------
    // Residual

    /// Parse the residual of the current macroblock into scaled coefficients.
    fn read_residual<E: Entropy>(
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
                    total = ent.residual_block(self, Cat::Luma8x8, i8x8, 64, &mut lv8)?;
                    let (bx, by) = ((i8x8 & 1) * 2, (i8x8 >> 1) * 2);
                    let cnt = total.min(255) as u8;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        self.mbs[addr].nz[(by + dy) * 4 + bx + dx] = cnt;
                    }
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
                    let n = ent.residual_block(self, cat, r, max, &mut lv)?;
                    self.mbs[addr].nz[r] = n as u8;
                    if n > 0 {
                        self.mbs[addr].nzmask |= 1 << r;
                        self.blk_nz |= 1 << r;
                        for k in 0..max {
                            if lv[k] != 0 {
                                let pos = ZIGZAG_4X4[k + start] as usize;
                                self.coef[r * 16 + pos] =
                                    tr::dequant_4x4(lv[k], self.ls.l4[l4][qp_rem][pos], qp_per);
                            }
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
                        let n = ent.residual_block(self, Cat::ChromaAc, comp * 4 + blk, 15, &mut lv)?;
                        self.mbs[addr].nz[16 + comp * 4 + blk] = n as u8;
                        if n > 0 {
                            self.blk_nz |= 1 << (16 + comp * 4 + blk);
                            for k in 0..15 {
                                if lv[k] != 0 {
                                    let pos = ZIGZAG_4X4[k + 1] as usize;
                                    self.coef[COEF_CB + comp * 64 + blk * 16 + pos] =
                                        tr::dequant_4x4(lv[k], self.ls.l4[list][rem][pos], per);
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------------------
    // Reconstruction

    /// Apply the 4x4 residual of luma block `r` (raster index) at plane position `(x, y)`.
    fn add_luma_block(&mut self, r: usize, x: usize, y: usize) {
        if self.blk_nz & (1 << r) == 0 {
            return;
        }
        let stride = self.cur.strides[0];
        let c = &mut self.coef[r * 16..r * 16 + 16];
        let dst = &mut self.cur.planes[0][y * stride + x..];
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
    fn add_luma_block8(&mut self, i8: usize, x: usize, y: usize) {
        if self.blk_nz & (1 << (24 + i8)) == 0 {
            return;
        }
        let stride = self.cur.strides[0];
        let c = &mut self.coef[i8 * 64..i8 * 64 + 64];
        let mut b = [0i32; 64];
        b.copy_from_slice(c);
        tr::idct8x8(&mut b);
        tr::add_residual_8x8(&b, &mut self.cur.planes[0][y * stride + x..], stride);
        c.fill(0);
    }

    fn add_chroma_blocks(&mut self) {
        for comp in 0..2 {
            let stride = self.cur.strides[1 + comp];
            for blk in 0..4 {
                if self.blk_nz & (1 << (16 + comp * 4 + blk)) == 0 {
                    continue;
                }
                let base = COEF_CB + comp * 64 + blk * 16;
                let (x, y) = (self.mb_x * 8 + (blk & 1) * 4, self.mb_y * 8 + (blk >> 1) * 4);
                let c = &mut self.coef[base..base + 16];
                let dst = &mut self.cur.planes[1 + comp][y * stride + x..];
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
        let stride = self.cur.strides[0];
        let (mx, my) = (self.mb_x * 16, self.mb_y * 16);
        if t8 {
            for i8 in 0..4 {
                let (bx, by) = ((i8 & 1) * 2, (i8 >> 1) * 2);
                let (x, y) = (mx + bx * 4, my + by * 4);
                let mode = self.mbs[self.mb_addr].ipm[by * 4 + bx] as u8;
                let (mut top, mut left, tl, avail) = self.gather_8x8(bx, by, x, y);
                let (ft, fl, ftl);
                (ft, fl, ftl) = intra::filter_8x8_refs(&top, &left, tl, avail);
                top = ft;
                left = fl;
                let dst = &mut self.cur.planes[0][y * stride + x..];
                intra::predict_nxn::<8>(dst, stride, mode, &top, &left, ftl, avail)?;
                self.add_luma_block8(i8, x, y);
            }
        } else {
            for blk in 0..16 {
                let (bx, by) = blk_xy(blk);
                let (x, y) = (mx + bx * 4, my + by * 4);
                let mode = self.mbs[self.mb_addr].ipm[by * 4 + bx] as u8;
                let (top, left, tl, avail) = self.gather_4x4(bx, by, x, y);
                let dst = &mut self.cur.planes[0][y * stride + x..];
                intra::predict_nxn::<4>(dst, stride, mode, &top, &left, tl, avail)?;
                self.add_luma_block(by * 4 + bx, x, y);
            }
        }
        Ok(())
    }

    /// Neighbouring samples and availability for the 4x4 block at `(bx, by)`, picture position `(x, y)`.
    fn gather_4x4(&self, bx: usize, by: usize, x: usize, y: usize) -> ([u8; 8], [u8; 4], u8, u8) {
        let stride = self.cur.strides[0];
        let p = &self.cur.planes[0];
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
        let stride = self.cur.strides[0];
        let p = &self.cur.planes[0];
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
        let stride = self.cur.strides[0];
        let (mx, my) = (self.mb_x * 16, self.mb_y * 16);
        let top_ok = self.intra_ok(self.nb);
        let left_ok = self.intra_ok(self.na);
        let tl_ok = self.intra_ok(self.nd);
        let mut top = [128u8; 16];
        let mut left = [128u8; 16];
        let mut tl = 128;
        let mut avail = 0;
        {
            let p = &self.cur.planes[0];
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
        intra::predict_16x16(
            &mut self.cur.planes[0][my * stride + mx..],
            stride,
            mode,
            &top,
            &left,
            tl,
            avail,
        )?;
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
            let stride = self.cur.strides[1 + comp];
            let mut top = [128u8; 8];
            let mut left = [128u8; 8];
            let mut tl = 128;
            let mut avail = 0;
            {
                let p = &self.cur.planes[1 + comp];
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
                &mut self.cur.planes[1 + comp][my * stride + mx..],
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
