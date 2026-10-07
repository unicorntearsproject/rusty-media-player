//! Picture-level state of the software decoder: the CTB address tables, the per-block information every stage reads, and the order
//! in which a picture's stages run.
use super::ctu::Dec;
use super::deblock;
use super::frame::{ColMotion, Frame, Mv};
use super::sao;
use crate::nal;
use crate::ps::{Pps, Sps};
use crate::slice::SliceHeader;
use crate::stream::{Picture, SliceSeg};
use crate::{Error, Result};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ops::Deref;

/// What is kept for each 4x4 luma block.
#[derive(Clone, Copy, Default)]
pub(super) struct Cell {
    /// 0 not decoded yet, 1 intra, 2 inter.
    pub mode: u8,
    /// `cu_skip_flag`.
    pub skip: bool,
    /// `CtDepth` of the coding unit.
    pub depth: u8,
    /// `IntraPredModeY`.
    pub intra_mode: u8,
    /// `QpY`.
    pub qp: i8,
    /// `F_*` bits.
    pub flags: u8,
}

/// The coding unit was coded without transform or quantisation.
pub(super) const F_BYPASS: u8 = 1;
/// PCM samples with the loop filters off for them.
pub(super) const F_PCM_NOFILTER: u8 = 2;
/// The luma transform block here has a non-zero coefficient.
pub(super) const F_CBF: u8 = 4;
/// A transform block edge on the left side of this block.
pub(super) const F_TU_LEFT: u8 = 8;
/// A transform block edge on the top side.
pub(super) const F_TU_TOP: u8 = 16;
/// A prediction block edge on the left side.
pub(super) const F_PU_LEFT: u8 = 32;
/// A prediction block edge on the top side.
pub(super) const F_PU_TOP: u8 = 64;

/// The motion of a prediction block.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct MvField {
    /// Vectors for list 0 and 1.
    pub mv: [Mv; 2],
    /// Reference indices, -1 when the list is not used.
    pub ref_idx: [i8; 2],
    /// POC of the reference pictures (identifies them across slices, for the deblocking filter).
    pub ref_poc: [i32; 2],
}

impl MvField {
    pub const NONE: MvField = MvField { mv: [Mv { x: 0, y: 0 }; 2], ref_idx: [-1, -1], ref_poc: [0, 0] };
}

/// `SaoTypeIdx`, offsets and band position or edge class of one CTB.
#[derive(Clone, Copy, Default)]
pub(super) struct SaoCtb {
    pub type_idx: [u8; 3],
    pub band_pos: [u8; 3],
    pub eo_class: [u8; 3],
    pub offset: [[i16; 4]; 3],
}

pub(super) struct PicState {
    pub ctb_log2: u32,
    pub w_ctb: usize,
    pub h_ctb: usize,
    pub width: usize,
    pub height: usize,
    pub rs_to_ts: Vec<u32>,
    pub ts_to_rs: Vec<u32>,
    /// `TileId` by raster CTB address.
    pub tile_id_rs: Vec<u16>,
    pub col_bd: Vec<u32>,
    pub row_bd: Vec<u32>,
    pub log2_min_tb: u32,
    pub w_tb: usize,
    /// `MinTbAddrZs` by (y * w_tb + x).
    pub zs: Vec<u32>,
    /// `SliceAddrRs` by raster CTB address (`u32::MAX` while not decoded).
    pub ctb_slice_addr: Vec<u32>,
    /// Index into `slices` by raster CTB address.
    pub ctb_slice: Vec<u16>,
    pub w4: usize,
    pub h4: usize,
    pub cells: Vec<Cell>,
    pub mvf: Vec<MvField>,
    pub sao: Vec<SaoCtb>,
    /// Header of each independent slice of the picture.
    pub slices: Vec<SliceHeader>,
    /// `SliceAddrRs` of the slice being decoded.
    pub cur_slice_addr: u32,
    /// The contexts stored for WPP synchronisation.
    pub wpp_state: Option<crate::cabac::Contexts>,
}

