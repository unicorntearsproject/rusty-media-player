//! Sequence parameter set (7.3.2.1.1) and VUI (Annex E).
use super::scaling::ScalingSyntax;
use crate::bitstream::{BitReader, BitWriter};
use crate::error::{Error, Result};
use alloc::vec::Vec;

/// Frame cropping offsets, in crop units (see [`Sps::crop_rect`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Crop {
    /// `frame_crop_left_offset`.
    pub left: u32,
    /// `frame_crop_right_offset`.
    pub right: u32,
    /// `frame_crop_top_offset`.
    pub top: u32,
    /// `frame_crop_bottom_offset`.
    pub bottom: u32,
}

/// The parts of the VUI that a player or encoder cares about. HRD parameters are parsed and dropped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Vui {
    /// `aspect_ratio_idc` and, for Extended_SAR (255), `(sar_width, sar_height)`.
    pub aspect_ratio: Option<(u8, u16, u16)>,
    /// `video_format`.
    pub video_format: u8,
    /// `video_full_range_flag`.
    pub full_range: bool,
    /// `(colour_primaries, transfer_characteristics, matrix_coefficients)` if signalled.
    pub colour: Option<(u8, u8, u8)>,
    /// `(num_units_in_tick, time_scale, fixed_frame_rate_flag)` if signalled.
    pub timing: Option<(u32, u32, bool)>,
    /// `num_reorder_frames`, if `bitstream_restriction_flag` was set.
    pub num_reorder_frames: Option<u32>,
    /// `max_dec_frame_buffering`, if `bitstream_restriction_flag` was set.
    pub max_dec_frame_buffering: Option<u32>,
}

/// A sequence parameter set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sps {
    /// `profile_idc`.
    pub profile_idc: u8,
    /// `constraint_set0_flag` (bit 7) to `constraint_set5_flag` (bit 2) and the 2 reserved bits, as in the byte.
    pub constraint_flags: u8,
    /// `level_idc`.
    pub level_idc: u8,
    /// `seq_parameter_set_id`.
    pub id: u32,
    /// `chroma_format_idc` (1 = 4:2:0).
    pub chroma_format_idc: u32,
    /// `separate_colour_plane_flag`.
    pub separate_colour_plane: bool,
    /// `bit_depth_luma_minus8 + 8`.
    pub bit_depth_luma: u32,
    /// `bit_depth_chroma_minus8 + 8`.
    pub bit_depth_chroma: u32,
    /// `qpprime_y_zero_transform_bypass_flag`.
    pub transform_bypass: bool,
    /// The sequence scaling matrix, if `seq_scaling_matrix_present_flag`.
    pub scaling: Option<ScalingSyntax>,
    /// `log2_max_frame_num_minus4 + 4`.
    pub log2_max_frame_num: u32,
    /// `pic_order_cnt_type`.
    pub poc_type: u32,
    /// `log2_max_pic_order_cnt_lsb_minus4 + 4` (type 0).
    pub log2_max_poc_lsb: u32,
    /// `delta_pic_order_always_zero_flag` (type 1).
    pub delta_pic_order_always_zero: bool,
    /// `offset_for_non_ref_pic` (type 1).
    pub offset_for_non_ref_pic: i32,
    /// `offset_for_top_to_bottom_field` (type 1).
    pub offset_for_top_to_bottom_field: i32,
    /// `offset_for_ref_frame[]` (type 1).
    pub offset_for_ref_frame: Vec<i32>,
    /// `max_num_ref_frames`.
    pub max_num_ref_frames: u32,
    /// `gaps_in_frame_num_value_allowed_flag`.
    pub gaps_in_frame_num_allowed: bool,
    /// `pic_width_in_mbs_minus1 + 1`.
    pub pic_width_in_mbs: u32,
    /// `pic_height_in_map_units_minus1 + 1`.
    pub pic_height_in_map_units: u32,
    /// `frame_mbs_only_flag`.
    pub frame_mbs_only: bool,
    /// `mb_adaptive_frame_field_flag`.
    pub mb_adaptive_frame_field: bool,
    /// `direct_8x8_inference_flag`.
    pub direct_8x8_inference: bool,
    /// Frame cropping, if `frame_cropping_flag`.
    pub crop: Option<Crop>,
    /// VUI parameters, if present.
    pub vui: Option<Vui>,
}

