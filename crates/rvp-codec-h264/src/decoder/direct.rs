//! Direct prediction for B macroblocks (8.4.1.2).
use super::slice::SliceDecoder;
use crate::error::{Error, Result};

impl SliceDecoder<'_> {
    /// Derive motion for the direct 8x8 blocks in `mask` (bit per 8x8 block) of the current macroblock.
    pub(crate) fn direct_mb(&mut self, _mask: u8) -> Result<()> {
        Err(Error::Unsupported("direct prediction"))
    }
}
