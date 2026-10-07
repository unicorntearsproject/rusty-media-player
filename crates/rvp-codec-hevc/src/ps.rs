//! Parameter sets: the sequence parameter set (SPS) and picture parameter set (PPS) of Main and Main 10 streams. The video parameter
//! set carries nothing a decoder needs and is skipped.
use crate::bits::{BitReader, ceil_log2};
use crate::{Error, Result};
use alloc::vec::Vec;

/// One short-term reference picture set (7.3.7 and 7.4.8), already expanded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShortTermRps {
    /// `DeltaPocS0`: POC differences of the pictures before the current one, nearest first (negative).
    pub delta_poc_s0: Vec<i32>,
    /// `UsedByCurrPicS0`.
    pub used_s0: Vec<bool>,
    /// `DeltaPocS1`: POC differences of the pictures after the current one, nearest first (positive).
    pub delta_poc_s1: Vec<i32>,
    /// `UsedByCurrPicS1`.
    pub used_s1: Vec<bool>,
}

impl ShortTermRps {
    /// `NumDeltaPocs`.
    pub fn num_delta_pocs(&self) -> usize {
        self.delta_poc_s0.len() + self.delta_poc_s1.len()
    }

    /// How many of its pictures the current picture uses.
    pub fn used_count(&self) -> usize {
        self.used_s0.iter().chain(&self.used_s1).filter(|u| **u).count()
    }
}

/// Parse `st_ref_pic_set(idx)`; `sets` are the ones of the SPS parsed so far (`idx` of them), `in_slice` is true for the one at
/// `idx == num_short_term_ref_pic_sets` that a slice header carries.
pub fn parse_short_term_rps(
    r: &mut BitReader,
    idx: usize,
    num_sets: usize,
    sets: &[ShortTermRps],
) -> Result<ShortTermRps> {
    let inter = if idx != 0 { r.flag()? } else { false };
    let mut out = ShortTermRps::default();
    if inter {
        let delta_idx = if idx == num_sets { r.ue()? as usize + 1 } else { 1 };
        if delta_idx > idx {
            return Err(Error::Invalid("short-term RPS predicts from a set that does not exist"));
        }
        let sign = r.bits(1)?;
        let abs = r.ue()? as i32 + 1;
        let delta_rps = (1 - 2 * sign as i32) * abs;
        let refset = sets.get(idx - delta_idx).ok_or(Error::Invalid("short-term RPS reference"))?;
        let (n_neg, n_all) = (refset.delta_poc_s0.len(), refset.num_delta_pocs());
        let mut used = Vec::with_capacity(n_all + 1);
        let mut use_delta = Vec::with_capacity(n_all + 1);
        for _ in 0..=n_all {
            let u = r.flag()?;
            let d = if u { true } else { r.flag()? };
            used.push(u);
            use_delta.push(d);
        }
        // (7-61)
        for j in (0..refset.delta_poc_s1.len()).rev() {
            let d = refset.delta_poc_s1[j] + delta_rps;
            if d < 0 && use_delta[n_neg + j] {
                out.delta_poc_s0.push(d);
                out.used_s0.push(used[n_neg + j]);
            }
        }
        if delta_rps < 0 && use_delta[n_all] {
            out.delta_poc_s0.push(delta_rps);
            out.used_s0.push(used[n_all]);
        }
        for j in 0..n_neg {
            let d = refset.delta_poc_s0[j] + delta_rps;
            if d < 0 && use_delta[j] {
                out.delta_poc_s0.push(d);
                out.used_s0.push(used[j]);
            }
        }
        // (7-62)
        for j in (0..n_neg).rev() {
            let d = refset.delta_poc_s0[j] + delta_rps;
            if d > 0 && use_delta[j] {
                out.delta_poc_s1.push(d);
                out.used_s1.push(used[j]);
            }
        }
        if delta_rps > 0 && use_delta[n_all] {
            out.delta_poc_s1.push(delta_rps);
            out.used_s1.push(used[n_all]);
        }
        for j in 0..refset.delta_poc_s1.len() {
            let d = refset.delta_poc_s1[j] + delta_rps;
            if d > 0 && use_delta[n_neg + j] {
                out.delta_poc_s1.push(d);
                out.used_s1.push(used[n_neg + j]);
            }
        }
    } else {
        let (neg, pos) = (r.ue()? as usize, r.ue()? as usize);
        if neg > 16 || pos > 16 {
            return Err(Error::Invalid("too many pictures in a short-term RPS"));
        }
        let mut prev = 0;
        for _ in 0..neg {
            prev -= r.ue()? as i32 + 1;
            out.delta_poc_s0.push(prev);
            out.used_s0.push(r.flag()?);
        }
        prev = 0;
        for _ in 0..pos {
            prev += r.ue()? as i32 + 1;
            out.delta_poc_s1.push(prev);
            out.used_s1.push(r.flag()?);
        }
    }
    if out.num_delta_pocs() > 16 {
        return Err(Error::Invalid("too many pictures in a short-term RPS"));
    }
    Ok(out)
}

