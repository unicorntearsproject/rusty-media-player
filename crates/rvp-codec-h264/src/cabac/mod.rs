//! CABAC (clause 9.3): context initialisation tables, the arithmetic decoding engine and a matching
//! arithmetic encoder.
//!
//! The decoder in [`CabacDecoder`] keeps the arithmetic code value with prefetched bits so it needs one table
//! lookup and a shift per bin, yet it tracks the exact bit position the specification's bit-serial engine would
//! have (needed after I_PCM). [`CabacEncoder`] is the informative encoder of 9.3.4. Both share [`Contexts`].
//!
//! Syntax element binarizations and context selection live with the callers: they depend on the macroblock
//! neighbourhood. The offsets they need are in [`ctx`].

mod init_tables;
mod tables;

pub use init_tables::CABAC_INIT;
pub use tables::{
    LAST_COEFF_8X8, RANGE_TAB_LPS, SIG_COEFF_8X8_FIELD, SIG_COEFF_8X8_FRAME, TRANS_IDX_LPS, TRANS_IDX_MPS,
};

use crate::bitstream::BitWriter;

/// Context index offsets (Table 9-34) for frame-coded 4:2:0 video.
pub mod ctx {
    /// `mb_type` in I slices (prefix bins), and the I-type suffix base for SI slices.
    pub const MB_TYPE_I: usize = 3;
    /// `mb_skip_flag` in P slices.
    pub const MB_SKIP_P: usize = 11;
    /// `mb_type` prefix in P slices.
    pub const MB_TYPE_P: usize = 14;
    /// `mb_type` suffix (intra types) in P slices.
    pub const MB_TYPE_P_INTRA: usize = 17;
    /// `sub_mb_type` in P slices.
    pub const SUB_MB_TYPE_P: usize = 21;
    /// `mb_skip_flag` in B slices.
    pub const MB_SKIP_B: usize = 24;
    /// `mb_type` prefix in B slices.
    pub const MB_TYPE_B: usize = 27;
    /// `mb_type` suffix (intra types) in B slices.
    pub const MB_TYPE_B_INTRA: usize = 32;
    /// `sub_mb_type` in B slices.
    pub const SUB_MB_TYPE_B: usize = 36;
    /// `mvd_lX[][][0]` (horizontal).
    pub const MVD_X: usize = 40;
    /// `mvd_lX[][][1]` (vertical).
    pub const MVD_Y: usize = 47;
    /// `ref_idx_lX`.
    pub const REF_IDX: usize = 54;
    /// `mb_qp_delta`.
    pub const MB_QP_DELTA: usize = 60;
    /// `intra_chroma_pred_mode`.
    pub const INTRA_CHROMA_PRED_MODE: usize = 64;
    /// `prev_intra4x4_pred_mode_flag` and `prev_intra8x8_pred_mode_flag`.
    pub const PREV_INTRA_PRED_MODE_FLAG: usize = 68;
    /// `rem_intra4x4_pred_mode` and `rem_intra8x8_pred_mode`.
    pub const REM_INTRA_PRED_MODE: usize = 69;
    /// `coded_block_pattern`, luma prefix.
    pub const CBP_LUMA: usize = 73;
    /// `coded_block_pattern`, chroma suffix.
    pub const CBP_CHROMA: usize = 77;
    /// `coded_block_flag` for categories below 5.
    pub const CODED_BLOCK_FLAG: usize = 85;
    /// `significant_coeff_flag`, frame coded, categories below 5.
    pub const SIG_COEFF: usize = 105;
    /// `last_significant_coeff_flag`, frame coded, categories below 5.
    pub const LAST_COEFF: usize = 166;
    /// `coeff_abs_level_minus1`, categories below 5.
    pub const COEFF_ABS_LEVEL: usize = 227;
    /// `end_of_slice_flag` and the I_PCM bin (terminating, not adaptive).
    pub const TERMINATE: usize = 276;
    /// `transform_size_8x8_flag`.
    pub const TRANSFORM_8X8: usize = 399;
    /// `significant_coeff_flag`, frame coded, category 5.
    pub const SIG_COEFF_8X8: usize = 402;
    /// `last_significant_coeff_flag`, frame coded, category 5.
    pub const LAST_COEFF_8X8: usize = 417;
    /// `coeff_abs_level_minus1`, category 5.
    pub const COEFF_ABS_LEVEL_8X8: usize = 426;
    /// `ctxIdxBlockCatOffset` of `coded_block_flag` for categories 0 to 4 (Table 9-40).
    pub const CBF_CAT_OFFSET: [usize; 5] = [0, 4, 8, 12, 16];
    /// `ctxIdxBlockCatOffset` of `significant_coeff_flag` and `last_significant_coeff_flag` for categories 0 to 4.
    pub const SIG_CAT_OFFSET: [usize; 5] = [0, 15, 29, 44, 47];
    /// `ctxIdxBlockCatOffset` of `coeff_abs_level_minus1` for categories 0 to 4.
    pub const ABS_CAT_OFFSET: [usize; 5] = [0, 10, 20, 30, 39];
}

