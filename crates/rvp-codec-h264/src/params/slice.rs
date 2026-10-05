//! Slice header (7.3.3) with reference list modification, prediction weight table and reference marking.
use super::{ParamSets, Pps, Sps};
use crate::bitstream::{BitReader, BitWriter, NalHeader, NalUnitType};
use crate::error::{Error, Result};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Slice type (`slice_type % 5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceType {
    /// Predictive.
    P,
    /// Bi-predictive.
    B,
    /// Intra.
    I,
    /// Switching predictive (Extended profile; not decodable here).
    Sp,
    /// Switching intra (Extended profile; not decodable here).
    Si,
}

impl SliceType {
    /// From `slice_type % 5`.
    pub fn from_u32(v: u32) -> Self {
        match v % 5 {
            0 => Self::P,
            1 => Self::B,
            2 => Self::I,
            3 => Self::Sp,
            _ => Self::Si,
        }
    }

    /// The value 0 to 4.
    pub fn to_u32(self) -> u32 {
        match self {
            Self::P => 0,
            Self::B => 1,
            Self::I => 2,
            Self::Sp => 3,
            Self::Si => 4,
        }
    }

    /// True for I and SI.
    pub fn is_intra(self) -> bool {
        matches!(self, Self::I | Self::Si)
    }
}

/// One `modification_of_pic_nums_idc` operation (0 to 2; idc 3 ends the list and is implicit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefListMod {
    /// `modification_of_pic_nums_idc`: 0 subtract, 1 add, 2 long-term.
    pub idc: u32,
    /// `abs_diff_pic_num_minus1` or `long_term_pic_num`.
    pub value: u32,
}

/// One memory management control operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mmco {
    /// 1: mark a short-term picture unused.
    ShortTermUnused {
        /// `difference_of_pic_nums_minus1`.
        difference_of_pic_nums_minus1: u32,
    },
    /// 2: mark a long-term picture unused.
    LongTermUnused {
        /// `long_term_pic_num`.
        long_term_pic_num: u32,
    },
    /// 3: assign a long-term frame index to a short-term picture.
    AssignLongTerm {
        /// `difference_of_pic_nums_minus1`.
        difference_of_pic_nums_minus1: u32,
        /// `long_term_frame_idx`.
        long_term_frame_idx: u32,
    },
    /// 4: set the maximum long-term frame index.
    MaxLongTermIdx {
        /// `max_long_term_frame_idx_plus1`.
        max_long_term_frame_idx_plus1: u32,
    },
    /// 5: mark everything unused and reset POC state.
    ClearAll,
    /// 6: mark the current picture long-term.
    CurrentLongTerm {
        /// `long_term_frame_idx`.
        long_term_frame_idx: u32,
    },
}

/// `dec_ref_pic_marking()`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DecRefPicMarking {
    /// IDR only: `no_output_of_prior_pics_flag`.
    pub no_output_of_prior_pics: bool,
    /// IDR only: `long_term_reference_flag`.
    pub long_term_reference: bool,
    /// Non-IDR: `adaptive_ref_pic_marking_mode_flag`.
    pub adaptive: bool,
    /// The operations, when `adaptive`.
    pub ops: Vec<Mmco>,
}

/// Explicit weighted prediction parameters for one reference index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeightEntry {
    /// `luma_weight_lX_flag`.
    pub luma_flag: bool,
    /// Luma weight (the default `1 << denom` if the flag is 0).
    pub luma_weight: i32,
    /// Luma offset.
    pub luma_offset: i32,
    /// `chroma_weight_lX_flag`.
    pub chroma_flag: bool,
    /// Cb and Cr weights.
    pub chroma_weight: [i32; 2],
    /// Cb and Cr offsets.
    pub chroma_offset: [i32; 2],
}

/// `pred_weight_table()`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PredWeightTable {
    /// `luma_log2_weight_denom`.
    pub luma_log2_denom: u32,
    /// `chroma_log2_weight_denom`.
    pub chroma_log2_denom: u32,
    /// Entries for list 0 then list 1, one per active reference index.
    pub entries: [Vec<WeightEntry>; 2],
}

/// A parsed slice header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceHeader {
    /// `nal_ref_idc`.
    pub nal_ref_idc: u8,
    /// True for NAL type 5.
    pub idr: bool,
    /// `first_mb_in_slice`.
    pub first_mb_in_slice: u32,
    /// Slice type.
    pub slice_type: SliceType,
    /// `slice_type >= 5`: every slice of the picture has this type.
    pub slice_type_all: bool,
    /// `pic_parameter_set_id`.
    pub pps_id: u32,
    /// `colour_plane_id`.
    pub colour_plane_id: u32,
    /// `frame_num`.
    pub frame_num: u32,
    /// `field_pic_flag`.
    pub field_pic: bool,
    /// `bottom_field_flag`.
    pub bottom_field: bool,
    /// `idr_pic_id`.
    pub idr_pic_id: u32,
    /// `pic_order_cnt_lsb`.
    pub pic_order_cnt_lsb: u32,
    /// `delta_pic_order_cnt_bottom`.
    pub delta_pic_order_cnt_bottom: i32,
    /// `delta_pic_order_cnt[0..2]`.
    pub delta_pic_order_cnt: [i32; 2],
    /// `redundant_pic_cnt`.
    pub redundant_pic_cnt: u32,
    /// `direct_spatial_mv_pred_flag`.
    pub direct_spatial_mv_pred: bool,
    /// `num_ref_idx_active_override_flag`.
    pub num_ref_idx_override: bool,
    /// Effective `num_ref_idx_l0_active_minus1 + 1` (0 for I slices).
    pub num_ref_idx_l0_active: u32,
    /// Effective `num_ref_idx_l1_active_minus1 + 1` (0 unless B).
    pub num_ref_idx_l1_active: u32,
    /// `ref_pic_list_modification()` operations for list 0 and 1 (`None`: flag is 0).
    pub ref_list_mod: [Option<Vec<RefListMod>>; 2],
    /// `pred_weight_table()` if present.
    pub pred_weight_table: Option<PredWeightTable>,
    /// `dec_ref_pic_marking()` (present iff `nal_ref_idc != 0`).
    pub dec_ref_pic_marking: Option<DecRefPicMarking>,
    /// `cabac_init_idc`.
    pub cabac_init_idc: u32,
    /// `slice_qp_delta`.
    pub slice_qp_delta: i32,
    /// `sp_for_switch_flag`.
    pub sp_for_switch: bool,
    /// `slice_qs_delta`.
    pub slice_qs_delta: i32,
    /// `disable_deblocking_filter_idc`.
    pub disable_deblocking_filter_idc: u32,
    /// `slice_alpha_c0_offset_div2`.
    pub slice_alpha_c0_offset_div2: i32,
    /// `slice_beta_offset_div2`.
    pub slice_beta_offset_div2: i32,
    /// Bit position in the RBSP where `slice_data()` begins.
    pub data_bit_pos: usize,
}

impl SliceHeader {
    /// A header with every field at its default, for an encoder to fill in.
    pub fn new(slice_type: SliceType, idr: bool) -> Self {
        Self {
            nal_ref_idc: 1,
            idr,
            first_mb_in_slice: 0,
            slice_type,
            slice_type_all: false,
            pps_id: 0,
            colour_plane_id: 0,
            frame_num: 0,
            field_pic: false,
            bottom_field: false,
            idr_pic_id: 0,
            pic_order_cnt_lsb: 0,
            delta_pic_order_cnt_bottom: 0,
            delta_pic_order_cnt: [0; 2],
            redundant_pic_cnt: 0,
            direct_spatial_mv_pred: true,
            num_ref_idx_override: false,
            num_ref_idx_l0_active: 0,
            num_ref_idx_l1_active: 0,
            ref_list_mod: [None, None],
            pred_weight_table: None,
            dec_ref_pic_marking: None,
            cabac_init_idc: 0,
            slice_qp_delta: 0,
            sp_for_switch: false,
            slice_qs_delta: 0,
            disable_deblocking_filter_idc: 0,
            slice_alpha_c0_offset_div2: 0,
            slice_beta_offset_div2: 0,
            data_bit_pos: 0,
        }
    }

    /// Parse a slice header from the RBSP of a slice NAL unit (after the NAL header byte). Returns the header
    /// and the parameter sets it refers to.
    pub fn parse(rbsp: &[u8], nal: NalHeader, sets: &ParamSets) -> Result<(Self, Arc<Sps>, Arc<Pps>)> {
        let mut r = BitReader::new(rbsp);
        let idr = nal.unit_type == NalUnitType::IdrSlice;
        let first_mb_in_slice = r.read_ue()?;
        let st = r.read_ue()?;
        if st > 9 {
            return Err(Error::Invalid("slice_type out of range"));
        }
        let pps_id = r.read_ue()?;
        let pps = sets.pps(pps_id).ok_or(Error::Invalid("slice refers to a missing PPS"))?.clone();
        let sps = sets.sps(pps.sps_id).ok_or(Error::Invalid("slice refers to a missing SPS"))?.clone();
        let slice_type = SliceType::from_u32(st);
        let mut h = Self::new(slice_type, idr);
        h.nal_ref_idc = nal.ref_idc;
        h.first_mb_in_slice = first_mb_in_slice;
        h.slice_type_all = st >= 5;
        h.pps_id = pps_id;
        if sps.separate_colour_plane {
            h.colour_plane_id = r.read_bits(2)?;
        }
        h.frame_num = r.read_bits(sps.log2_max_frame_num)?;
        if !sps.frame_mbs_only {
            h.field_pic = r.read_flag()?;
            if h.field_pic {
                h.bottom_field = r.read_flag()?;
            }
        }
        if idr {
            h.idr_pic_id = r.read_ue()?;
        }
        if sps.poc_type == 0 {
            h.pic_order_cnt_lsb = r.read_bits(sps.log2_max_poc_lsb)?;
            if pps.bottom_field_pic_order_in_frame_present && !h.field_pic {
                h.delta_pic_order_cnt_bottom = r.read_se()?;
            }
        }
        if sps.poc_type == 1 && !sps.delta_pic_order_always_zero {
            h.delta_pic_order_cnt[0] = r.read_se()?;
            if pps.bottom_field_pic_order_in_frame_present && !h.field_pic {
                h.delta_pic_order_cnt[1] = r.read_se()?;
            }
        }
        if pps.redundant_pic_cnt_present {
            h.redundant_pic_cnt = r.read_ue()?;
        }
        if slice_type == SliceType::B {
            h.direct_spatial_mv_pred = r.read_flag()?;
        }
        let is_p = matches!(slice_type, SliceType::P | SliceType::Sp);
        let is_b = slice_type == SliceType::B;
        if is_p || is_b {
            h.num_ref_idx_l0_active = pps.num_ref_idx_l0_default;
            h.num_ref_idx_l1_active = if is_b { pps.num_ref_idx_l1_default } else { 0 };
            h.num_ref_idx_override = r.read_flag()?;
            if h.num_ref_idx_override {
                h.num_ref_idx_l0_active = r.read_ue()?.saturating_add(1);
                if is_b {
                    h.num_ref_idx_l1_active = r.read_ue()?.saturating_add(1);
                }
            }
            let max = if h.field_pic { 32 } else { 16 };
            if h.num_ref_idx_l0_active > max || h.num_ref_idx_l1_active > max {
                return Err(Error::Invalid("num_ref_idx_active out of range"));
            }
        }
        // ref_pic_list_modification()
        if !slice_type.is_intra() {
            for list in 0..(1 + is_b as usize) {
                if r.read_flag()? {
                    let mut ops = Vec::new();
                    loop {
                        let idc = r.read_ue()?;
                        match idc {
                            0..=2 => ops.push(RefListMod { idc, value: r.read_ue()? }),
                            3 => break,
                            _ => return Err(Error::Invalid("modification_of_pic_nums_idc out of range")),
                        }
                        if ops.len() > 66 {
                            return Err(Error::Invalid("too many reference list modifications"));
                        }
                    }
                    h.ref_list_mod[list] = Some(ops);
                }
            }
        }
        if (pps.weighted_pred && is_p) || (pps.weighted_bipred_idc == 1 && is_b) {
            h.pred_weight_table = Some(parse_pred_weight_table(&mut r, &sps, &h)?);
        }
        if nal.ref_idc != 0 {
            let mut m = DecRefPicMarking::default();
            if idr {
                m.no_output_of_prior_pics = r.read_flag()?;
                m.long_term_reference = r.read_flag()?;
            } else {
                m.adaptive = r.read_flag()?;
                if m.adaptive {
                    loop {
                        let op = r.read_ue()?;
                        let mmco = match op {
                            0 => break,
                            1 => Mmco::ShortTermUnused { difference_of_pic_nums_minus1: r.read_ue()? },
                            2 => Mmco::LongTermUnused { long_term_pic_num: r.read_ue()? },
                            3 => Mmco::AssignLongTerm {
                                difference_of_pic_nums_minus1: r.read_ue()?,
                                long_term_frame_idx: r.read_ue()?,
                            },
                            4 => Mmco::MaxLongTermIdx { max_long_term_frame_idx_plus1: r.read_ue()? },
                            5 => Mmco::ClearAll,
                            6 => Mmco::CurrentLongTerm { long_term_frame_idx: r.read_ue()? },
                            _ => {
                                return Err(Error::Invalid(
                                    "memory_management_control_operation out of range",
                                ));
                            }
                        };
                        m.ops.push(mmco);
                        if m.ops.len() > 66 {
                            return Err(Error::Invalid("too many memory management operations"));
                        }
                    }
                }
            }
            h.dec_ref_pic_marking = Some(m);
        }
        if pps.cabac && !slice_type.is_intra() {
            h.cabac_init_idc = r.read_ue()?;
            if h.cabac_init_idc > 2 {
                return Err(Error::Invalid("cabac_init_idc out of range"));
            }
        }
        h.slice_qp_delta = r.read_se()?;
        if matches!(slice_type, SliceType::Sp | SliceType::Si) {
            if slice_type == SliceType::Sp {
                h.sp_for_switch = r.read_flag()?;
            }
            h.slice_qs_delta = r.read_se()?;
        }
        if pps.deblocking_filter_control_present {
            h.disable_deblocking_filter_idc = r.read_ue()?;
            if h.disable_deblocking_filter_idc > 2 {
                return Err(Error::Invalid("disable_deblocking_filter_idc out of range"));
            }
            if h.disable_deblocking_filter_idc != 1 {
                h.slice_alpha_c0_offset_div2 = r.read_se()?;
                h.slice_beta_offset_div2 = r.read_se()?;
                if !(-6..=6).contains(&h.slice_alpha_c0_offset_div2)
                    || !(-6..=6).contains(&h.slice_beta_offset_div2)
                {
                    return Err(Error::Invalid("deblocking offset out of range"));
                }
            }
        }
        if pps.num_slice_groups > 1 {
            return Err(Error::Unsupported("slice groups (FMO)"));
        }
        h.data_bit_pos = r.pos();
        if r.overrun() {
            return Err(Error::Truncated);
        }
        Ok((h, sps, pps))
    }

    /// Write the header. The caller supplies the parameter sets it was built against; `slice_data()` follows.
    pub fn write(&self, sps: &Sps, pps: &Pps, w: &mut BitWriter) {
        w.put_ue(self.first_mb_in_slice);
        w.put_ue(self.slice_type.to_u32() + if self.slice_type_all { 5 } else { 0 });
        w.put_ue(self.pps_id);
        if sps.separate_colour_plane {
            w.put_bits(self.colour_plane_id, 2);
        }
        w.put_bits(self.frame_num, sps.log2_max_frame_num);
        if !sps.frame_mbs_only {
            w.put_bit(self.field_pic);
            if self.field_pic {
                w.put_bit(self.bottom_field);
            }
        }
        if self.idr {
            w.put_ue(self.idr_pic_id);
        }
        if sps.poc_type == 0 {
            w.put_bits(self.pic_order_cnt_lsb, sps.log2_max_poc_lsb);
            if pps.bottom_field_pic_order_in_frame_present && !self.field_pic {
                w.put_se(self.delta_pic_order_cnt_bottom);
            }
        }
        if sps.poc_type == 1 && !sps.delta_pic_order_always_zero {
            w.put_se(self.delta_pic_order_cnt[0]);
            if pps.bottom_field_pic_order_in_frame_present && !self.field_pic {
                w.put_se(self.delta_pic_order_cnt[1]);
            }
        }
        if pps.redundant_pic_cnt_present {
            w.put_ue(self.redundant_pic_cnt);
        }
        let is_p = matches!(self.slice_type, SliceType::P | SliceType::Sp);
        let is_b = self.slice_type == SliceType::B;
        if is_b {
            w.put_bit(self.direct_spatial_mv_pred);
        }
        if is_p || is_b {
            w.put_bit(self.num_ref_idx_override);
            if self.num_ref_idx_override {
                w.put_ue(self.num_ref_idx_l0_active - 1);
                if is_b {
                    w.put_ue(self.num_ref_idx_l1_active - 1);
                }
            }
        }
        if !self.slice_type.is_intra() {
            for list in 0..(1 + is_b as usize) {
                match &self.ref_list_mod[list] {
                    Some(ops) => {
                        w.put_bit(true);
                        for op in ops {
                            w.put_ue(op.idc);
                            w.put_ue(op.value);
                        }
                        w.put_ue(3);
                    }
                    None => w.put_bit(false),
                }
            }
        }
        if let Some(t) = &self.pred_weight_table {
            w.put_ue(t.luma_log2_denom);
            w.put_ue(t.chroma_log2_denom);
            for list in 0..(1 + is_b as usize) {
                for e in &t.entries[list] {
                    w.put_bit(e.luma_flag);
                    if e.luma_flag {
                        w.put_se(e.luma_weight);
                        w.put_se(e.luma_offset);
                    }
                    w.put_bit(e.chroma_flag);
                    if e.chroma_flag {
                        for j in 0..2 {
                            w.put_se(e.chroma_weight[j]);
                            w.put_se(e.chroma_offset[j]);
                        }
                    }
                }
            }
        }
        if let Some(m) = &self.dec_ref_pic_marking {
            if self.idr {
                w.put_bit(m.no_output_of_prior_pics);
                w.put_bit(m.long_term_reference);
            } else {
                w.put_bit(m.adaptive);
                if m.adaptive {
                    for op in &m.ops {
                        match *op {
                            Mmco::ShortTermUnused { difference_of_pic_nums_minus1 } => {
                                w.put_ue(1);
                                w.put_ue(difference_of_pic_nums_minus1);
                            }
                            Mmco::LongTermUnused { long_term_pic_num } => {
                                w.put_ue(2);
                                w.put_ue(long_term_pic_num);
                            }
                            Mmco::AssignLongTerm { difference_of_pic_nums_minus1, long_term_frame_idx } => {
                                w.put_ue(3);
                                w.put_ue(difference_of_pic_nums_minus1);
                                w.put_ue(long_term_frame_idx);
                            }
                            Mmco::MaxLongTermIdx { max_long_term_frame_idx_plus1 } => {
                                w.put_ue(4);
                                w.put_ue(max_long_term_frame_idx_plus1);
                            }
                            Mmco::ClearAll => w.put_ue(5),
                            Mmco::CurrentLongTerm { long_term_frame_idx } => {
                                w.put_ue(6);
                                w.put_ue(long_term_frame_idx);
                            }
                        }
                    }
                    w.put_ue(0);
                }
            }
        }
        if pps.cabac && !self.slice_type.is_intra() {
            w.put_ue(self.cabac_init_idc);
        }
        w.put_se(self.slice_qp_delta);
        if matches!(self.slice_type, SliceType::Sp | SliceType::Si) {
            if self.slice_type == SliceType::Sp {
                w.put_bit(self.sp_for_switch);
            }
            w.put_se(self.slice_qs_delta);
        }
        if pps.deblocking_filter_control_present {
            w.put_ue(self.disable_deblocking_filter_idc);
            if self.disable_deblocking_filter_idc != 1 {
                w.put_se(self.slice_alpha_c0_offset_div2);
                w.put_se(self.slice_beta_offset_div2);
            }
        }
    }

    /// `SliceQPY` = 26 + pic_init_qp_minus26 + slice_qp_delta.
    pub fn slice_qp(&self, pps: &Pps) -> i32 {
        pps.pic_init_qp + self.slice_qp_delta
    }
}

fn parse_pred_weight_table(r: &mut BitReader, sps: &Sps, h: &SliceHeader) -> Result<PredWeightTable> {
    let mut t = PredWeightTable { luma_log2_denom: r.read_ue()?, ..Default::default() };
    let chroma = sps.chroma_array_type() != 0;
    if chroma {
        t.chroma_log2_denom = r.read_ue()?;
    }
    if t.luma_log2_denom > 7 || t.chroma_log2_denom > 7 {
        return Err(Error::Invalid("log2_weight_denom out of range"));
    }
    let lists = if h.slice_type == SliceType::B { 2 } else { 1 };
    for list in 0..lists {
        let n = if list == 0 { h.num_ref_idx_l0_active } else { h.num_ref_idx_l1_active };
        for _ in 0..n {
            let mut e = WeightEntry {
                luma_flag: false,
                luma_weight: 1 << t.luma_log2_denom,
                luma_offset: 0,
                chroma_flag: false,
                chroma_weight: [1 << t.chroma_log2_denom; 2],
                chroma_offset: [0; 2],
            };
            e.luma_flag = r.read_flag()?;
            if e.luma_flag {
                e.luma_weight = r.read_se()?;
                e.luma_offset = r.read_se()?;
                if !(-128..=127).contains(&e.luma_weight) || !(-128..=127).contains(&e.luma_offset) {
                    return Err(Error::Invalid("luma weight out of range"));
                }
            }
            if chroma {
                e.chroma_flag = r.read_flag()?;
                if e.chroma_flag {
                    for j in 0..2 {
                        e.chroma_weight[j] = r.read_se()?;
                        e.chroma_offset[j] = r.read_se()?;
                        if !(-128..=127).contains(&e.chroma_weight[j])
                            || !(-128..=127).contains(&e.chroma_offset[j])
                        {
                            return Err(Error::Invalid("chroma weight out of range"));
                        }
                    }
                }
            }
            t.entries[list].push(e);
        }
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sets() -> ParamSets {
        let sps = Sps::parse(&{
            let mut s = crate::params::sps::tests_support::sample();
            s.log2_max_frame_num = 6;
            s.poc_type = 0;
            s.log2_max_poc_lsb = 6;
            s.write()
        })
        .unwrap();
        let mut pps = Pps::new(0, sps.id);
        pps.cabac = true;
        pps.weighted_pred = true;
        pps.weighted_bipred_idc = 1;
        pps.deblocking_filter_control_present = true;
        pps.num_ref_idx_l0_default = 2;
        pps.num_ref_idx_l1_default = 2;
        let mut ps = ParamSets::new();
        ps.insert_sps(sps);
        ps.insert_pps(Pps::parse(&pps.write(), &ParamSets::new()).unwrap());
        ps
    }

    #[test]
    fn roundtrip_b_slice_with_everything() {
        let ps = sets();
        let sps = ps.sps(0).unwrap().clone();
        let pps = ps.pps(0).unwrap().clone();
        let mut h = SliceHeader::new(SliceType::B, false);
        h.nal_ref_idc = 2;
        h.first_mb_in_slice = 7;
        h.frame_num = 33;
        h.pic_order_cnt_lsb = 40;
        h.num_ref_idx_override = true;
        h.num_ref_idx_l0_active = 3;
        h.num_ref_idx_l1_active = 1;
        h.ref_list_mod =
            [Some(alloc::vec![RefListMod { idc: 0, value: 1 }, RefListMod { idc: 2, value: 0 }]), None];
        let e = |lw| WeightEntry {
            luma_flag: true,
            luma_weight: lw,
            luma_offset: -3,
            chroma_flag: true,
            chroma_weight: [5, 6],
            chroma_offset: [1, -1],
        };
        h.pred_weight_table = Some(PredWeightTable {
            luma_log2_denom: 5,
            chroma_log2_denom: 4,
            entries: [alloc::vec![e(30), e(31), e(32)], alloc::vec![e(10)]],
        });
        h.dec_ref_pic_marking = Some(DecRefPicMarking {
            adaptive: true,
            ops: alloc::vec![
                Mmco::ShortTermUnused { difference_of_pic_nums_minus1: 2 },
                Mmco::CurrentLongTerm { long_term_frame_idx: 1 }
            ],
            ..Default::default()
        });
        h.cabac_init_idc = 2;
        h.slice_qp_delta = -4;
        h.disable_deblocking_filter_idc = 0;
        h.slice_alpha_c0_offset_div2 = 3;
        h.slice_beta_offset_div2 = -2;
        let mut w = BitWriter::new();
        h.write(&sps, &pps, &mut w);
        let bits = w.bit_len();
        w.put_trailing_bits();
        let bytes = w.into_bytes();
        let nal = NalHeader { ref_idc: 2, unit_type: NalUnitType::Slice };
        let (mut got, _, _) = SliceHeader::parse(&bytes, nal, &ps).unwrap();
        assert_eq!(got.data_bit_pos, bits);
        got.data_bit_pos = 0;
        assert_eq!(got, h);
        for n in 0..bytes.len() {
            let _ = SliceHeader::parse(&bytes[..n], nal, &ps);
        }
    }
}
