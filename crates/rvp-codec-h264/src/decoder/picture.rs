//! Decoded pictures, with the motion data later pictures need (direct prediction, deblocking).
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use rvp_core::par::SpinLock;

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

/// The motion data of a picture (what direct prediction, motion compensation and deblocking read).
#[derive(Clone)]
pub struct Motion {
    /// Width in macroblocks.
    pub mbw: usize,
    /// Height in macroblocks.
    pub mbh: usize,
    /// Motion vectors per 4x4 block, both lists, in a `4*mbw` wide grid.
    pub mv: [Vec<[i16; 2]>; 2],
    /// Reference indices per 8x8 block (`-1`: list unused), in a `2*mbw` wide grid.
    pub ref_idx: [Vec<i8>; 2],
    /// Unique id of the frame each 8x8 block referenced (`-1`: none).
    pub ref_id: [Vec<i32>; 2],
}

impl Motion {
    /// Motion data for a picture of the given size: every block unpredicted (like an intra picture).
    pub fn new(mbw: usize, mbh: usize) -> Self {
        let n4 = mbw * 4 * mbh * 4;
        let n8 = mbw * 2 * mbh * 2;
        Self {
            mbw,
            mbh,
            mv: [vec![[0; 2]; n4], vec![[0; 2]; n4]],
            ref_idx: [vec![-1; n8], vec![-1; n8]],
            ref_id: [vec![-1; n8], vec![-1; n8]],
        }
    }
}

/// Where the motion of a picture appears once its slices are parsed (possibly on another thread). Pictures that
/// reference it for direct prediction wait for it.
#[derive(Default)]
pub struct MotionSlot {
    cell: SpinLock<Option<Arc<Motion>>>,
}

impl MotionSlot {
    /// An empty slot.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A slot that already holds `m`.
    pub fn filled(m: Motion) -> Arc<Self> {
        let s = Self::new();
        s.set(Arc::new(m));
        s
    }

    /// Publish the motion.
    pub fn set(&self, m: Arc<Motion>) {
        *self.cell.lock() = Some(m);
    }

    /// The motion, waiting for the parsing of its picture if it is not there yet.
    pub fn wait(&self) -> Arc<Motion> {
        let mut b = rvp_core::par::Backoff::new();
        loop {
            if let Some(m) = self.cell.lock().as_ref() {
                return m.clone();
            }
            b.wait();
        }
    }
}

/// A decoded picture's samples, padded to whole macroblocks. The reconstruction side owns these; the parsing side only
/// knows pictures by their `uid`.
pub struct PlanePic {
    /// `Picture::uid` of the picture these samples belong to.
    pub uid: i32,
    /// Width in macroblocks.
    pub mbw: usize,
    /// Height in macroblocks.
    pub mbh: usize,
    /// Y, Cb, Cr sample planes.
    pub planes: [Vec<u8>; 3],
    /// Row strides of the planes.
    pub strides: [usize; 3],
}

impl PlanePic {
    /// Allocate a picture of the given size in macroblocks, filled with mid grey.
    pub fn new(mbw: usize, mbh: usize) -> Self {
        let (w, h) = (mbw * 16, mbh * 16);
        Self {
            uid: 0,
            mbw,
            mbh,
            planes: [vec![128; w * h], vec![128; w * h / 4], vec![128; w * h / 4]],
            strides: [w, w / 2, w / 2],
        }
    }

    /// True if this picture can be reused for the given size.
    pub fn fits(&self, mbw: usize, mbh: usize) -> bool {
        self.mbw == mbw && self.mbh == mbh
    }
}

/// A frame store of the parsing side: the bookkeeping of reference marking and output, and where the picture's motion
/// data will be.
#[derive(Clone)]
pub struct Picture {
    /// Width in macroblocks.
    pub mbw: usize,
    /// Height in macroblocks.
    pub mbh: usize,
    /// The motion data, once parsed.
    pub motion: Arc<MotionSlot>,
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
    /// A picture of the given size in macroblocks whose motion data is still to come.
    pub fn new(mbw: usize, mbh: usize) -> Self {
        Self {
            mbw,
            mbh,
            motion: MotionSlot::new(),
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

    /// Reset the bookkeeping for reuse as a new picture.
    pub fn reset(&mut self) {
        self.motion = MotionSlot::new();
        self.ref_state = RefState::Unused;
        self.needed_for_output = false;
        self.non_existing = false;
        self.long_term_idx = 0;
    }

    /// True if the picture is marked as a reference (short or long term).
    #[inline]
    pub fn is_ref(&self) -> bool {
        self.ref_state != RefState::Unused
    }
}
