//! Per-macroblock bookkeeping shared by parsing, prediction and deblocking.

/// The macroblock is intra coded (I_NxN, I_16x16 or I_PCM).
pub const F_INTRA: u16 = 1;
/// Intra 16x16.
pub const F_I16: u16 = 2;
/// I_PCM.
pub const F_PCM: u16 = 4;
/// P_Skip or B_Skip.
pub const F_SKIP: u16 = 8;
/// `transform_size_8x8_flag`.
pub const F_T8X8: u16 = 16;
/// B_Skip or B_Direct_16x16 (all four 8x8 blocks are direct).
pub const F_DIRECT: u16 = 32;
/// Intra 4x4.
pub const F_I4: u16 = 64;
/// Intra 8x8.
pub const F_I8: u16 = 128;

/// Everything later macroblocks and the deblocking filter need to know about a decoded macroblock.
#[derive(Clone, Copy)]
pub struct MbInfo {
    /// 1-based slice number within the picture; 0 means "not decoded".
    pub slice: u16,
    /// `F_*` flags.
    pub flags: u16,
    /// `QPY` for deblocking (0 for I_PCM).
    pub qp: i8,
    /// `QPc` for Cb and Cr, derived from `qp` for deblocking.
    pub qpc: [i8; 2],
    /// Coded block pattern: luma bits 0..3, chroma in bits 4..5.
    pub cbp: u8,
    /// `intra_chroma_pred_mode`.
    pub chroma_mode: u8,
    /// Bit per 8x8 block that is predicted in direct mode (B slices).
    pub direct8: u8,
    /// Coded-block-flag bits of the DC blocks: bit 0 Intra16x16 luma DC, bit 1 Cb DC, bit 2 Cr DC.
    pub cbf_dc: u8,
    /// Coefficient counts per 4x4 block: luma raster 0..16, Cb 16..20, Cr 20..24.
    pub nz: [u8; 24],
    /// Intra 4x4/8x8 prediction modes per 4x4 block, raster order (`-1` if the MB is not I4x4/I8x8).
    pub ipm: [i8; 16],
    /// Absolute motion vector differences per list and 4x4 block, saturated (CABAC contexts).
    pub mvd: [[[u8; 2]; 16]; 2],
    /// Bit per luma 4x4 block (raster) that has non-zero coefficients (8x8 transform blocks set all four bits).
    pub nzmask: u16,
}

impl MbInfo {
    /// A macroblock that has not been decoded.
    pub const EMPTY: MbInfo = MbInfo {
        slice: 0,
        flags: 0,
        qp: 0,
        qpc: [0; 2],
        cbp: 0,
        chroma_mode: 0,
        direct8: 0,
        cbf_dc: 0,
        nz: [0; 24],
        ipm: [-1; 16],
        mvd: [[[0; 2]; 16]; 2],
        nzmask: 0,
    };

    /// True if intra coded.
    #[inline]
    pub fn is_intra(&self) -> bool {
        self.flags & F_INTRA != 0
    }
}

/// Block index in decoding order (`luma4x4BlkIdx`) to 4x4 raster `(x, y)` inside the macroblock.
#[inline]
pub fn blk_xy(idx: usize) -> (usize, usize) {
    (((idx >> 2) & 1) * 2 + (idx & 1), ((idx >> 3) & 1) * 2 + ((idx >> 1) & 1))
}

/// Decoding order of a 4x4 block at raster position `(x, y)`.
#[inline]
pub fn blk_order(x: usize, y: usize) -> usize {
    ((y >> 1) * 2 + (x >> 1)) * 4 + (y & 1) * 2 + (x & 1)
}
