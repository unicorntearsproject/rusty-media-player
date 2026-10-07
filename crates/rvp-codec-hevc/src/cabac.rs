//! The arithmetic decoding engine of 9.3.4.3 (the same engine as H.264's) and the context state of a slice.
use crate::ctx;

/// `rangeTabLPS[pStateIdx][qCodIRangeIdx]` (Table 9-52).
pub const RANGE_TAB_LPS: [[u8; 4]; 64] = [
    [128, 176, 208, 240],
    [128, 167, 197, 227],
    [128, 158, 187, 216],
    [123, 150, 178, 205],
    [116, 142, 169, 195],
    [111, 135, 160, 185],
    [105, 128, 152, 175],
    [100, 122, 144, 166],
    [95, 116, 137, 158],
    [90, 110, 130, 150],
    [85, 104, 123, 142],
    [81, 99, 117, 135],
    [77, 94, 111, 128],
    [73, 89, 105, 122],
    [69, 85, 100, 116],
    [66, 80, 95, 110],
    [62, 76, 90, 104],
    [59, 72, 86, 99],
    [56, 69, 81, 94],
    [53, 65, 77, 89],
    [51, 62, 73, 85],
    [48, 59, 69, 80],
    [46, 56, 66, 76],
    [43, 53, 63, 72],
    [41, 50, 59, 69],
    [39, 48, 56, 65],
    [37, 45, 54, 62],
    [35, 43, 51, 59],
    [33, 41, 48, 56],
    [32, 39, 46, 53],
    [30, 37, 43, 50],
    [29, 35, 41, 48],
    [27, 33, 39, 45],
    [26, 31, 37, 43],
    [24, 30, 35, 41],
    [23, 28, 33, 39],
    [22, 27, 32, 37],
    [21, 26, 30, 35],
    [20, 24, 29, 33],
    [19, 23, 27, 31],
    [18, 22, 26, 30],
    [17, 21, 25, 28],
    [16, 20, 23, 27],
    [15, 19, 22, 25],
    [14, 18, 21, 24],
    [14, 17, 20, 23],
    [13, 16, 19, 22],
    [12, 15, 18, 21],
    [12, 14, 17, 20],
    [11, 14, 16, 19],
    [11, 13, 15, 18],
    [10, 12, 15, 17],
    [10, 12, 14, 16],
    [9, 11, 13, 15],
    [9, 11, 12, 14],
    [8, 10, 12, 14],
    [8, 9, 11, 13],
    [7, 9, 11, 12],
    [7, 9, 10, 12],
    [7, 8, 10, 11],
    [6, 8, 9, 11],
    [6, 7, 9, 10],
    [6, 7, 8, 9],
    [2, 2, 2, 2],
];

/// `transIdxLPS` (Table 9-53).
pub const TRANS_IDX_LPS: [u8; 64] = [
    0, 0, 1, 2, 2, 4, 4, 5, 6, 7, 8, 9, 9, 11, 11, 12, 13, 13, 15, 15, 16, 16, 18, 18, 19, 19, 21, 21, 22,
    22, 23, 24, 24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33, 33, 33, 34, 34, 35, 35, 35,
    36, 36, 36, 37, 37, 37, 38, 38, 63,
];

/// `transIdxMPS` (Table 9-53).
pub const TRANS_IDX_MPS: [u8; 64] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
    29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54,
    55, 56, 57, 58, 59, 60, 61, 62, 62, 63,
];

/// Next context state byte (`pStateIdx << 1 | valMps`) after the most probable symbol.
const NEXT_MPS: [u8; 128] = {
    let mut t = [0u8; 128];
    let mut s = 0;
    while s < 128 {
        t[s] = (TRANS_IDX_MPS[s >> 1] << 1) | (s & 1) as u8;
        s += 1;
    }
    t
};

/// Next context state byte after the least probable symbol (the MPS flips at state 0).
const NEXT_LPS: [u8; 128] = {
    let mut t = [0u8; 128];
    let mut s = 0;
    while s < 128 {
        let mps = (s & 1) as u8;
        let new_mps = if s >> 1 == 0 { 1 - mps } else { mps };
        t[s] = (TRANS_IDX_LPS[s >> 1] << 1) | new_mps;
        s += 1;
    }
    t
};

/// Both transitions in one table: index `state` after an MPS, `state + 128` after an LPS.
const NEXT: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut s = 0;
    while s < 128 {
        t[s] = NEXT_MPS[s];
        t[s + 128] = NEXT_LPS[s];
        s += 1;
    }
    t
};

/// `rangeTabLps` flattened for a state byte: index `(state >> 1) * 4 + qCodIRangeIdx`.
const LPS_FLAT: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = RANGE_TAB_LPS[i >> 2][i & 3];
        i += 1;
    }
    t
};