/// The adaptive context states of a slice: `pStateIdx << 1 | valMPS` per context.
#[derive(Clone)]
pub struct Contexts {
    state: [u8; 1024],
}

impl Default for Contexts {
    fn default() -> Self {
        Self { state: [0; 1024] }
    }
}

impl Contexts {
    /// Initialise all contexts (9.3.1.1) for a slice with quantiser `slice_qp`. `cabac_init_idc` is `None` for I and
    /// SI slices.
    pub fn new(slice_qp: i32, cabac_init_idc: Option<u32>) -> Self {
        let mut c = Self::default();
        c.init(slice_qp, cabac_init_idc);
        c
    }

    /// Re-initialise in place.
    pub fn init(&mut self, slice_qp: i32, cabac_init_idc: Option<u32>) {
        let col = match cabac_init_idc {
            None => 0,
            Some(i) => 1 + (i.min(2) as usize),
        };
        let qp = slice_qp.clamp(0, 51);
        for (i, st) in self.state.iter_mut().enumerate() {
            let (m, n) = CABAC_INIT[i][col];
            let pre = (((m as i32 * qp) >> 4) + n as i32).clamp(1, 126);
            *st = if pre <= 63 { ((63 - pre) as u8) << 1 } else { (((pre - 64) as u8) << 1) | 1 };
        }
        // ctxIdx 276 is the terminating context; it is never used adaptively.
        self.state[ctx::TERMINATE] = 63 << 1;
    }
}

/// The arithmetic decoding engine.
pub struct CabacDecoder<'a> {
    data: &'a [u8],
    /// Next byte to load.
    pos: usize,
    /// `codIOffset << nbits | prefetched bits`.
    value: u64,
    /// Number of prefetched bits in `value`.
    nbits: u32,
    /// `codIRange`.
    range: u32,
    /// Bits read past the end of the data (so the caller can detect truncation).
    overrun_bits: u32,
}

impl<'a> CabacDecoder<'a> {
    /// Initialise the engine (9.3.1.2) reading from `data[start..]`.
    pub fn new(data: &'a [u8], start: usize) -> Self {
        let mut d = Self { data, pos: start, value: 0, nbits: 0, range: 510, overrun_bits: 0 };
        // Load enough bytes for codIOffset (9 bits) plus prefetch.
        d.refill();
        d.refill();
        d.nbits -= 9;
        d
    }

    #[inline]
    fn refill(&mut self) {
        // Load 16 bits at a time; the window never exceeds 9 + 39 bits.
        let hi = self.data.get(self.pos).copied();
        let lo = self.data.get(self.pos + 1).copied();
        if hi.is_none() {
            self.overrun_bits += 8;
        }
        if lo.is_none() {
            self.overrun_bits += 8;
        }
        self.value = (self.value << 16) | ((hi.unwrap_or(0) as u64) << 8) | lo.unwrap_or(0) as u64;
        self.pos += 2;
        self.nbits += 16;
    }

    /// True if the engine has consumed bits beyond the end of the data.
    pub fn overrun(&self) -> bool {
        // `overrun_bits` counts prefetched padding; consumed padding is what matters.
        self.overrun_bits > self.nbits
    }

