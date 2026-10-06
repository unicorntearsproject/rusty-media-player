//! Picture parameter set (7.3.2.2).
use super::ParamSets;
use super::scaling::ScalingSyntax;
use crate::bitstream::{BitReader, BitWriter};
use crate::error::{Error, Result};
use alloc::vec::Vec;

/// A picture parameter set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pps {
    /// `pic_parameter_set_id`.
    pub id: u32,
    /// `seq_parameter_set_id`.
    pub sps_id: u32,
    /// `entropy_coding_mode_flag`: true for CABAC.
    pub cabac: bool,
    /// `bottom_field_pic_order_in_frame_present_flag`.
    pub bottom_field_pic_order_in_frame_present: bool,
    /// `num_slice_groups_minus1 + 1` (only 1 is decodable).
    pub num_slice_groups: u32,
    /// `num_ref_idx_l0_default_active_minus1 + 1`.
    pub num_ref_idx_l0_default: u32,
    /// `num_ref_idx_l1_default_active_minus1 + 1`.
    pub num_ref_idx_l1_default: u32,
    /// `weighted_pred_flag`.
    pub weighted_pred: bool,
    /// `weighted_bipred_idc` (0 default, 1 explicit, 2 implicit).
    pub weighted_bipred_idc: u32,
    /// `pic_init_qp_minus26 + 26`.
    pub pic_init_qp: i32,
    /// `pic_init_qs_minus26 + 26`.
    pub pic_init_qs: i32,
    /// `chroma_qp_index_offset`.
    pub chroma_qp_index_offset: i32,
    /// `deblocking_filter_control_present_flag`.
    pub deblocking_filter_control_present: bool,
    /// `constrained_intra_pred_flag`.
    pub constrained_intra_pred: bool,
    /// `redundant_pic_cnt_present_flag`.
    pub redundant_pic_cnt_present: bool,
    /// `transform_8x8_mode_flag`.
    pub transform_8x8_mode: bool,
    /// The picture scaling matrix, if `pic_scaling_matrix_present_flag`.
    pub scaling: Option<ScalingSyntax>,
    /// `second_chroma_qp_index_offset` (equals `chroma_qp_index_offset` when not signalled).
    pub second_chroma_qp_index_offset: i32,
    /// Whether the High-profile extension fields were present in the RBSP (they are written only then).
    pub has_extension: bool,
}

impl Pps {
    /// A baseline-style PPS with all defaults, for building one by hand.
    pub fn new(id: u32, sps_id: u32) -> Self {
        Self {
            id,
            sps_id,
            cabac: false,
            bottom_field_pic_order_in_frame_present: false,
            num_slice_groups: 1,
            num_ref_idx_l0_default: 1,
            num_ref_idx_l1_default: 1,
            weighted_pred: false,
            weighted_bipred_idc: 0,
            pic_init_qp: 26,
            pic_init_qs: 26,
            chroma_qp_index_offset: 0,
            deblocking_filter_control_present: false,
            constrained_intra_pred: false,
            redundant_pic_cnt_present: false,
            transform_8x8_mode: false,
            scaling: None,
            second_chroma_qp_index_offset: 0,
            has_extension: false,
        }
    }