impl PicState {
    fn new(sps: &Sps, pps: &Pps) -> Self {
        let ctb_log2 = sps.log2_ctb as u32;
        let (w_ctb, h_ctb) = (sps.width_in_ctbs() as usize, sps.height_in_ctbs() as usize);
        let (cols, rows) = pps.tile_grid(sps);
        let mut col_bd = vec![0u32];
        for c in &cols {
            col_bd.push(col_bd.last().copied().unwrap_or(0) + c);
        }
        let mut row_bd = vec![0u32];
        for r in &rows {
            row_bd.push(row_bd.last().copied().unwrap_or(0) + r);
        }
        // 6.5.1: CtbAddrRsToTs, CtbAddrTsToRs and TileId.
        let n = w_ctb * h_ctb;
        let mut rs_to_ts = vec![0u32; n];
        let mut ts_to_rs = vec![0u32; n];
        let mut tile_id_rs = vec![0u16; n];
        for rs in 0..n {
            let (tbx, tby) = ((rs % w_ctb) as u32, (rs / w_ctb) as u32);
            let tile_x = (0..cols.len()).rev().find(|i| tbx >= col_bd[*i]).unwrap_or(0);
            let tile_y = (0..rows.len()).rev().find(|i| tby >= row_bd[*i]).unwrap_or(0);
            let mut v = 0u32;
            for i in 0..tile_x {
                v += rows[tile_y] * cols[i];
            }
            for j in 0..tile_y {
                v += w_ctb as u32 * rows[j];
            }
            v += (tby - row_bd[tile_y]) * cols[tile_x] + tbx - col_bd[tile_x];
            rs_to_ts[rs] = v;
            ts_to_rs[v as usize] = rs as u32;
            tile_id_rs[rs] = (tile_y * cols.len() + tile_x) as u16;
        }
        // 6.5.2: MinTbAddrZs.
        let log2_min_tb = sps.log2_min_tb as u32;
        let shift = ctb_log2 - log2_min_tb;
        let (w_tb, h_tb) = (w_ctb << shift, h_ctb << shift);
        let mut zs = vec![0u32; w_tb * h_tb];
        for y in 0..h_tb {
            for x in 0..w_tb {
                let rs = (y >> shift) * w_ctb + (x >> shift);
                let mut a = rs_to_ts[rs] << (shift * 2);
                for i in 0..shift {
                    let m = 1usize << i;
                    a += (if m & x != 0 { m * m } else { 0 } as u32)
                        + (if m & y != 0 { 2 * m * m } else { 0 } as u32);
                }
                zs[y * w_tb + x] = a;
            }
        }
        let (width, height) = (sps.width as usize, sps.height as usize);
        let (w4, h4) = (width.div_ceil(4), height.div_ceil(4));
        Self {
            ctb_log2,
            w_ctb,
            h_ctb,
            width,
            height,
            rs_to_ts,
            ts_to_rs,
            tile_id_rs,
            col_bd,
            row_bd,
            log2_min_tb,
            w_tb,
            zs,
            ctb_slice_addr: vec![u32::MAX; n],
            ctb_slice: vec![0; n],
            w4,
            h4,
            cells: vec![Cell::default(); w4 * h4],
            mvf: vec![MvField::NONE; w4 * h4],
            sao: vec![SaoCtb::default(); n],
            slices: Vec::new(),
            cur_slice_addr: 0,
            wpp_state: None,
        }
    }

    /// 6.4.1: whether the block at luma (`xn`, `yn`) is available for the one at (`xc`, `yc`): inside the picture, already decoded, in
    /// the same slice and the same tile.
    pub fn available(&self, xc: i32, yc: i32, xn: i32, yn: i32) -> bool {
        if xn < 0 || yn < 0 || xn >= self.width as i32 || yn >= self.height as i32 {
            return false;
        }
        let s = self.log2_min_tb;
        let (zn, zc) = (
            self.zs[(yn as usize >> s) * self.w_tb + (xn as usize >> s)],
            self.zs[(yc as usize >> s) * self.w_tb + (xc as usize >> s)],
        );
        if zn > zc {
            return false;
        }
        let (cn, cc) = (self.ctb_of(xn, yn), self.ctb_of(xc, yc));
        let sa = self.ctb_slice_addr[cn];
        sa != u32::MAX && sa == self.ctb_slice_addr[cc] && self.tile_id_rs[cn] == self.tile_id_rs[cc]
    }

    /// Raster address of the CTB containing luma (`x`, `y`).
    pub fn ctb_of(&self, x: i32, y: i32) -> usize {
        (y as usize >> self.ctb_log2) * self.w_ctb + (x as usize >> self.ctb_log2)
    }
}

/// Decode one picture: every slice segment, then the loop filters, then the motion kept for later pictures.
pub(super) fn decode_picture(pic: &Picture<'_, super::Surface>) -> Result<()> {
    let (sps, pps) = (pic.sps, pic.pps);
    let mut st = PicState::new(sps, pps);
    let mut cur = pic.surface.borrow_mut();
    cur.poc = pic.poc;
    cur.motion.fill(ColMotion::default());
    // The reference pictures, read-only for the whole picture.
    let ref_guards: Vec<core::cell::Ref<'_, Frame>> =
        pic.refs.iter().map(|r| RefCell::borrow(r.surface.deref())).collect();
    let refs: Vec<&Frame> = ref_guards.iter().map(|g| &**g).collect();
    let mut prev_independent: Option<usize> = None;
    let mut saved_ds = None;
    for seg in &pic.slices {
        if !seg.header.dependent {
            st.slices.push(seg.header.clone());
            prev_independent = Some(st.slices.len() - 1);
        }
        let slice_idx = prev_independent.ok_or(Error::Invalid("a dependent slice segment first"))?;
        decode_segment(&mut st, &mut cur, &refs, pic, seg, slice_idx, &mut saved_ds)?;
    }
    deblock::run(&mut st, &mut cur, sps, pps)?;
    sao::run(&mut st, &mut cur, sps, pps)?;
    Ok(())
}

fn decode_segment(
    st: &mut PicState,
    cur: &mut Frame,
    refs: &[&Frame],
    pic: &Picture<'_, super::Surface>,
    seg: &SliceSeg<'_>,
    slice_idx: usize,
    saved_ds: &mut Option<super::ctu::SavedState>,
) -> Result<()> {
    let mut rbsp = Vec::with_capacity(seg.unit.len());
    let mut removed = Vec::new();
    nal::unescape(seg.unit, &mut rbsp, &mut removed);
    let mut dec = Dec::new(
        pic.sps,
        pic.pps,
        &seg.header,
        &seg.ref_lists,
        pic,
        refs,
        st,
        cur,
        &rbsp,
        &removed,
        slice_idx,
    );
    dec.run(saved_ds)
}