/// Scaling lists (7.3.4): 4x4 (6 matrices of 16), 8x8 (6 of 64), 16x16 (6 of 64 with a DC), 32x32 (2 of 64 with a DC; the spec's matrixId
/// 0 and 3 are stored at 0 and 1). All in up-right diagonal scan order, as in the bitstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalingList {
    /// 4x4.
    pub l4: [[u8; 16]; 6],
    /// 8x8.
    pub l8: [[u8; 64]; 6],
    /// 16x16.
    pub l16: [[u8; 64]; 6],
    /// 32x32 (matrixId 0 and 3 of the spec).
    pub l32: [[u8; 64]; 2],
    /// DC of the 16x16 lists.
    pub dc16: [u8; 6],
    /// DC of the 32x32 lists.
    pub dc32: [u8; 2],
}

/// Table 7-6, intra, in scan order.
const DEFAULT_INTRA: [u8; 64] = [
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 16, 17, 16, 17, 18, 17, 18, 18, 17, 18, 21, 19, 20, 21, 20,
    19, 21, 24, 22, 22, 24, 24, 22, 22, 24, 25, 25, 27, 30, 27, 25, 25, 29, 31, 35, 35, 31, 29, 36, 41, 44,
    41, 36, 47, 54, 54, 47, 65, 70, 65, 88, 88, 115,
];
/// Table 7-6, inter, in scan order.
const DEFAULT_INTER: [u8; 64] = [
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 17, 18, 18, 18, 18, 18, 18, 20, 20, 20, 20, 20,
    20, 20, 24, 24, 24, 24, 24, 24, 24, 24, 25, 25, 25, 25, 25, 25, 25, 28, 28, 28, 28, 28, 28, 33, 33, 33,
    33, 33, 41, 41, 41, 41, 54, 54, 54, 71, 71, 91,
];

impl ScalingList {
    /// The default lists (Table 7-5 and 7-6): flat 4x4, the intra and inter tables for the others.
    pub fn default_lists() -> Self {
        let pick = |m: usize| if m < 3 { DEFAULT_INTRA } else { DEFAULT_INTER };
        Self {
            l4: [[16; 16]; 6],
            l8: core::array::from_fn(pick),
            l16: core::array::from_fn(pick),
            l32: [DEFAULT_INTRA, DEFAULT_INTER],
            dc16: [16; 6],
            dc32: [16; 2],
        }
    }

