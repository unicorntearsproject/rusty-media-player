//! Slice segment headers (7.3.6).
use crate::bits::{BitReader, ceil_log2};
use crate::nal::{self, NalHeader};
use crate::ps::{Pps, ShortTermRps, Sps, parse_short_term_rps, slice_address_bits};
use crate::{Error, Result};
use alloc::vec::Vec;

/// `slice_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceType {
    /// B slice.
    B = 0,
    /// P slice.
    P = 1,
    /// I slice.
    I = 2,
}

/// One long-term reference picture of a slice header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LongTerm {
    /// `PocLsbLt`.
    pub poc_lsb: u32,
    /// `UsedByCurrPicLt`.
    pub used_by_curr: bool,
    /// `delta_poc_msb_present_flag`.
    pub msb_present: bool,
    /// `DeltaPocMsbCycleLt` (accumulated as in 7-52).
    pub delta_poc_msb_cycle: u32,
}

/// The prediction weights of one list entry, as derived (7.4.7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Weight {
    /// `LumaWeightL0/L1`.
    pub luma_weight: i32,
    /// `luma_offset_l0/l1`.
    pub luma_offset: i32,
    /// `ChromaWeightL0/L1` for Cb and Cr.
    pub chroma_weight: [i32; 2],
    /// `ChromaOffsetL0/L1` for Cb and Cr.
    pub chroma_offset: [i32; 2],
}

/// `pred_weight_table()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredWeights {
    /// `luma_log2_weight_denom`.
    pub luma_log2_denom: u8,
    /// `ChromaLog2WeightDenom`.
    pub chroma_log2_denom: u8,
    /// The weights of list 0 and list 1 by reference index.
    pub weights: [Vec<Weight>; 2],
}

/// A slice segment header.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceHeader {
    /// The unit's header.
    pub nal: NalHeader,
    /// `first_slice_segment_in_pic_flag`.
    pub first_slice_segment_in_pic: bool,
    /// `no_output_of_prior_pics_flag`.
    pub no_output_of_prior_pics: bool,
    /// `slice_pic_parameter_set_id`.
    pub pps_id: u8,
    /// `dependent_slice_segment_flag`.
    pub dependent: bool,
    /// `slice_segment_address`.
    pub segment_address: u32,
    /// `slice_type`.
    pub slice_type: SliceType,
    /// `pic_output_flag`.
    pub pic_output_flag: bool,
    /// `colour_plane_id`.
    pub colour_plane_id: u8,
    /// `slice_pic_order_cnt_lsb` (0 for IDR).
    pub poc_lsb: u32,
    /// The short-term RPS in force: the SPS's chosen one, or the one the header carries (empty for IDR).
    pub st_rps: ShortTermRps,
    /// `short_term_ref_pic_set_sps_flag`.
    pub st_rps_from_sps: bool,
    /// `short_term_ref_pic_set_idx`.
    pub st_rps_idx: u32,
    /// The bits of `st_ref_pic_set()` in the header (VA-API wants this when the set is coded in the slice).
    pub st_rps_bits: u32,
    /// The long-term pictures.
    pub long_term: Vec<LongTerm>,
    /// `slice_temporal_mvp_enabled_flag`.
    pub temporal_mvp: bool,
    /// `slice_sao_luma_flag`.
    pub sao_luma: bool,
    /// `slice_sao_chroma_flag`.
    pub sao_chroma: bool,
    /// `num_ref_idx_l0_active_minus1 + 1` and the same for list 1 (0 for lists a slice type does not use).
    pub num_ref_idx: [u8; 2],
    /// `list_entry_l0/l1` when the lists are modified.
    pub list_entry: [Option<Vec<u32>>; 2],
    /// `mvd_l1_zero_flag`.
    pub mvd_l1_zero: bool,
    /// `cabac_init_flag`.
    pub cabac_init: bool,
    /// `collocated_from_l0_flag`.
    pub collocated_from_l0: bool,
    /// `collocated_ref_idx`.
    pub collocated_ref_idx: u8,
    /// `pred_weight_table()`, when the slice has one.
    pub pred_weights: Option<PredWeights>,
    /// `five_minus_max_num_merge_cand`.
    pub five_minus_max_num_merge_cand: u8,
    /// `slice_qp_delta`.
    pub qp_delta: i8,
    /// `slice_cb_qp_offset`.
    pub cb_qp_offset: i8,
    /// `slice_cr_qp_offset`.
    pub cr_qp_offset: i8,
    /// `slice_deblocking_filter_disabled_flag`.
    pub deblocking_disabled: bool,
    /// `slice_beta_offset_div2`.
    pub beta_offset_div2: i8,
    /// `slice_tc_offset_div2`.
    pub tc_offset_div2: i8,
    /// `slice_loop_filter_across_slices_enabled_flag`.
    pub loop_filter_across_slices: bool,
    /// `entry_point_offset_minus1 + 1` for each entry point.
    pub entry_points: Vec<u32>,
    /// Where the slice data starts in the unit: the byte offset in the escaped bytes of the unit (header included).
    pub data_offset: usize,
    /// Where the slice data starts in the unit with the emulation prevention bytes taken out.
    pub data_offset_rbsp: usize,
    /// How many emulation prevention bytes the header (up to `data_offset`) has.
    pub header_emulation_bytes: usize,
}