const HIGH_PROFILES: [u8; 9] = [100, 110, 122, 244, 44, 83, 86, 118, 128];

fn is_high(profile: u8) -> bool {
    HIGH_PROFILES.contains(&profile) || matches!(profile, 138 | 139 | 134 | 135)
}

fn skip_hrd(r: &mut BitReader) -> Result<()> {
    let cpb_cnt = r.read_ue()?;
    if cpb_cnt > 31 {
        return Err(Error::Invalid("cpb_cnt_minus1 out of range"));
    }
    r.read_bits(8)?;
    for _ in 0..=cpb_cnt {
        r.read_ue()?;
        r.read_ue()?;
        r.read_bit()?;
    }
    r.read_bits(20)?;
    Ok(())
}

fn parse_vui(r: &mut BitReader) -> Result<Vui> {
    let mut v = Vui::default();
    if r.read_flag()? {
        let idc = r.read_bits(8)? as u8;
        let (w, h) = if idc == 255 { (r.read_bits(16)? as u16, r.read_bits(16)? as u16) } else { (0, 0) };
        v.aspect_ratio = Some((idc, w, h));
    }
    if r.read_flag()? {
        r.read_bit()?; // overscan_appropriate_flag
    }
    v.video_format = 5;
    if r.read_flag()? {
        v.video_format = r.read_bits(3)? as u8;
        v.full_range = r.read_flag()?;
        if r.read_flag()? {
            v.colour = Some((r.read_bits(8)? as u8, r.read_bits(8)? as u8, r.read_bits(8)? as u8));
        }
    }
    if r.read_flag()? {
        r.read_ue()?;
        r.read_ue()?;
    }
    if r.read_flag()? {
        let n = r.read_bits(32)?;
        let t = r.read_bits(32)?;
        v.timing = Some((n, t, r.read_flag()?));
    }
    let nal_hrd = r.read_flag()?;
    if nal_hrd {
        skip_hrd(r)?;
    }
    let vcl_hrd = r.read_flag()?;
    if vcl_hrd {
        skip_hrd(r)?;
    }
    if nal_hrd || vcl_hrd {
        r.read_bit()?; // low_delay_hrd_flag
    }
    r.read_bit()?; // pic_struct_present_flag
    if r.read_flag()? {
        r.read_bit()?; // motion_vectors_over_pic_boundaries_flag
        r.read_ue()?;
        r.read_ue()?;
        r.read_ue()?;
        r.read_ue()?;
        v.num_reorder_frames = Some(r.read_ue()?);
        v.max_dec_frame_buffering = Some(r.read_ue()?);
    }
    Ok(v)
}