    /// Parse `scaling_list_data()`.
    pub fn parse(r: &mut BitReader) -> Result<Self> {
        let mut s = Self::default_lists();
        for size_id in 0..4usize {
            let step = if size_id == 3 { 3 } else { 1 };
            let mut matrix_id = 0usize;
            while matrix_id < 6 {
                let slot = if size_id == 3 { matrix_id / 3 } else { matrix_id };
                let pred_mode = r.flag()?;
                if !pred_mode {
                    let delta = r.ue()? as usize * step;
                    if delta > matrix_id {
                        return Err(Error::Invalid("scaling list refers to a matrix before the first"));
                    }
                    if delta == 0 {
                        // The default list (and DC 16).
                        match size_id {
                            0 => s.l4[slot] = [16; 16],
                            1 => s.l8[slot] = if matrix_id < 3 { DEFAULT_INTRA } else { DEFAULT_INTER },
                            2 => {
                                s.l16[slot] = if matrix_id < 3 { DEFAULT_INTRA } else { DEFAULT_INTER };
                                s.dc16[slot] = 16;
                            }
                            _ => {
                                s.l32[slot] = if matrix_id < 3 { DEFAULT_INTRA } else { DEFAULT_INTER };
                                s.dc32[slot] = 16;
                            }
                        }
                    } else {
                        let rslot = if size_id == 3 { (matrix_id - delta) / 3 } else { matrix_id - delta };
                        match size_id {
                            0 => s.l4[slot] = s.l4[rslot],
                            1 => s.l8[slot] = s.l8[rslot],
                            2 => {
                                s.l16[slot] = s.l16[rslot];
                                s.dc16[slot] = s.dc16[rslot];
                            }
                            _ => {
                                s.l32[slot] = s.l32[rslot];
                                s.dc32[slot] = s.dc32[rslot];
                            }
                        }
                    }
                } else {
                    let mut next = 8i32;
                    let coef_num = 64.min(1 << (4 + (size_id << 1)));
                    if size_id > 1 {
                        let dc = r.se()? + 8;
                        if !(1..=255).contains(&dc) {
                            return Err(Error::Invalid("scaling list DC out of range"));
                        }
                        next = dc;
                        if size_id == 2 {
                            s.dc16[slot] = dc as u8;
                        } else {
                            s.dc32[slot] = dc as u8;
                        }
                    }
                    for i in 0..coef_num {
                        let delta = r.se()?;
                        next = (next + delta + 256).rem_euclid(256);
                        match size_id {
                            0 => s.l4[slot][i] = next as u8,
                            1 => s.l8[slot][i] = next as u8,
                            2 => s.l16[slot][i] = next as u8,
                            _ => s.l32[slot][i] = next as u8,
                        }
                    }
                }
                matrix_id += step;
            }
        }
        Ok(s)
    }
}

/// Colour description from the VUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    /// `video_full_range_flag`.
    pub full_range: bool,
    /// `colour_primaries` (2 when unspecified).
    pub primaries: u8,
    /// `transfer_characteristics` (2 when unspecified): 16 is PQ, 18 is HLG.
    pub transfer: u8,
    /// `matrix_coeffs` (2 when unspecified).
    pub matrix: u8,
}

impl Default for Colour {
    fn default() -> Self {
        Self { full_range: false, primaries: 2, transfer: 2, matrix: 2 }
    }
}

/// The SPS fields a decoder needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Sps {
    /// `sps_seq_parameter_set_id`.
    pub id: u8,
    /// `general_profile_idc` (1 Main, 2 Main 10).
    pub profile: u8,
    /// `general_tier_flag`.
    pub tier: bool,
    /// `general_level_idc`.
    pub level: u8,
    /// `sps_max_sub_layers_minus1`.
    pub max_sub_layers_minus1: u8,
    /// `chroma_format_idc` (1 for the supported profiles).
    pub chroma_format_idc: u8,
    /// `pic_width_in_luma_samples`.
    pub width: u32,
    /// `pic_height_in_luma_samples`.
    pub height: u32,
    /// The conformance window: left, right, top, bottom, in luma samples (the offsets are in chroma units, here already scaled).
    pub crop: [u32; 4],
    /// `BitDepthY`.
    pub bit_depth_luma: u8,
    /// `BitDepthC`.
    pub bit_depth_chroma: u8,
    /// `MaxPicOrderCntLsb`.
    pub max_poc_lsb: u32,
    /// `sps_max_dec_pic_buffering_minus1` of the highest sub-layer.
    pub max_dec_pic_buffering_minus1: u8,
    /// `sps_max_num_reorder_pics` of the highest sub-layer.
    pub max_num_reorder_pics: u8,
    /// `sps_max_latency_increase_plus1` of the highest sub-layer.
    pub max_latency_increase_plus1: u32,
    /// `MinCbLog2SizeY`.
    pub log2_min_cb: u8,
    /// `CtbLog2SizeY`.
    pub log2_ctb: u8,
    /// `MinTbLog2SizeY`.
    pub log2_min_tb: u8,
    /// `MaxTbLog2SizeY`.
    pub log2_max_tb: u8,
    /// `max_transform_hierarchy_depth_inter`.
    pub max_th_depth_inter: u8,
    /// `max_transform_hierarchy_depth_intra`.
    pub max_th_depth_intra: u8,
    /// `scaling_list_enabled_flag`.
    pub scaling_list_enabled: bool,
    /// The lists the SPS carries or implies (defaults when enabled without data); `None` when scaling lists are off.
    pub scaling_list: Option<ScalingList>,
    /// `amp_enabled_flag`.
    pub amp_enabled: bool,
    /// `sample_adaptive_offset_enabled_flag`.
    pub sao_enabled: bool,
    /// `pcm_enabled_flag`.
    pub pcm_enabled: bool,
    /// `PcmBitDepthY`.
    pub pcm_bit_depth_luma: u8,
    /// `PcmBitDepthC`.
    pub pcm_bit_depth_chroma: u8,
    /// `Log2MinIpcmCbSizeY`.
    pub log2_min_pcm_cb: u8,
    /// `Log2MaxIpcmCbSizeY`.
    pub log2_max_pcm_cb: u8,
    /// `pcm_loop_filter_disabled_flag`.
    pub pcm_loop_filter_disabled: bool,
    /// The short-term RPS candidates.
    pub st_rps: Vec<ShortTermRps>,
    /// `long_term_ref_pics_present_flag`.
    pub long_term_ref_pics_present: bool,
    /// `lt_ref_pic_poc_lsb_sps` and `used_by_curr_pic_lt_sps_flag`.
    pub lt_ref_pics: Vec<(u32, bool)>,
    /// `sps_temporal_mvp_enabled_flag`.
    pub temporal_mvp_enabled: bool,
    /// `strong_intra_smoothing_enabled_flag`.
    pub strong_intra_smoothing: bool,
    /// The colour description, when the VUI has one.
    pub colour: Colour,
}