impl SliceHeader {
    /// `NumPicTotalCurr`.
    pub fn num_pic_total_curr(&self) -> usize {
        self.st_rps.used_count() + self.long_term.iter().filter(|l| l.used_by_curr).count()
    }

    /// Whether this is an intra picture's slice.
    pub fn is_intra(&self) -> bool {
        self.slice_type == SliceType::I
    }

    /// Parse the header of the NAL unit `unit` (including its two header bytes). `pps` and `sps` look a set up by id; `prev` is the
    /// header of the independent slice segment before this one (a dependent segment copies its fields).
    pub fn parse(
        unit: &[u8],
        pps_of: &dyn Fn(u8) -> Option<(Pps, Sps)>,
        prev: Option<&SliceHeader>,
    ) -> Result<Self> {
        let nalh = NalHeader::parse(unit).ok_or(Error::Invalid("NAL header"))?;
        if !nalh.is_slice() {
            return Err(Error::Invalid("not a slice NAL unit"));
        }
        let mut rbsp = Vec::with_capacity(unit.len());
        let mut removed = Vec::new();
        nal::unescape(unit, &mut rbsp, &mut removed);
        let mut r = BitReader::new(&rbsp[2..]);
        let first = r.flag()?;
        let no_output_of_prior_pics = if nalh.is_irap() { r.flag()? } else { false };
        let pps_id = r.ue()?;
        if pps_id > 63 {
            return Err(Error::Invalid("slice_pic_parameter_set_id"));
        }
        let (pps, sps) =
            pps_of(pps_id as u8).ok_or(Error::Invalid("slice refers to a parameter set that is missing"))?;
        let mut dependent = false;
        let mut segment_address = 0;
        if !first {
            if pps.dependent_slice_segments_enabled {
                dependent = r.flag()?;
            }
            segment_address = r.bits(slice_address_bits(&sps))?;
            if segment_address >= sps.pic_size_in_ctbs() {
                return Err(Error::Invalid("slice_segment_address"));
            }
        }
        let mut h = if dependent {
            let p =
                prev.ok_or(Error::Invalid("a dependent slice segment without an independent one before it"))?;
            let mut h = p.clone();
            h.nal = nalh;
            h.first_slice_segment_in_pic = first;
            h.no_output_of_prior_pics = no_output_of_prior_pics;
            h.dependent = true;
            h.segment_address = segment_address;
            h.entry_points = Vec::new();
            h
        } else {
            Self::parse_independent(
                &mut r,
                nalh,
                first,
                no_output_of_prior_pics,
                pps_id as u8,
                segment_address,
                &pps,
                &sps,
            )?
        };
        if pps.tiles_enabled || pps.entropy_coding_sync_enabled {
            let n = r.ue()?;
            if n > 4096 {
                return Err(Error::Invalid("num_entry_point_offsets"));
            }
            if n > 0 {
                let len = r.ue()? + 1;
                if len > 32 {
                    return Err(Error::Invalid("offset_len_minus1"));
                }
                for _ in 0..n {
                    h.entry_points.push(r.bits(len)? + 1);
                }
            }
        }
        if pps.slice_segment_header_extension_present {
            let n = r.ue()? as usize;
            r.skip(n * 8)?;
        }
        r.byte_alignment()?;
        // The header ends at this byte of the unescaped unit (two header bytes in front); in the escaped bytes it is further on by the
        // emulation prevention bytes before it.
        let end = 2 + r.byte_pos();
        h.data_offset_rbsp = end;
        h.data_offset = nal::escaped_pos(end, &removed);
        h.header_emulation_bytes = removed.iter().filter(|p| **p < h.data_offset).count();
        Ok(h)
    }