impl Sps {
    /// Parse an SPS RBSP (the NAL payload after the header byte, emulation prevention removed).
    pub fn parse(rbsp: &[u8]) -> Result<Self> {
        let mut r = BitReader::new(rbsp);
        let profile_idc = r.read_bits(8)? as u8;
        let constraint_flags = r.read_bits(8)? as u8;
        let level_idc = r.read_bits(8)? as u8;
        let id = r.read_ue()?;
        if id > 31 {
            return Err(Error::Invalid("seq_parameter_set_id out of range"));
        }
        let mut s = Sps {
            profile_idc,
            constraint_flags,
            level_idc,
            id,
            chroma_format_idc: 1,
            separate_colour_plane: false,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            transform_bypass: false,
            scaling: None,
            log2_max_frame_num: 4,
            poc_type: 0,
            log2_max_poc_lsb: 4,
            delta_pic_order_always_zero: false,
            offset_for_non_ref_pic: 0,
            offset_for_top_to_bottom_field: 0,
            offset_for_ref_frame: Vec::new(),
            max_num_ref_frames: 0,
            gaps_in_frame_num_allowed: false,
            pic_width_in_mbs: 0,
            pic_height_in_map_units: 0,
            frame_mbs_only: true,
            mb_adaptive_frame_field: false,
            direct_8x8_inference: false,
            crop: None,
            vui: None,
        };
        if is_high(profile_idc) {
            s.chroma_format_idc = r.read_ue()?;
            if s.chroma_format_idc > 3 {
                return Err(Error::Invalid("chroma_format_idc out of range"));
            }
            if s.chroma_format_idc == 3 {
                s.separate_colour_plane = r.read_flag()?;
            }
            let bl = r.read_ue()?;
            let bc = r.read_ue()?;
            if bl > 6 || bc > 6 {
                return Err(Error::Invalid("bit depth out of range"));
            }
            s.bit_depth_luma = bl + 8;
            s.bit_depth_chroma = bc + 8;
            s.transform_bypass = r.read_flag()?;
            if r.read_flag()? {
                let n = if s.chroma_format_idc != 3 { 8 } else { 12 };
                s.scaling = Some(ScalingSyntax::parse(&mut r, n)?);
            }
        }
        let l = r.read_ue()?;
        if l > 12 {
            return Err(Error::Invalid("log2_max_frame_num_minus4 out of range"));
        }
        s.log2_max_frame_num = l + 4;
        s.poc_type = r.read_ue()?;
        match s.poc_type {
            0 => {
                let l = r.read_ue()?;
                if l > 12 {
                    return Err(Error::Invalid("log2_max_pic_order_cnt_lsb_minus4 out of range"));
                }
                s.log2_max_poc_lsb = l + 4;
            }
            1 => {
                s.delta_pic_order_always_zero = r.read_flag()?;
                s.offset_for_non_ref_pic = r.read_se()?;
                s.offset_for_top_to_bottom_field = r.read_se()?;
                let n = r.read_ue()?;
                if n > 255 {
                    return Err(Error::Invalid("num_ref_frames_in_pic_order_cnt_cycle out of range"));
                }
                for _ in 0..n {
                    s.offset_for_ref_frame.push(r.read_se()?);
                }
            }
            2 => {}
            _ => return Err(Error::Invalid("pic_order_cnt_type out of range")),
        }
        s.max_num_ref_frames = r.read_ue()?;
        if s.max_num_ref_frames > 16 {
            return Err(Error::Invalid("max_num_ref_frames out of range"));
        }
        s.gaps_in_frame_num_allowed = r.read_flag()?;
        s.pic_width_in_mbs = r.read_ue()?.saturating_add(1);
        s.pic_height_in_map_units = r.read_ue()?.saturating_add(1);
        if s.pic_width_in_mbs > 1024
            || s.pic_height_in_map_units > 1024
            || s.pic_width_in_mbs * s.pic_height_in_map_units > 139_264
        {
            return Err(Error::Unsupported("picture too large"));
        }
        s.frame_mbs_only = r.read_flag()?;
        if !s.frame_mbs_only {
            s.mb_adaptive_frame_field = r.read_flag()?;
        }
        s.direct_8x8_inference = r.read_flag()?;
        if r.read_flag()? {
            s.crop = Some(Crop {
                left: r.read_ue()?,
                right: r.read_ue()?,
                top: r.read_ue()?,
                bottom: r.read_ue()?,
            });
        }
        if r.read_flag()? {
            // A damaged VUI must not make the whole SPS unusable: keep what we have.
            s.vui = parse_vui(&mut r).ok();
        }
        Ok(s)
    }