impl Sps {
    /// `CtbSizeY`.
    pub fn ctb_size(&self) -> u32 {
        1 << self.log2_ctb
    }

    /// `PicWidthInCtbsY`.
    pub fn width_in_ctbs(&self) -> u32 {
        self.width.div_ceil(self.ctb_size())
    }

    /// `PicHeightInCtbsY`.
    pub fn height_in_ctbs(&self) -> u32 {
        self.height.div_ceil(self.ctb_size())
    }

    /// `PicSizeInCtbsY`.
    pub fn pic_size_in_ctbs(&self) -> u32 {
        self.width_in_ctbs() * self.height_in_ctbs()
    }

    /// The displayed size after the conformance window.
    pub fn display_size(&self) -> (u32, u32) {
        (self.width - self.crop[0] - self.crop[1], self.height - self.crop[2] - self.crop[3])
    }

    /// The picture buffers the stream needs at most (`sps_max_dec_pic_buffering_minus1 + 1`).
    pub fn dpb_size(&self) -> usize {
        self.max_dec_pic_buffering_minus1 as usize + 1
    }

    /// Parse an SPS from its RBSP (the bytes after the two-byte NAL header, emulation prevention removed).
    pub fn parse(rbsp: &[u8]) -> Result<Self> {
        let mut r = BitReader::new(rbsp);
        r.bits(4)?; // sps_video_parameter_set_id
        let max_sub_layers_minus1 = r.bits(3)? as u8;
        if max_sub_layers_minus1 > 6 {
            return Err(Error::Invalid("sps_max_sub_layers_minus1"));
        }
        r.flag()?; // sps_temporal_id_nesting_flag
        let (profile, tier, level) = parse_profile_tier_level(&mut r, max_sub_layers_minus1)?;
        let id = r.ue()?;
        if id > 15 {
            return Err(Error::Invalid("sps id"));
        }
        let chroma_format_idc = r.ue()?;
        if chroma_format_idc != 1 {
            return Err(Error::Unsupported("chroma format other than 4:2:0"));
        }
        let width = r.ue()?;
        let height = r.ue()?;
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err(Error::Invalid("picture size"));
        }
        let mut crop = [0u32; 4];
        if r.flag()? {
            // The offsets count in units of two luma samples for 4:2:0.
            for c in crop.iter_mut() {
                *c = r.ue()? * 2;
            }
            if crop[0] + crop[1] >= width || crop[2] + crop[3] >= height {
                return Err(Error::Invalid("conformance window"));
            }
        }
        let bit_depth_luma = r.ue()? + 8;
        let bit_depth_chroma = r.ue()? + 8;
        if !matches!(bit_depth_luma, 8 | 10) || bit_depth_luma != bit_depth_chroma {
            return Err(Error::Unsupported("bit depth other than 8 or 10"));
        }
        let log2_max_poc_lsb = r.ue()? + 4;
        if log2_max_poc_lsb > 16 {
            return Err(Error::Invalid("log2_max_pic_order_cnt_lsb"));
        }
        let sub_layer_ordering = r.flag()?;
        let (mut dpb, mut reorder, mut latency) = (0, 0, 0);
        let first = if sub_layer_ordering { 0 } else { max_sub_layers_minus1 };
        for _ in first..=max_sub_layers_minus1 {
            dpb = r.ue()?;
            reorder = r.ue()?;
            latency = r.ue()?;
        }
        if dpb > 15 || reorder > dpb {
            return Err(Error::Invalid("decoded picture buffer sizes"));
        }
        let log2_min_cb = r.ue()? + 3;
        let log2_diff_cb = r.ue()?;
        let log2_min_tb = r.ue()? + 2;
        let log2_diff_tb = r.ue()?;
        let (log2_ctb, log2_max_tb) = (log2_min_cb + log2_diff_cb, log2_min_tb + log2_diff_tb);
        if !(4..=6).contains(&log2_ctb) || log2_max_tb > 5 || log2_min_tb >= log2_min_cb {
            return Err(Error::Invalid("coding or transform block sizes"));
        }
        let max_th_depth_inter = r.ue()?;
        let max_th_depth_intra = r.ue()?;
        let scaling_list_enabled = r.flag()?;
        let mut scaling_list = None;
        if scaling_list_enabled {
            scaling_list =
                Some(if r.flag()? { ScalingList::parse(&mut r)? } else { ScalingList::default_lists() });
        }
        let amp_enabled = r.flag()?;
        let sao_enabled = r.flag()?;
        let pcm_enabled = r.flag()?;
        let (mut pcm_l, mut pcm_c, mut pcm_min, mut pcm_max, mut pcm_lf) = (0, 0, 0, 0, false);
        if pcm_enabled {
            pcm_l = r.bits(4)? as u8 + 1;
            pcm_c = r.bits(4)? as u8 + 1;
            pcm_min = r.ue()? as u8 + 3;
            pcm_max = pcm_min + r.ue()? as u8;
            pcm_lf = r.flag()?;
        }
        let num_st = r.ue()? as usize;
        if num_st > 64 {
            return Err(Error::Invalid("num_short_term_ref_pic_sets"));
        }
        let mut st_rps: Vec<ShortTermRps> = Vec::with_capacity(num_st);
        for i in 0..num_st {
            let s = parse_short_term_rps(&mut r, i, num_st, &st_rps)?;
            st_rps.push(s);
        }
        let long_term_ref_pics_present = r.flag()?;
        let mut lt_ref_pics = Vec::new();
        if long_term_ref_pics_present {
            let n = r.ue()?;
            if n > 32 {
                return Err(Error::Invalid("num_long_term_ref_pics_sps"));
            }
            for _ in 0..n {
                let lsb = r.bits(log2_max_poc_lsb)?;
                lt_ref_pics.push((lsb, r.flag()?));
            }
        }
        let temporal_mvp_enabled = r.flag()?;
        let strong_intra_smoothing = r.flag()?;
        let mut colour = Colour::default();
        if r.flag()? {
            colour = parse_vui_colour(&mut r)?;
        }
        Ok(Self {
            id: id as u8,
            profile,
            tier,
            level,
            max_sub_layers_minus1,
            chroma_format_idc: chroma_format_idc as u8,
            width,
            height,
            crop,
            bit_depth_luma: bit_depth_luma as u8,
            bit_depth_chroma: bit_depth_chroma as u8,
            max_poc_lsb: 1 << log2_max_poc_lsb,
            max_dec_pic_buffering_minus1: dpb as u8,
            max_num_reorder_pics: reorder as u8,
            max_latency_increase_plus1: latency,
            log2_min_cb: log2_min_cb as u8,
            log2_ctb: log2_ctb as u8,
            log2_min_tb: log2_min_tb as u8,
            log2_max_tb: log2_max_tb as u8,
            max_th_depth_inter: max_th_depth_inter as u8,
            max_th_depth_intra: max_th_depth_intra as u8,
            scaling_list_enabled,
            scaling_list,
            amp_enabled,
            sao_enabled,
            pcm_enabled,
            pcm_bit_depth_luma: pcm_l,
            pcm_bit_depth_chroma: pcm_c,
            log2_min_pcm_cb: pcm_min,
            log2_max_pcm_cb: pcm_max,
            pcm_loop_filter_disabled: pcm_lf,
            st_rps,
            long_term_ref_pics_present,
            lt_ref_pics,
            temporal_mvp_enabled,
            strong_intra_smoothing,
            colour,
        })
    }
}