    fn parse_independent(
        r: &mut BitReader,
        nalh: NalHeader,
        first: bool,
        no_output_of_prior_pics: bool,
        pps_id: u8,
        segment_address: u32,
        pps: &Pps,
        sps: &Sps,
    ) -> Result<Self> {
        for _ in 0..pps.num_extra_slice_header_bits {
            r.flag()?;
        }
        let slice_type = match r.ue()? {
            0 => SliceType::B,
            1 => SliceType::P,
            2 => SliceType::I,
            _ => return Err(Error::Invalid("slice_type")),
        };
        let pic_output_flag = if pps.output_flag_present { r.flag()? } else { true };
        let mut poc_lsb = 0;
        let mut st_rps = ShortTermRps::default();
        let (mut st_rps_from_sps, mut st_rps_idx, mut st_rps_bits) = (false, 0, 0);
        let mut long_term: Vec<LongTerm> = Vec::new();
        let mut temporal_mvp = false;
        if !nalh.is_idr() {
            poc_lsb = r.bits(sps.max_poc_lsb.trailing_zeros())?;
            st_rps_from_sps = r.flag()?;
            if !st_rps_from_sps {
                let before = r.pos();
                st_rps = parse_short_term_rps(r, sps.st_rps.len(), sps.st_rps.len(), &sps.st_rps)?;
                st_rps_bits = (r.pos() - before) as u32;
            } else {
                if sps.st_rps.is_empty() {
                    return Err(Error::Invalid("slice uses an SPS short-term RPS but the SPS has none"));
                }
                if sps.st_rps.len() > 1 {
                    st_rps_idx = r.bits(ceil_log2(sps.st_rps.len() as u32))?;
                }
                st_rps = sps
                    .st_rps
                    .get(st_rps_idx as usize)
                    .ok_or(Error::Invalid("short_term_ref_pic_set_idx"))?
                    .clone();
            }
            if sps.long_term_ref_pics_present {
                let n_sps = if !sps.lt_ref_pics.is_empty() { r.ue()? as usize } else { 0 };
                let n_pics = r.ue()? as usize;
                if n_sps + n_pics > 32 || n_sps > sps.lt_ref_pics.len() {
                    return Err(Error::Invalid("long-term picture counts"));
                }
                for i in 0..n_sps + n_pics {
                    let (lsb, used) = if i < n_sps {
                        let idx = if sps.lt_ref_pics.len() > 1 {
                            r.bits(ceil_log2(sps.lt_ref_pics.len() as u32))? as usize
                        } else {
                            0
                        };
                        *sps.lt_ref_pics.get(idx).ok_or(Error::Invalid("lt_idx_sps"))?
                    } else {
                        let lsb = r.bits(sps.max_poc_lsb.trailing_zeros())?;
                        (lsb, r.flag()?)
                    };
                    let msb_present = r.flag()?;
                    let mut cycle = if msb_present { r.ue()? } else { 0 };
                    // (7-52): the cycles accumulate, except at the first entry and at the first of those coded in the header.
                    if i != 0 && i != n_sps {
                        cycle += long_term[i - 1].delta_poc_msb_cycle;
                    }
                    long_term.push(LongTerm {
                        poc_lsb: lsb,
                        used_by_curr: used,
                        msb_present,
                        delta_poc_msb_cycle: cycle,
                    });
                }
            }
            if sps.temporal_mvp_enabled {
                temporal_mvp = r.flag()?;
            }
        }
        let (mut sao_luma, mut sao_chroma) = (false, false);
        if sps.sao_enabled {
            sao_luma = r.flag()?;
            sao_chroma = r.flag()?;
        }
        let mut h = Self {
            nal: nalh,
            first_slice_segment_in_pic: first,
            no_output_of_prior_pics,
            pps_id,
            dependent: false,
            segment_address,
            slice_type,
            pic_output_flag,
            colour_plane_id: 0,
            poc_lsb,
            st_rps,
            st_rps_from_sps,
            st_rps_idx,
            st_rps_bits,
            long_term,
            temporal_mvp,
            sao_luma,
            sao_chroma,
            num_ref_idx: [0, 0],
            list_entry: [None, None],
            mvd_l1_zero: false,
            cabac_init: false,
            collocated_from_l0: true,
            collocated_ref_idx: 0,
            pred_weights: None,
            five_minus_max_num_merge_cand: 0,
            qp_delta: 0,
            cb_qp_offset: 0,
            cr_qp_offset: 0,
            deblocking_disabled: pps.deblocking_filter_disabled,
            beta_offset_div2: pps.beta_offset_div2,
            tc_offset_div2: pps.tc_offset_div2,
            loop_filter_across_slices: pps.loop_filter_across_slices_enabled,
            entry_points: Vec::new(),
            data_offset: 0,
            data_offset_rbsp: 0,
            header_emulation_bytes: 0,
        };
        if slice_type != SliceType::I {
            h.num_ref_idx = [
                pps.num_ref_idx_l0_default_active_minus1 + 1,
                if slice_type == SliceType::B { pps.num_ref_idx_l1_default_active_minus1 + 1 } else { 0 },
            ];
            if r.flag()? {
                h.num_ref_idx[0] = r.ue()? as u8 + 1;
                if slice_type == SliceType::B {
                    h.num_ref_idx[1] = r.ue()? as u8 + 1;
                }
            }
            if h.num_ref_idx[0] > 15 || h.num_ref_idx[1] > 15 {
                return Err(Error::Invalid("num_ref_idx_active"));
            }
            let total = h.num_pic_total_curr();
            if total == 0 {
                return Err(Error::Invalid("a P or B slice with no reference pictures"));
            }
            if pps.lists_modification_present && total > 1 {
                let bits = ceil_log2(total as u32);
                for l in 0..if slice_type == SliceType::B { 2 } else { 1 } {
                    if r.flag()? {
                        let mut v = Vec::new();
                        for _ in 0..h.num_ref_idx[l] {
                            let e = r.bits(bits)?;
                            if e as usize >= total {
                                return Err(Error::Invalid("list_entry"));
                            }
                            v.push(e);
                        }
                        h.list_entry[l] = Some(v);
                    }
                }
            }
            if slice_type == SliceType::B {
                h.mvd_l1_zero = r.flag()?;
            }
            if pps.cabac_init_present {
                h.cabac_init = r.flag()?;
            }
            if h.temporal_mvp {
                if slice_type == SliceType::B {
                    h.collocated_from_l0 = r.flag()?;
                }
                let list = if h.collocated_from_l0 { 0 } else { 1 };
                if h.num_ref_idx[list] > 1 {
                    h.collocated_ref_idx = r.ue()? as u8;
                    if h.collocated_ref_idx >= h.num_ref_idx[list] {
                        return Err(Error::Invalid("collocated_ref_idx"));
                    }
                }
            }
            if (pps.weighted_pred && slice_type == SliceType::P)
                || (pps.weighted_bipred && slice_type == SliceType::B)
            {
                h.pred_weights = Some(parse_pred_weights(r, &h)?);
            }
            let m = r.ue()?;
            if m > 4 {
                return Err(Error::Invalid("five_minus_max_num_merge_cand"));
            }
            h.five_minus_max_num_merge_cand = m as u8;
        }
        h.qp_delta = r.se()? as i8;
        if pps.slice_chroma_qp_offsets_present {
            h.cb_qp_offset = r.se()? as i8;
            h.cr_qp_offset = r.se()? as i8;
        }
        let override_flag = if pps.deblocking_filter_override_enabled { r.flag()? } else { false };
        if override_flag {
            h.deblocking_disabled = r.flag()?;
            if !h.deblocking_disabled {
                h.beta_offset_div2 = r.se()? as i8;
                h.tc_offset_div2 = r.se()? as i8;
            }
        }
        if pps.loop_filter_across_slices_enabled && (h.sao_luma || h.sao_chroma || !h.deblocking_disabled) {
            h.loop_filter_across_slices = r.flag()?;
        }
        Ok(h)
    }
}