    /// The bit position the specification's engine would be at: one past the last bit read into `codIOffset`.
    pub fn bit_pos(&self) -> usize {
        self.pos * 8 - self.nbits as usize
    }

    /// Decode a bin with the adaptive context `ctx_idx`.
    #[inline]
    pub fn decision(&mut self, ctxs: &mut Contexts, ctx_idx: usize) -> u32 {
        if self.nbits < 8 {
            self.refill();
        }
        let st = ctxs.state[ctx_idx];
        let (p, mps) = ((st >> 1) as usize, (st & 1) as u32);
        let lps = RANGE_TAB_LPS[p][((self.range >> 6) & 3) as usize] as u32;
        let rmps = self.range - lps;
        let scaled = (rmps as u64) << self.nbits;
        let bin;
        if self.value < scaled {
            bin = mps;
            ctxs.state[ctx_idx] = (TRANS_IDX_MPS[p] << 1) | mps as u8;
            self.range = rmps;
            if rmps < 256 {
                self.range <<= 1;
                self.nbits -= 1;
            }
        } else {
            self.value -= scaled;
            bin = 1 - mps;
            let new_mps = if p == 0 { 1 - mps } else { mps };
            ctxs.state[ctx_idx] = (TRANS_IDX_LPS[p] << 1) | new_mps as u8;
            let shift = lps.leading_zeros() - 23; // bring range (9 bits) up to at least 256
            self.range = lps << shift;
            self.nbits -= shift;
        }
        bin
    }

    /// Decode a bypass bin.
    #[inline]
    pub fn bypass(&mut self) -> u32 {
        if self.nbits < 8 {
            self.refill();
        }
        self.nbits -= 1;
        let scaled = (self.range as u64) << self.nbits;
        if self.value >= scaled {
            self.value -= scaled;
            1
        } else {
            0
        }
    }

    /// Decode a bin before termination (end of slice or the I_PCM flag).
    pub fn terminate(&mut self) -> u32 {
        if self.nbits < 8 {
            self.refill();
        }
        self.range -= 2;
        let scaled = (self.range as u64) << self.nbits;
        if self.value >= scaled {
            1
        } else {
            if self.range < 256 {
                self.range <<= 1;
                self.nbits -= 1;
            }
            0
        }
    }

    /// The data from the next byte boundary after the current position (used for I_PCM samples).
    pub fn byte_aligned_pos(&self) -> usize {
        self.bit_pos().div_ceil(8)
    }

    /// Re-initialise the engine at byte offset `start` (after I_PCM samples).
    pub fn restart(&mut self, start: usize) {
        *self = CabacDecoder::new(self.data, start);
    }

    /// The underlying data.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }
}

/// The arithmetic encoder of 9.3.4, writing into a [`BitWriter`].
pub struct CabacEncoder {
    low: u32,
    range: u32,
    first_bit: bool,
    outstanding: u32,
    out: BitWriter,
}

impl Default for CabacEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl CabacEncoder {
    /// Start encoding (9.3.4.1).
    pub fn new() -> Self {
        Self { low: 0, range: 510, first_bit: true, outstanding: 0, out: BitWriter::new() }
    }

    fn put_bit(&mut self, b: u32) {
        if self.first_bit {
            self.first_bit = false;
        } else {
            self.out.put_bit(b != 0);
        }
        while self.outstanding > 0 {
            self.out.put_bit(b == 0);
            self.outstanding -= 1;
        }
    }

    fn renorm(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put_bit(0);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put_bit(1);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }

    /// Encode `bin` with the adaptive context `ctx_idx`.
    pub fn decision(&mut self, ctxs: &mut Contexts, ctx_idx: usize, bin: u32) {
        let st = ctxs.state[ctx_idx];
        let (p, mps) = ((st >> 1) as usize, (st & 1) as u32);
        let lps = RANGE_TAB_LPS[p][((self.range >> 6) & 3) as usize] as u32;
        self.range -= lps;
        if bin != mps {
            self.low += self.range;
            self.range = lps;
            let new_mps = if p == 0 { 1 - mps } else { mps };
            ctxs.state[ctx_idx] = (TRANS_IDX_LPS[p] << 1) | new_mps as u8;
        } else {
            ctxs.state[ctx_idx] = (TRANS_IDX_MPS[p] << 1) | mps as u8;
        }
        self.renorm();
    }