    /// Parse a PPS RBSP. The SPS it refers to should already be in `sets` (it decides how many scaling lists
    /// follow); if it is missing, 4:2:0 is assumed.
    pub fn parse(rbsp: &[u8], sets: &ParamSets) -> Result<Self> {
        let mut r = BitReader::new(rbsp);
        let id = r.read_ue()?;
        let sps_id = r.read_ue()?;
        if id > 255 || sps_id > 31 {
            return Err(Error::Invalid("parameter set id out of range"));
        }
        let mut p = Pps::new(id, sps_id);
        p.cabac = r.read_flag()?;
        p.bottom_field_pic_order_in_frame_present = r.read_flag()?;
        p.num_slice_groups = r.read_ue()?.saturating_add(1);
        if p.num_slice_groups > 1 {
            // FMO: the slice group syntax is not parsed; the decoder rejects the stream.
            return Ok(p);
        }
        let l0 = r.read_ue()?;
        let l1 = r.read_ue()?;
        if l0 > 31 || l1 > 31 {
            return Err(Error::Invalid("num_ref_idx_default out of range"));
        }
        p.num_ref_idx_l0_default = l0 + 1;
        p.num_ref_idx_l1_default = l1 + 1;
        p.weighted_pred = r.read_flag()?;
        p.weighted_bipred_idc = r.read_bits(2)?;
        p.pic_init_qp = 26i32
            .checked_add(r.read_se()?)
            .filter(|q| (0..=51).contains(q))
            .ok_or(Error::Invalid("pic_init_qp out of range"))?;
        p.pic_init_qs = 26 + r.read_se()?;
        p.chroma_qp_index_offset = r.read_se()?;
        if !(-12..=12).contains(&p.chroma_qp_index_offset) {
            return Err(Error::Invalid("chroma_qp_index_offset out of range"));
        }
        p.second_chroma_qp_index_offset = p.chroma_qp_index_offset;
        p.deblocking_filter_control_present = r.read_flag()?;
        p.constrained_intra_pred = r.read_flag()?;
        p.redundant_pic_cnt_present = r.read_flag()?;
        if r.more_rbsp_data() {
            p.has_extension = true;
            p.transform_8x8_mode = r.read_flag()?;
            if r.read_flag()? {
                let chroma_format = sets.sps(sps_id).map(|s| s.chroma_format_idc).unwrap_or(1);
                let n = 6 + if chroma_format != 3 { 2 } else { 6 } * p.transform_8x8_mode as usize;
                p.scaling = Some(ScalingSyntax::parse(&mut r, n)?);
            }
            p.second_chroma_qp_index_offset = r.read_se()?;
            if !(-12..=12).contains(&p.second_chroma_qp_index_offset) {
                return Err(Error::Invalid("second_chroma_qp_index_offset out of range"));
            }
        }
        Ok(p)
    }

    /// Serialise as a PPS RBSP including trailing bits.
    pub fn write(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put_ue(self.id);
        w.put_ue(self.sps_id);
        w.put_bit(self.cabac);
        w.put_bit(self.bottom_field_pic_order_in_frame_present);
        w.put_ue(self.num_slice_groups - 1);
        w.put_ue(self.num_ref_idx_l0_default - 1);
        w.put_ue(self.num_ref_idx_l1_default - 1);
        w.put_bit(self.weighted_pred);
        w.put_bits(self.weighted_bipred_idc, 2);
        w.put_se(self.pic_init_qp - 26);
        w.put_se(self.pic_init_qs - 26);
        w.put_se(self.chroma_qp_index_offset);
        w.put_bit(self.deblocking_filter_control_present);
        w.put_bit(self.constrained_intra_pred);
        w.put_bit(self.redundant_pic_cnt_present);
        if self.has_extension || self.transform_8x8_mode || self.scaling.is_some() {
            w.put_bit(self.transform_8x8_mode);
            match &self.scaling {
                Some(s) => {
                    w.put_bit(true);
                    s.write(&mut w);
                }
                None => w.put_bit(false),
            }
            w.put_se(self.second_chroma_qp_index_offset);
        }
        w.put_trailing_bits();
        w.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut p = Pps::new(5, 2);
        p.cabac = true;
        p.num_ref_idx_l0_default = 4;
        p.weighted_pred = true;
        p.weighted_bipred_idc = 2;
        p.pic_init_qp = 20;
        p.chroma_qp_index_offset = -2;
        p.second_chroma_qp_index_offset = -2;
        p.deblocking_filter_control_present = true;
        p.transform_8x8_mode = true;
        p.has_extension = true;
        let rbsp = p.write();
        let q = Pps::parse(&rbsp, &ParamSets::new()).unwrap();
        assert_eq!(p, q);
        for n in 0..rbsp.len() {
            let _ = Pps::parse(&rbsp[..n], &ParamSets::new());
        }
    }
}