    /// Serialise as an SPS RBSP (including trailing bits). Writes the VUI fields in [`Vui`] but no HRD.
    pub fn write(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put_bits(self.profile_idc as u32, 8);
        w.put_bits(self.constraint_flags as u32, 8);
        w.put_bits(self.level_idc as u32, 8);
        w.put_ue(self.id);
        if is_high(self.profile_idc) {
            w.put_ue(self.chroma_format_idc);
            if self.chroma_format_idc == 3 {
                w.put_bit(self.separate_colour_plane);
            }
            w.put_ue(self.bit_depth_luma - 8);
            w.put_ue(self.bit_depth_chroma - 8);
            w.put_bit(self.transform_bypass);
            match &self.scaling {
                Some(s) => {
                    w.put_bit(true);
                    s.write(&mut w);
                }
                None => w.put_bit(false),
            }
        }
        w.put_ue(self.log2_max_frame_num - 4);
        w.put_ue(self.poc_type);
        match self.poc_type {
            0 => w.put_ue(self.log2_max_poc_lsb - 4),
            1 => {
                w.put_bit(self.delta_pic_order_always_zero);
                w.put_se(self.offset_for_non_ref_pic);
                w.put_se(self.offset_for_top_to_bottom_field);
                w.put_ue(self.offset_for_ref_frame.len() as u32);
                for &o in &self.offset_for_ref_frame {
                    w.put_se(o);
                }
            }
            _ => {}
        }
        w.put_ue(self.max_num_ref_frames);
        w.put_bit(self.gaps_in_frame_num_allowed);
        w.put_ue(self.pic_width_in_mbs - 1);
        w.put_ue(self.pic_height_in_map_units - 1);
        w.put_bit(self.frame_mbs_only);
        if !self.frame_mbs_only {
            w.put_bit(self.mb_adaptive_frame_field);
        }
        w.put_bit(self.direct_8x8_inference);
        match &self.crop {
            Some(c) => {
                w.put_bit(true);
                w.put_ue(c.left);
                w.put_ue(c.right);
                w.put_ue(c.top);
                w.put_ue(c.bottom);
            }
            None => w.put_bit(false),
        }
        match &self.vui {
            Some(v) => {
                w.put_bit(true);
                match v.aspect_ratio {
                    Some((idc, sw, sh)) => {
                        w.put_bit(true);
                        w.put_bits(idc as u32, 8);
                        if idc == 255 {
                            w.put_bits(sw as u32, 16);
                            w.put_bits(sh as u32, 16);
                        }
                    }
                    None => w.put_bit(false),
                }
                w.put_bit(false); // overscan
                let signal = v.colour.is_some() || v.full_range || v.video_format != 5;
                w.put_bit(signal);
                if signal {
                    w.put_bits(v.video_format as u32, 3);
                    w.put_bit(v.full_range);
                    match v.colour {
                        Some((a, b, c)) => {
                            w.put_bit(true);
                            w.put_bits(a as u32, 8);
                            w.put_bits(b as u32, 8);
                            w.put_bits(c as u32, 8);
                        }
                        None => w.put_bit(false),
                    }
                }
                w.put_bit(false); // chroma_loc_info
                match v.timing {
                    Some((n, t, f)) => {
                        w.put_bit(true);
                        w.put_bits(n, 32);
                        w.put_bits(t, 32);
                        w.put_bit(f);
                    }
                    None => w.put_bit(false),
                }
                w.put_bit(false); // nal hrd
                w.put_bit(false); // vcl hrd
                w.put_bit(false); // pic_struct_present
                match (v.num_reorder_frames, v.max_dec_frame_buffering) {
                    (Some(n), Some(m)) => {
                        w.put_bit(true);
                        w.put_bit(true);
                        w.put_ue(2);
                        w.put_ue(1);
                        w.put_ue(16);
                        w.put_ue(16);
                        w.put_ue(n);
                        w.put_ue(m);
                    }
                    _ => w.put_bit(false),
                }
            }
            None => w.put_bit(false),
        }
        w.put_trailing_bits();
        w.into_bytes()
    }

    /// Width of the decoded frame in macroblocks.
    pub fn width_mbs(&self) -> usize {
        self.pic_width_in_mbs as usize
    }

    /// Height of the decoded frame in macroblocks (frame height in map units times two if interlaced).
    pub fn height_mbs(&self) -> usize {
        self.pic_height_in_map_units as usize * if self.frame_mbs_only { 1 } else { 2 }
    }

    /// `ChromaArrayType`.
    pub fn chroma_array_type(&self) -> u32 {
        if self.separate_colour_plane { 0 } else { self.chroma_format_idc }
    }

    /// The cropped output rectangle in luma samples: `(x, y, width, height)`.
    pub fn crop_rect(&self) -> (usize, usize, usize, usize) {
        let w = self.width_mbs() * 16;
        let h = self.height_mbs() * 16;
        let Some(c) = self.crop else { return (0, 0, w, h) };
        let (cx, cy) = match self.chroma_array_type() {
            0 => (1, 2 - self.frame_mbs_only as usize),
            1 => (2, 2 * (2 - self.frame_mbs_only as usize)),
            2 => (2, 2 - self.frame_mbs_only as usize),
            _ => (1, 2 - self.frame_mbs_only as usize),
        };
        let x = (c.left as usize).saturating_mul(cx);
        let y = (c.top as usize).saturating_mul(cy);
        let rx = (c.right as usize).saturating_mul(cx);
        let ry = (c.bottom as usize).saturating_mul(cy);
        if x + rx >= w || y + ry >= h {
            return (0, 0, w, h); // nonsense crop: ignore it
        }
        (x, y, w - x - rx, h - y - ry)
    }