/// `pred_weight_table()` for 4:2:0 (a chroma array is always present).
fn parse_pred_weights(r: &mut BitReader, h: &SliceHeader) -> Result<PredWeights> {
    let luma_log2 = r.ue()?;
    if luma_log2 > 7 {
        return Err(Error::Invalid("luma_log2_weight_denom"));
    }
    let chroma_log2 = luma_log2 as i32 + r.se()?;
    if !(0..=7).contains(&chroma_log2) {
        return Err(Error::Invalid("delta_chroma_log2_weight_denom"));
    }
    let lists = if h.slice_type == SliceType::B { 2 } else { 1 };
    let mut weights: [Vec<Weight>; 2] = [Vec::new(), Vec::new()];
    for l in 0..lists {
        let n = h.num_ref_idx[l] as usize;
        let luma_flags: Vec<bool> = (0..n).map(|_| r.flag()).collect::<Result<_>>()?;
        let chroma_flags: Vec<bool> = (0..n).map(|_| r.flag()).collect::<Result<_>>()?;
        for i in 0..n {
            let mut w = Weight {
                luma_weight: 1 << luma_log2,
                luma_offset: 0,
                chroma_weight: [1 << chroma_log2; 2],
                chroma_offset: [0; 2],
            };
            if luma_flags[i] {
                w.luma_weight += r.se()?;
                w.luma_offset = r.se()?;
            }
            if chroma_flags[i] {
                for j in 0..2 {
                    let dw = r.se()?;
                    let doff = r.se()?;
                    w.chroma_weight[j] = (1 << chroma_log2) + dw;
                    // (7-56) with wpOffsetHalfRangeC = 128.
                    w.chroma_offset[j] =
                        (128 + doff - ((128 * w.chroma_weight[j]) >> chroma_log2)).clamp(-128, 127);
                }
            }
            weights[l].push(w);
        }
    }
    Ok(PredWeights { luma_log2_denom: luma_log2 as u8, chroma_log2_denom: chroma_log2 as u8, weights })
}
