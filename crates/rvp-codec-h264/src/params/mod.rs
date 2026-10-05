//! Parameter sets and slice headers: parsing and writing.
//!
//! The structs are plain data. A decoder fills them with `parse`; an encoder builds them by hand and calls
//! `write`. Both directions use the same field names as the specification.

pub mod pps;
pub mod scaling;
pub mod slice;
pub mod sps;

pub use pps::Pps;
pub use scaling::{ScalingMatrices, ScalingSyntax};
pub use slice::{DecRefPicMarking, Mmco, PredWeightTable, RefListMod, SliceHeader, SliceType, WeightEntry};
pub use sps::{Crop, Sps, Vui};

use alloc::rc::Rc;
use alloc::vec::Vec;

/// The sets of active-able parameter sets received so far, by id.
#[derive(Debug, Default, Clone)]
pub struct ParamSets {
    sps: Vec<Option<Rc<Sps>>>,
    pps: Vec<Option<Rc<Pps>>>,
}

impl ParamSets {
    /// No parameter sets.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store (or replace) an SPS.
    pub fn insert_sps(&mut self, sps: Sps) {
        let id = sps.id as usize;
        if self.sps.len() <= id {
            self.sps.resize(id + 1, None);
        }
        self.sps[id] = Some(Rc::new(sps));
    }

    /// Store (or replace) a PPS.
    pub fn insert_pps(&mut self, pps: Pps) {
        let id = pps.id as usize;
        if self.pps.len() <= id {
            self.pps.resize(id + 1, None);
        }
        self.pps[id] = Some(Rc::new(pps));
    }

    /// Look up an SPS.
    pub fn sps(&self, id: u32) -> Option<&Rc<Sps>> {
        self.sps.get(id as usize)?.as_ref()
    }

    /// Look up a PPS.
    pub fn pps(&self, id: u32) -> Option<&Rc<Pps>> {
        self.pps.get(id as usize)?.as_ref()
    }
}