    /// `MaxDpbMbs` from Table A-1 for this level.
    pub fn max_dpb_mbs(&self) -> u32 {
        let level_1b = self.level_idc == 11
            && self.constraint_flags & 0x10 != 0
            && matches!(self.profile_idc, 66 | 77 | 88);
        match self.level_idc {
            9 | 10 => 396,
            11 if level_1b => 396,
            11 => 900,
            12 | 13 | 20 => 2376,
            21 => 4752,
            22 | 30 => 8100,
            31 => 18000,
            32 => 20480,
            40 | 41 => 32768,
            42 => 34816,
            50 => 110_400,
            51 | 52 => 184_320,
            60..=62 => 696_320,
            _ => 184_320,
        }
    }

    /// The decoded picture buffer capacity in frames (A.3.1 item h / A.3.2 item f), at most 16, at least
    /// `max_num_ref_frames`; honours `max_dec_frame_buffering` from the VUI.
    pub fn dpb_frames(&self) -> usize {
        let frame_mbs = (self.width_mbs() * self.height_mbs()).max(1) as u32;
        let from_level = (self.max_dpb_mbs() / frame_mbs).min(16) as usize;
        let mut n = from_level;
        if let Some(m) = self.vui.as_ref().and_then(|v| v.max_dec_frame_buffering) {
            n = m as usize;
        }
        n.clamp(self.max_num_ref_frames.max(1) as usize, 16)
    }

    /// How many frames output may lag decode: `num_reorder_frames` if the VUI says, else the DPB size, and
    /// zero for POC type 2 (output order equals decode order).
    pub fn reorder_frames(&self) -> usize {
        if self.poc_type == 2 {
            return 0;
        }
        match self.vui.as_ref().and_then(|v| v.num_reorder_frames) {
            Some(n) => (n as usize).min(self.dpb_frames()),
            None => self.dpb_frames(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Sps {
        Sps {
            profile_idc: 100,
            constraint_flags: 0,
            level_idc: 31,
            id: 3,
            chroma_format_idc: 1,
            separate_colour_plane: false,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            transform_bypass: false,
            scaling: None,
            log2_max_frame_num: 8,
            poc_type: 1,
            log2_max_poc_lsb: 4,
            delta_pic_order_always_zero: false,
            offset_for_non_ref_pic: -2,
            offset_for_top_to_bottom_field: 0,
            offset_for_ref_frame: alloc::vec![2, 4],
            max_num_ref_frames: 4,
            gaps_in_frame_num_allowed: false,
            pic_width_in_mbs: 80,
            pic_height_in_map_units: 45,
            frame_mbs_only: true,
            mb_adaptive_frame_field: false,
            direct_8x8_inference: true,
            crop: Some(Crop { left: 0, right: 0, top: 0, bottom: 4 }),
            vui: Some(Vui {
                aspect_ratio: Some((255, 4, 3)),
                video_format: 5,
                full_range: true,
                colour: Some((1, 1, 1)),
                timing: Some((1, 60, true)),
                num_reorder_frames: Some(2),
                max_dec_frame_buffering: Some(4),
            }),
        }
    }

    #[test]
    fn roundtrip() {
        let s = base();
        let rbsp = s.write();
        assert_eq!(Sps::parse(&rbsp).unwrap(), s);
        assert_eq!(s.crop_rect(), (0, 0, 1280, 712));
        assert_eq!(s.dpb_frames(), 4);
        assert_eq!(s.reorder_frames(), 2);
    }

    #[test]
    fn truncation_never_panics() {
        let rbsp = base().write();
        for n in 0..rbsp.len() {
            let _ = Sps::parse(&rbsp[..n]);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    /// A small valid SPS for other modules' tests.
    pub fn sample() -> Sps {
        Sps {
            profile_idc: 100,
            constraint_flags: 0,
            level_idc: 40,
            id: 0,
            chroma_format_idc: 1,
            separate_colour_plane: false,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            transform_bypass: false,
            scaling: None,
            log2_max_frame_num: 4,
            poc_type: 0,
            log2_max_poc_lsb: 4,
            delta_pic_order_always_zero: false,
            offset_for_non_ref_pic: 0,
            offset_for_top_to_bottom_field: 0,
            offset_for_ref_frame: Vec::new(),
            max_num_ref_frames: 4,
            gaps_in_frame_num_allowed: false,
            pic_width_in_mbs: 4,
            pic_height_in_map_units: 3,
            frame_mbs_only: true,
            mb_adaptive_frame_field: false,
            direct_8x8_inference: true,
            crop: None,
            vui: None,
        }
    }
}