    /// Encode a bypass bin.
    pub fn bypass(&mut self, bin: u32) {
        self.low <<= 1;
        if bin != 0 {
            self.low += self.range;
        }
        if self.low >= 1024 {
            self.put_bit(1);
            self.low -= 1024;
        } else if self.low < 512 {
            self.put_bit(0);
        } else {
            self.low -= 512;
            self.outstanding += 1;
        }
    }

    /// Encode a terminating bin; with `bin == 1` the encoder is flushed (end of slice or before I_PCM).
    pub fn terminate(&mut self, bin: u32) {
        self.range -= 2;
        if bin != 0 {
            self.low += self.range;
            // EncodeFlush
            self.range = 2;
            self.renorm();
            self.put_bit((self.low >> 9) & 1);
            self.out.put_bits(((self.low >> 7) & 3) | 1, 2);
        } else {
            self.renorm();
        }
    }

    /// Bytes written so far; valid for appending raw data (I_PCM) only right after `terminate(1)`.
    pub fn bit_len(&self) -> usize {
        self.out.bit_len()
    }

    /// Append raw bits after a flush (I_PCM alignment and samples).
    pub fn writer(&mut self) -> &mut BitWriter {
        &mut self.out
    }

    /// Finish: the flushed bytes (the last bit written by the flush acts as the `rbsp_stop_one_bit`, the rest of
    /// the byte is zero).
    pub fn finish(self) -> alloc::vec::Vec<u8> {
        self.out.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn init_matches_known_states() {
        // ctxIdx 0 for slice QP 26: m=20, n=-15 -> preCtxState = ((20*26)>>4) - 15 = 17 -> pState 46, MPS 0.
        let c = Contexts::new(26, None);
        assert_eq!(c.state[0], 46 << 1);
        // ctxIdx 70 (I slice): m=0, n=11 -> preCtxState 11 -> pState 52, MPS 0.
        assert_eq!(c.state[70], 52 << 1);
    }

    #[test]
    fn encoder_decoder_roundtrip() {
        let mut seed = 7u32;
        for round in 0..50 {
            let n = 1 + lcg(&mut seed) as usize % 3000;
            let qp = (lcg(&mut seed) % 52) as i32;
            let mut ops = Vec::new();
            for _ in 0..n {
                let kind = lcg(&mut seed) % 10;
                let ctx = (lcg(&mut seed) % 460) as usize;
                // Skewed bins so contexts adapt.
                let bin = (lcg(&mut seed) % 100 < 20 + (ctx as u32 % 60)) as u32;
                ops.push((kind, ctx, bin));
            }
            let mut enc = CabacEncoder::new();
            let mut ec = Contexts::new(qp, Some(round % 3));
            for &(kind, ctx, bin) in &ops {
                match kind {
                    0 => enc.bypass(bin),
                    1 => enc.terminate(0),
                    _ => enc.decision(&mut ec, ctx, bin),
                }
            }
            enc.terminate(1);
            let bytes = enc.finish();
            let mut dec = CabacDecoder::new(&bytes, 0);
            let mut dc = Contexts::new(qp, Some(round % 3));
            for (i, &(kind, ctx, bin)) in ops.iter().enumerate() {
                let got = match kind {
                    0 => dec.bypass(),
                    1 => dec.terminate(),
                    _ => dec.decision(&mut dc, ctx),
                };
                let want = if kind == 1 { 0 } else { bin };
                assert_eq!(got, want, "round {round} op {i} kind {kind}");
            }
            assert_eq!(dec.terminate(), 1);
            assert!(!dec.overrun());
            // After the terminating bin the engine is at the end of the stream (stop bit is the last bit).
            assert_eq!(dec.bit_pos() / 8 + 1, bytes.len().max(dec.bit_pos() / 8 + 1));
        }
    }

    #[test]
    fn all_context_states_valid() {
        for qp in [0, 26, 51] {
            for idc in [None, Some(0), Some(1), Some(2)] {
                let c = Contexts::new(qp, idc);
                assert!(c.state.iter().all(|&s| (s >> 1) < 64));
            }
        }
    }
}
