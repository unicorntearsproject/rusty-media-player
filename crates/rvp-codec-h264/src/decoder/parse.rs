//! Parsing a picture's slice data: entropy decoding, motion vector derivation and residual scaling, producing the
//! [`PicJob`] the reconstruction side needs. A picture is parsed by one [`ParseJob`], which can run on any thread,
//! so that several pictures are parsed at once (a picture only waits for the motion of its co-located picture).
use super::mbinfo::MbInfo;
use super::picture::{Motion, MotionSlot};
use super::recon::{PicJob, SliceRecon};
use super::slice::{RefInfo, SliceDecoder, Store};
use crate::bitstream::BitReader;
use crate::error::{Error, Result};
use crate::params::{Pps, SliceHeader, Sps};
use crate::transform::LevelScale;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use rvp_core::par::SpinLock;

/// Runs [`ParseJob`]s (as opaque closures), possibly on other threads. The caller never needs to wait for a job; the
/// runner may block in `spawn` to keep the number of unfinished jobs small.
pub trait ParseRunner {
    /// Start `job`.
    fn spawn(&mut self, job: Box<dyn FnOnce() + Send + 'static>);
}

/// Counters shared with the jobs.
#[derive(Default)]
pub struct StatsCell {
    pub(crate) pictures: AtomicU64,
    pub(crate) slices: AtomicU64,
    pub(crate) errors: AtomicU64,
    pub(crate) concealed_mbs: AtomicU64,
}

/// One slice of a picture, with everything its parsing needs.
pub(crate) struct SliceWork {
    pub(crate) rbsp: Vec<u8>,
    pub(crate) hdr: SliceHeader,
    pub(crate) sps: Arc<Sps>,
    pub(crate) pps: Arc<Pps>,
    pub(crate) ls: Arc<LevelScale>,
    pub(crate) refs: [Vec<RefInfo>; 2],
    pub(crate) cur_poc: i32,
    /// Motion of `RefPicList1[0]`, for B slices.
    pub(crate) col: Option<Arc<MotionSlot>>,
    /// What the reconstruction side needs to know about the slice.
    pub(crate) recon: SliceRecon,
}

/// Where a parsed picture is delivered.
#[derive(Default)]
pub struct JobSlot {
    job: SpinLock<Option<Box<PicJob>>>,
}

impl JobSlot {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn set(&self, j: Box<PicJob>) {
        *self.job.lock() = Some(j);
    }

    /// The parsed picture, waiting for its parsing to finish.
    pub(crate) fn wait(&self) -> Box<PicJob> {
        loop {
            if let Some(j) = self.job.lock().take() {
                return j;
            }
            rvp_core::par::relax();
        }
    }
}

/// Parses all slices of one picture.
pub(crate) struct ParseJob {
    pub(crate) uid: i32,
    pub(crate) mbw: usize,
    pub(crate) mbh: usize,
    pub(crate) slices: Vec<SliceWork>,
    pub(crate) conceal_src: Option<i32>,
    pub(crate) motion_slot: Arc<MotionSlot>,
    pub(crate) out: Arc<JobSlot>,
    pub(crate) stats: Arc<StatsCell>,
    /// The decoder's reset counter, and its value when this job was created: a job from before a reset does nothing.
    pub(crate) epoch: Arc<AtomicU64>,
    pub(crate) my_epoch: u64,
}

impl ParseJob {
    /// Parse the picture and deliver it. Returns the first error of any slice (the picture is delivered regardless:
    /// macroblocks that could not be parsed are concealed).
    pub(crate) fn run(self) -> Result<()> {
        let ParseJob { uid, mbw, mbh, slices, conceal_src, motion_slot, out, stats, epoch, my_epoch } = self;
        let mut mbs = vec![MbInfo::EMPTY; mbw * mbh];
        let mut motion = Motion::new(mbw, mbh);
        let mut store = Store::default();
        let mut recon_slices = Vec::with_capacity(slices.len());
        let mut first_err: Option<Error> = None;
        let mut cancelled = epoch.load(Ordering::Acquire) != my_epoch;
        for (i, w) in slices.into_iter().enumerate() {
            if cancelled {
                break;
            }
            let mut sd = SliceDecoder::new(
                &w.sps,
                &w.pps,
                &w.hdr,
                &w.ls,
                &mut motion,
                &mut mbs,
                w.col,
                w.refs,
                w.cur_poc,
                i as u16 + 1,
                &mut store,
            );
            let r = if w.pps.cabac {
                sd.decode_cabac(&w.rbsp, w.hdr.data_bit_pos.div_ceil(8))
            } else {
                let mut br = BitReader::new(&w.rbsp);
                br.skip(w.hdr.data_bit_pos as u32);
                sd.decode_cavlc(br)
            };
            match r {
                Ok(()) => {
                    stats.slices.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                    first_err.get_or_insert(e);
                }
            }
            recon_slices.push(w.recon);
            cancelled = epoch.load(Ordering::Acquire) != my_epoch;
        }
        let missing = mbs.iter().filter(|m| m.slice == 0).count();
        if missing > 0 && !cancelled {
            stats.concealed_mbs.fetch_add(missing as u64, Ordering::Relaxed);
        }
        let motion = Arc::new(motion);
        motion_slot.set(motion.clone());
        out.set(Box::new(PicJob {
            uid,
            mbw,
            mbh,
            cancelled,
            mbs,
            motion,
            slices: recon_slices,
            coefs: store.coefs,
            pcm: store.pcm,
            conceal_src,
        }));
        first_err.map_or(Ok(()), Err)
    }
}
