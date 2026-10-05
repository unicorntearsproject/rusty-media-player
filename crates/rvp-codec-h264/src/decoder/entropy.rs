//! The syntax-element layer both entropy coders implement, plus the CAVLC implementation.
use super::slice::SliceDecoder;
use crate::bitstream::BitReader;
use crate::cavlc::{
    self,
    tables::{CBP_INTER, CBP_INTRA},
};
use crate::error::{Error, Result};
use crate::params::SliceType;

/// Residual block category (`ctxBlockCat`, Table 9-42 without the 4:4:4 categories).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cat {
    /// Intra16x16 luma DC (category 0).
    LumaDc16,
    /// Intra16x16 luma AC (category 1).
    LumaAc16,
    /// Luma 4x4 block (category 2).
    Luma4x4,
    /// Chroma DC (category 3).
    ChromaDc,
    /// Chroma AC (category 4).
    ChromaAc,
    /// Luma 8x8 block (category 5; CABAC only).
    Luma8x8,
}

/// Reads macroblock-layer syntax elements. `d` gives access to the neighbourhood the contexts depend on.
pub trait Entropy {
    /// True for CABAC (changes how 8x8 residual blocks and skipping are coded).
    const IS_CABAC: bool;
    /// `mb_type` in the numbering of the current slice type (Tables 7-11, 7-13, 7-14).
    fn mb_type(&mut self, d: &SliceDecoder<'_>) -> Result<u32>;
    /// `sub_mb_type` (Tables 7-17, 7-18).
    fn sub_mb_type(&mut self, d: &SliceDecoder<'_>) -> Result<u32>;
    /// `transform_size_8x8_flag`.
    fn transform_8x8_flag(&mut self, d: &SliceDecoder<'_>) -> Result<bool>;
    /// `prev_intra4x4_pred_mode_flag` / `prev_intra8x8_pred_mode_flag` and, if clear, `rem_intra*_pred_mode`.
    fn intra_pred_mode(&mut self) -> Result<(bool, u8)>;
    /// `intra_chroma_pred_mode`.
    fn intra_chroma_pred_mode(&mut self, d: &SliceDecoder<'_>) -> Result<u8>;
    /// `ref_idx_lX` of the partition whose top-left 8x8 block is `(x8, y8)` (units of 8 samples in the MB);
    /// `num_ref` is the number of active references.
    fn ref_idx(
        &mut self,
        d: &SliceDecoder<'_>,
        list: usize,
        x8: usize,
        y8: usize,
        num_ref: usize,
    ) -> Result<u32>;
    /// One component of `mvd_lX` of the partition whose top-left 4x4 block is `(x4, y4)` inside the MB.
    fn mvd(&mut self, d: &SliceDecoder<'_>, list: usize, x4: usize, y4: usize, comp: usize) -> Result<i32>;
    /// `coded_block_pattern` (luma bits 0..3, chroma in bits 4..5).
    fn coded_block_pattern(&mut self, d: &SliceDecoder<'_>, intra: bool) -> Result<u8>;
    /// `mb_qp_delta`.
    fn mb_qp_delta(&mut self, d: &SliceDecoder<'_>) -> Result<i32>;
    /// One residual block. `idx` is the raster 4x4 block index for luma, `comp * 4 + blk` for chroma AC, the
    /// component for chroma DC and the 8x8 block index for 8x8 blocks. Coefficients are written to `out` in scan
    /// order (positions 0 to `max - 1`); returns the number of non-zero coefficients.
    fn residual_block(
        &mut self,
        d: &SliceDecoder<'_>,
        cat: Cat,
        idx: usize,
        max: usize,
        out: &mut [i32],
    ) -> Result<usize>;
    /// [`residual_block`](Entropy::residual_block) in sparse form: the scan positions of the non-zero coefficients go to
    /// `pos` and their levels to `val`; returns how many there are. The default goes through the dense form.
    fn residual_sparse(
        &mut self,
        d: &SliceDecoder<'_>,
        cat: Cat,
        idx: usize,
        max: usize,
        pos: &mut [u8; 64],
        val: &mut [i32; 64],
    ) -> Result<usize> {
        let mut lv = [0i32; 64];
        let n = self.residual_block(d, cat, idx, max, &mut lv)?;
        let mut m = 0;
        for (k, &v) in lv[..max].iter().enumerate() {
            if v != 0 {
                pos[m] = k as u8;
                val[m] = v;
                m += 1;
            }
        }
        debug_assert_eq!(m, n);
        Ok(m)
    }
    /// The 384 PCM sample bytes of an I_PCM macroblock; the entropy decoder resynchronises afterwards.
    fn pcm_samples(&mut self, out: &mut [u8; 384]) -> Result<()>;
}

/// CAVLC (Exp-Golomb and table-driven) syntax element reading.
pub struct Cavlc<'a> {
    /// The slice data bit reader.
    pub r: BitReader<'a>,
}

impl Entropy for Cavlc<'_> {
    const IS_CABAC: bool = false;

    fn mb_type(&mut self, _d: &SliceDecoder<'_>) -> Result<u32> {
        self.r.read_ue()
    }

    fn sub_mb_type(&mut self, _d: &SliceDecoder<'_>) -> Result<u32> {
        self.r.read_ue()
    }

    fn transform_8x8_flag(&mut self, _d: &SliceDecoder<'_>) -> Result<bool> {
        self.r.read_flag()
    }

    fn intra_pred_mode(&mut self) -> Result<(bool, u8)> {
        if self.r.read_flag()? { Ok((true, 0)) } else { Ok((false, self.r.read_bits(3)? as u8)) }
    }

    fn intra_chroma_pred_mode(&mut self, _d: &SliceDecoder<'_>) -> Result<u8> {
        let v = self.r.read_ue()?;
        if v > 3 {
            return Err(Error::Invalid("intra_chroma_pred_mode out of range"));
        }
        Ok(v as u8)
    }

    fn ref_idx(
        &mut self,
        _d: &SliceDecoder<'_>,
        _list: usize,
        _x8: usize,
        _y8: usize,
        num_ref: usize,
    ) -> Result<u32> {
        let v = self.r.read_te(num_ref as u32 - 1)?;
        if v as usize >= num_ref {
            return Err(Error::Invalid("ref_idx out of range"));
        }
        Ok(v)
    }

    fn mvd(
        &mut self,
        _d: &SliceDecoder<'_>,
        _list: usize,
        _x4: usize,
        _y4: usize,
        _comp: usize,
    ) -> Result<i32> {
        self.r.read_se()
    }

    fn coded_block_pattern(&mut self, d: &SliceDecoder<'_>, intra: bool) -> Result<u8> {
        let _ = d;
        let v = self.r.read_ue()? as usize;
        if v >= 48 {
            return Err(Error::Invalid("coded_block_pattern codeNum out of range"));
        }
        Ok(if intra { CBP_INTRA[v] } else { CBP_INTER[v] })
    }

    fn mb_qp_delta(&mut self, _d: &SliceDecoder<'_>) -> Result<i32> {
        self.r.read_se()
    }

    fn residual_block(
        &mut self,
        d: &SliceDecoder<'_>,
        cat: Cat,
        idx: usize,
        max: usize,
        out: &mut [i32],
    ) -> Result<usize> {
        let nc = match cat {
            Cat::ChromaDc => -1,
            Cat::ChromaAc => {
                let (comp, blk) = (idx / 4, idx % 4);
                cavlc::predict_nc(
                    d.nz_left(1 + comp, blk & 1, blk >> 1),
                    d.nz_above(1 + comp, blk & 1, blk >> 1),
                )
            }
            _ => cavlc::predict_nc(d.nz_left(0, idx & 3, idx >> 2), d.nz_above(0, idx & 3, idx >> 2)),
        };
        cavlc::read_residual_block(&mut self.r, nc, max, out)
    }

    fn pcm_samples(&mut self, out: &mut [u8; 384]) -> Result<()> {
        self.r.align();
        if self.r.bits_left() < 384 * 8 {
            return Err(Error::Truncated);
        }
        let start = self.r.pos() / 8;
        out.copy_from_slice(&self.r.data()[start..start + 384]);
        self.r.skip(384 * 8);
        Ok(())
    }
}

/// True if the slice type has P-style inter prediction with list 0 only.
pub fn is_p(t: SliceType) -> bool {
    matches!(t, SliceType::P | SliceType::Sp)
}
