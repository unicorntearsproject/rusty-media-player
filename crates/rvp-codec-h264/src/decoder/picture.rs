//! Decoded pictures, with the motion data later pictures need (direct prediction, deblocking).
use alloc::vec;
use alloc::vec::Vec;

/// Reference marking of a stored picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefState {
    /// Not used for reference.
    Unused,
    /// Short-term reference.
    Short,
    /// Long-term reference.
    Long,
}

/// A frame store: samples plus per-block motion data and the bookkeeping of reference marking and output.
#[derive(Clone)]
pub struct Picture {
    /// Width in macroblocks.
    pub mbw: usize,
    /// Height in macroblocks.
    pub mbh: usize,
    /// Y, Cb, Cr sample planes, padded to whole macroblocks.
    pub planes: [Vec<u8>; 3],
    /// Row strides of the planes.
    pub strides: [usize; 3],
    /// Motion vectors per 4x4 block, both lists, in a `4*mbw` wide grid.
    pub mv: [Vec<[i16; 2]>; 2],
    /// Reference indices per 8x8 block (`-1`: list unused), in a `2*mbw` wide grid.
    pub ref_idx: [Vec<i8>; 2],
    /// Unique id of the frame each 8x8 block referenced (`-1`: none).
    pub ref_id: [Vec<i32>; 2],
    /// Unique id of this frame store use.
    pub uid: i32,
    /// `PicOrderCnt` of the frame.
    pub poc: i32,
    /// `frame_num`.
    pub frame_num: u32,
    /// Reference marking.
    pub ref_state: RefState,
    /// `LongTermFrameIdx` when long-term.
    pub long_term_idx: u32,
    /// Waiting to be output.
    pub needed_for_output: bool,
    /// Presentation timestamp from the container.
    pub pts: i64,
    /// A "non-existing" frame inserted for a `frame_num` gap.
    pub non_existing: bool,
    /// Decoding order counter (for tie-breaks and concealment).
    pub decode_order: u64,
}

impl Picture {
    /// Allocate a picture of the given size in macroblocks, filled with mid grey.
    pub fn new(mbw: usize, mbh: usize) -> Self {
        let (w, h) = (mbw * 16, mbh * 16);
        let n4 = mbw * 4 * mbh * 4;
        let n8 = mbw * 2 * mbh * 2;
        Self {
            mbw,
            mbh,
            planes: [vec![128; w * h], vec![128; w * h / 4], vec![128; w * h / 4]],
            strides: [w, w / 2, w / 2],
            mv: [vec![[0; 2]; n4], vec![[0; 2]; n4]],
            ref_idx: [vec![-1; n8], vec![-1; n8]],
            ref_id: [vec![-1; n8], vec![-1; n8]],
            uid: 0,
            poc: 0,
            frame_num: 0,
            ref_state: RefState::Unused,
            long_term_idx: 0,
            needed_for_output: false,
            pts: 0,
            non_existing: false,
            decode_order: 0,
        }
    }

    /// True if this picture can be reused for the given size.
    pub fn fits(&self, mbw: usize, mbh: usize) -> bool {
        self.mbw == mbw && self.mbh == mbh
    }

    /// Reset the bookkeeping and motion data for reuse as a new picture (samples are overwritten by decoding).
    pub fn reset(&mut self) {
        for l in 0..2 {
            self.mv[l].fill([0; 2]);
            self.ref_idx[l].fill(-1);
            self.ref_id[l].fill(-1);
        }
        self.ref_state = RefState::Unused;
        self.needed_for_output = false;
        self.non_existing = false;
        self.long_term_idx = 0;
    }

    /// Fill all samples with grey.
    pub fn fill_grey(&mut self) {
        for p in self.planes.iter_mut() {
            p.fill(128);
        }
    }

    /// True if the picture is marked as a reference (short or long term).
    #[inline]
    pub fn is_ref(&self) -> bool {
        self.ref_state != RefState::Unused
    }
}