/// `profile_tier_level(1, max_sub_layers_minus1)`: (profile, tier, level) of the general part; the sub-layer parts are skipped.
fn parse_profile_tier_level(r: &mut BitReader, max_sub_layers_minus1: u8) -> Result<(u8, bool, u8)> {
    let space = r.bits(2)?;
    let tier = r.flag()?;
    let mut profile = r.bits(5)? as u8;
    let compat = r.bits(32)?;
    r.skip(4 + 43 + 1)?; // source flags, constraint flags, reserved, inbld
    let level = r.bits(8)? as u8;
    if space != 0 {
        return Err(Error::Unsupported("profile space"));
    }
    // A stream may signal profile 0 and set the compatibility flag instead; take the lowest one it is compatible with.
    if profile == 0 {
        profile = (1..32u8).find(|p| compat & (1 << (31 - p)) != 0).unwrap_or(0);
    }
    if !matches!(profile, 1 | 2) {
        return Err(Error::Unsupported("HEVC profile other than Main and Main 10"));
    }
    let mut profile_present = [false; 8];
    let mut level_present = [false; 8];
    for i in 0..max_sub_layers_minus1 as usize {
        profile_present[i] = r.flag()?;
        level_present[i] = r.flag()?;
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            r.bits(2)?;
        }
    }
    for i in 0..max_sub_layers_minus1 as usize {
        if profile_present[i] {
            r.skip(88)?;
        }
        if level_present[i] {
            r.skip(8)?;
        }
    }
    Ok((profile, tier, level))
}