/// The adaptive context states of a slice segment: `pStateIdx << 1 | valMps` per context.
#[derive(Clone)]
pub struct Contexts {
    state: [u8; ctx::COUNT],
}

impl Default for Contexts {
    fn default() -> Self {
        Self { state: [0; ctx::COUNT] }
    }
}

impl Contexts {
    /// Initialise every context (9.3.2.2) for a slice of `init_type` (0 I, 1 or 2 for P and B) and `SliceQpY`.
    pub fn init(&mut self, init_type: usize, slice_qp: i32) {
        let qp = slice_qp.clamp(0, 51);
        for (i, st) in self.state.iter_mut().enumerate() {
            let v = ctx::INIT_VALUES[init_type.min(2)][i] as i32;
            let (slope, offset) = (v >> 4, v & 15);
            let (m, n) = (slope * 5 - 45, (offset << 3) - 16);
            let pre = (((m * qp) >> 4) + n).clamp(1, 126);
            *st = if pre <= 63 { ((63 - pre) as u8) << 1 } else { (((pre - 64) as u8) << 1) | 1 };
        }
    }
}

/// The arithmetic decoder.
#[derive(Clone)]
pub struct Cabac<'a> {
    data: &'a [u8],
    /// Next byte to load.
    pos: usize,
    /// `ivlOffset << nbits | prefetched bits`.
    value: u64,
    /// Number of prefetched bits in `value`.
    nbits: u32,
    /// `ivlCurrRange`.
    range: u32,
    /// Bytes read past the end (zeros), so a truncated slice is noticed.
    overrun_bits: u32,
}

impl<'a> Cabac<'a> {
    /// Initialise the engine (9.3.2.5) reading `data` from byte `start`.
    pub fn new(data: &'a [u8], start: usize) -> Self {
        let mut d = Self { data, pos: start, value: 0, nbits: 0, range: 510, overrun_bits: 0 };
        d.refill();
        d.refill();
        d.nbits -= 9;
        d
    }

    #[inline(always)]
    fn refill(&mut self) {
        if let Some(&[hi, lo]) = self.data.get(self.pos..).and_then(|d| d.first_chunk::<2>()) {
            self.value = (self.value << 16) | ((hi as u64) << 8) | lo as u64;
        } else {
            let hi = self.data.get(self.pos).copied();
            let lo = self.data.get(self.pos + 1).copied();
            if hi.is_none() {
                self.overrun_bits += 8;
            }
            if lo.is_none() {
                self.overrun_bits += 8;
            }
            self.value = (self.value << 16) | ((hi.unwrap_or(0) as u64) << 8) | lo.unwrap_or(0) as u64;
        }
        self.pos += 2;
        self.nbits += 16;
    }

    /// True if the engine consumed bits beyond the end of the data.
    pub fn overrun(&self) -> bool {
        self.overrun_bits > self.nbits
    }

    /// One past the last bit read into `ivlOffset`.
    pub fn bit_pos(&self) -> usize {
        self.pos * 8 - self.nbits as usize
    }

    /// Decode a bin with the adaptive context `idx`.
    #[inline(always)]
    pub fn decision(&mut self, ctxs: &mut Contexts, idx: usize) -> u32 {
        if self.nbits < 8 {
            self.refill();
        }
        let slot = &mut ctxs.state[idx];
        let st = (*slot & 127) as usize;
        let lps = LPS_FLAT[(st & !1) * 2 + ((self.range >> 6) & 3) as usize] as u32;
        let rmps = self.range - lps;
        let scaled = (rmps as u64) << self.nbits;
        let is_lps = (self.value >= scaled) as u64;
        self.value -= scaled & is_lps.wrapping_neg();
        *slot = NEXT[st + ((is_lps as usize) << 7)];
        let range = if is_lps != 0 { lps } else { rmps };
        let shift = range.leading_zeros() - 23;
        self.range = range << shift;
        self.nbits -= shift;
        (st & 1) as u32 ^ is_lps as u32
    }

    /// Decode a bypass bin.
    #[inline(always)]
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

    /// `n` bypass bins as an unsigned number, most significant first.
    #[inline]
    pub fn bypass_bits(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bypass();
        }
        v
    }

    /// Decode a terminating bin (end of slice segment, end of sub-stream, PCM flag).
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

    /// The byte after the arithmetic code, where PCM samples or the next sub-stream begin.
    pub fn byte_aligned_pos(&self) -> usize {
        self.bit_pos().div_ceil(8)
    }

    /// Restart the engine at byte `start` (after PCM samples, or at an entry point).
    pub fn restart(&mut self, start: usize) {
        *self = Cabac::new(self.data, start);
    }
}