/// The start of `vui_parameters()`, as far as the colour description.
fn parse_vui_colour(r: &mut BitReader) -> Result<Colour> {
    let mut c = Colour::default();
    if r.flag()? {
        // aspect_ratio_info_present_flag
        if r.bits(8)? == 255 {
            r.skip(32)?;
        }
    }
    if r.flag()? {
        r.flag()?; // overscan_appropriate_flag
    }
    if r.flag()? {
        // video_signal_type_present_flag
        r.bits(3)?; // video_format
        c.full_range = r.flag()?;
        if r.flag()? {
            c.primaries = r.bits(8)? as u8;
            c.transfer = r.bits(8)? as u8;
            c.matrix = r.bits(8)? as u8;
        }
    }
    Ok(c)
}

/// The PPS fields a decoder needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Pps {
    /// `pps_pic_parameter_set_id`.
    pub id: u8,
    /// `pps_seq_parameter_set_id`.
    pub sps_id: u8,
    /// `dependent_slice_segments_enabled_flag`.
    pub dependent_slice_segments_enabled: bool,
    /// `output_flag_present_flag`.
    pub output_flag_present: bool,
    /// `num_extra_slice_header_bits`.
    pub num_extra_slice_header_bits: u8,
    /// `sign_data_hiding_enabled_flag`.
    pub sign_data_hiding_enabled: bool,
    /// `cabac_init_present_flag`.
    pub cabac_init_present: bool,
    /// `num_ref_idx_l0_default_active_minus1`.
    pub num_ref_idx_l0_default_active_minus1: u8,
    /// `num_ref_idx_l1_default_active_minus1`.
    pub num_ref_idx_l1_default_active_minus1: u8,
    /// `init_qp_minus26`.
    pub init_qp_minus26: i8,
    /// `constrained_intra_pred_flag`.
    pub constrained_intra_pred: bool,
    /// `transform_skip_enabled_flag`.
    pub transform_skip_enabled: bool,
    /// `cu_qp_delta_enabled_flag`.
    pub cu_qp_delta_enabled: bool,
    /// `diff_cu_qp_delta_depth`.
    pub diff_cu_qp_delta_depth: u8,
    /// `pps_cb_qp_offset`.
    pub cb_qp_offset: i8,
    /// `pps_cr_qp_offset`.
    pub cr_qp_offset: i8,
    /// `pps_slice_chroma_qp_offsets_present_flag`.
    pub slice_chroma_qp_offsets_present: bool,
    /// `weighted_pred_flag`.
    pub weighted_pred: bool,
    /// `weighted_bipred_flag`.
    pub weighted_bipred: bool,
    /// `transquant_bypass_enabled_flag`.
    pub transquant_bypass_enabled: bool,
    /// `tiles_enabled_flag`.
    pub tiles_enabled: bool,
    /// `entropy_coding_sync_enabled_flag`.
    pub entropy_coding_sync_enabled: bool,
    /// `num_tile_columns_minus1`.
    pub num_tile_columns_minus1: u8,
    /// `num_tile_rows_minus1`.
    pub num_tile_rows_minus1: u8,
    /// `uniform_spacing_flag`.
    pub uniform_spacing: bool,
    /// `column_width_minus1` (explicit widths, all but the last column).
    pub column_width_minus1: Vec<u32>,
    /// `row_height_minus1` (all but the last row).
    pub row_height_minus1: Vec<u32>,
    /// `loop_filter_across_tiles_enabled_flag`.
    pub loop_filter_across_tiles_enabled: bool,
    /// `pps_loop_filter_across_slices_enabled_flag`.
    pub loop_filter_across_slices_enabled: bool,
    /// `deblocking_filter_control_present_flag`.
    pub deblocking_filter_control_present: bool,
    /// `deblocking_filter_override_enabled_flag`.
    pub deblocking_filter_override_enabled: bool,
    /// `pps_deblocking_filter_disabled_flag`.
    pub deblocking_filter_disabled: bool,
    /// `pps_beta_offset_div2`.
    pub beta_offset_div2: i8,
    /// `pps_tc_offset_div2`.
    pub tc_offset_div2: i8,
    /// The PPS's own scaling lists, if it has any (they win over the SPS's).
    pub scaling_list: Option<ScalingList>,
    /// `lists_modification_present_flag`.
    pub lists_modification_present: bool,
    /// `log2_parallel_merge_level_minus2`.
    pub log2_parallel_merge_level_minus2: u8,
    /// `slice_segment_header_extension_present_flag`.
    pub slice_segment_header_extension_present: bool,
}

impl Pps {
    /// Parse a PPS from its RBSP.
    pub fn parse(rbsp: &[u8]) -> Result<Self> {
        let mut r = BitReader::new(rbsp);
        let id = r.ue()?;
        let sps_id = r.ue()?;
        if id > 63 || sps_id > 15 {
            return Err(Error::Invalid("parameter set id"));
        }
        let dependent_slice_segments_enabled = r.flag()?;
        let output_flag_present = r.flag()?;
        let num_extra_slice_header_bits = r.bits(3)? as u8;
        let sign_data_hiding_enabled = r.flag()?;
        let cabac_init_present = r.flag()?;
        let (l0, l1) = (r.ue()?, r.ue()?);
        if l0 > 14 || l1 > 14 {
            return Err(Error::Invalid("default reference counts"));
        }
        let init_qp_minus26 = r.se()?;
        let constrained_intra_pred = r.flag()?;
        let transform_skip_enabled = r.flag()?;
        let cu_qp_delta_enabled = r.flag()?;
        let diff_cu_qp_delta_depth = if cu_qp_delta_enabled { r.ue()? } else { 0 };
        let cb_qp_offset = r.se()?;
        let cr_qp_offset = r.se()?;
        let slice_chroma_qp_offsets_present = r.flag()?;
        let weighted_pred = r.flag()?;
        let weighted_bipred = r.flag()?;
        let transquant_bypass_enabled = r.flag()?;
        let tiles_enabled = r.flag()?;
        let entropy_coding_sync_enabled = r.flag()?;
        let (mut cols_m1, mut rows_m1, mut uniform) = (0, 0, true);
        let (mut widths, mut heights) = (Vec::new(), Vec::new());
        let mut lf_tiles = true;
        if tiles_enabled {
            cols_m1 = r.ue()?;
            rows_m1 = r.ue()?;
            if cols_m1 > 19 || rows_m1 > 21 {
                return Err(Error::Invalid("tile counts"));
            }
            uniform = r.flag()?;
            if !uniform {
                for _ in 0..cols_m1 {
                    widths.push(r.ue()?);
                }
                for _ in 0..rows_m1 {
                    heights.push(r.ue()?);
                }
            }
            lf_tiles = r.flag()?;
        }
        let loop_filter_across_slices_enabled = r.flag()?;
        let deblocking_filter_control_present = r.flag()?;
        let (mut override_enabled, mut disabled, mut beta, mut tc) = (false, false, 0, 0);
        if deblocking_filter_control_present {
            override_enabled = r.flag()?;
            disabled = r.flag()?;
            if !disabled {
                beta = r.se()?;
                tc = r.se()?;
            }
        }
        let scaling_list = if r.flag()? { Some(ScalingList::parse(&mut r)?) } else { None };
        let lists_modification_present = r.flag()?;
        let log2_parallel_merge_level_minus2 = r.ue()?;
        let slice_segment_header_extension_present = r.flag()?;
        Ok(Self {
            id: id as u8,
            sps_id: sps_id as u8,
            dependent_slice_segments_enabled,
            output_flag_present,
            num_extra_slice_header_bits,
            sign_data_hiding_enabled,
            cabac_init_present,
            num_ref_idx_l0_default_active_minus1: l0 as u8,
            num_ref_idx_l1_default_active_minus1: l1 as u8,
            init_qp_minus26: init_qp_minus26 as i8,
            constrained_intra_pred,
            transform_skip_enabled,
            cu_qp_delta_enabled,
            diff_cu_qp_delta_depth: diff_cu_qp_delta_depth as u8,
            cb_qp_offset: cb_qp_offset as i8,
            cr_qp_offset: cr_qp_offset as i8,
            slice_chroma_qp_offsets_present,
            weighted_pred,
            weighted_bipred,
            transquant_bypass_enabled,
            tiles_enabled,
            entropy_coding_sync_enabled,
            num_tile_columns_minus1: cols_m1 as u8,
            num_tile_rows_minus1: rows_m1 as u8,
            uniform_spacing: uniform,
            column_width_minus1: widths,
            row_height_minus1: heights,
            loop_filter_across_tiles_enabled: lf_tiles,
            loop_filter_across_slices_enabled,
            deblocking_filter_control_present,
            deblocking_filter_override_enabled: override_enabled,
            deblocking_filter_disabled: disabled,
            beta_offset_div2: beta as i8,
            tc_offset_div2: tc as i8,
            scaling_list,
            lists_modification_present,
            log2_parallel_merge_level_minus2: log2_parallel_merge_level_minus2 as u8,
            slice_segment_header_extension_present,
        })
    }

    /// The column widths and row heights of the tile grid in CTBs (6.5.1): one tile when tiles are off.
    pub fn tile_grid(&self, sps: &Sps) -> (Vec<u32>, Vec<u32>) {
        let (cols, rows) = (self.num_tile_columns_minus1 as u32 + 1, self.num_tile_rows_minus1 as u32 + 1);
        let (pw, ph) = (sps.width_in_ctbs(), sps.height_in_ctbs());
        let split = |n: u32, total: u32, explicit: &[u32]| -> Vec<u32> {
            if self.uniform_spacing {
                (0..n).map(|i| ((i + 1) * total) / n - (i * total) / n).collect()
            } else {
                let mut v: Vec<u32> = explicit.iter().map(|x| x + 1).collect();
                let used: u32 = v.iter().sum();
                v.push(total.saturating_sub(used));
                v
            }
        };
        (split(cols, pw, &self.column_width_minus1), split(rows, ph, &self.row_height_minus1))
    }
}

/// `Ceil(Log2(PicSizeInCtbsY))`: the bits of `slice_segment_address`.
pub fn slice_address_bits(sps: &Sps) -> u32 {
    ceil_log2(sps.pic_size_in_ctbs())
}
