//! A decoded picture: three planes of `u16` samples and the motion data later pictures read for temporal prediction.
use super::{format_of, matrix_of};
use crate::Result;
use crate::ps::Sps;
use alloc::vec;
use alloc::vec::Vec;
use rvp_core::{PixelFormat, VideoFrame};

/// One plane of samples.
#[derive(Clone, Default)]
pub struct Plane {
    /// Samples, `stride` per row.
    pub data: Vec<u16>,
    /// Row length in samples.
    pub stride: usize,
    /// Width in samples.
    pub width: usize,
    /// Height in samples.
    pub height: usize,
}

impl Plane {
    fn new(width: usize, height: usize) -> Self {
        Self { data: vec![0; width * height], stride: width, width, height }
    }
}

/// A motion vector in quarter luma samples.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Mv {
    /// Horizontal component.
    pub x: i16,
    /// Vertical component.
    pub y: i16,
}

/// What a later picture needs of the motion of this one (8.5.3.2.9): vectors, the POC of the pictures they point at, long-term marks.
#[derive(Clone, Copy, Default)]
pub struct ColMotion {
    /// Vectors for list 0 and list 1.
    pub mv: [Mv; 2],
    /// POC of the reference picture of each list.
    pub ref_poc: [i32; 2],
    /// Bit 0 / 1: the list is used. Bit 2 / 3: its reference is a long-term picture.
    pub flags: u8,
}

/// A decoded picture.
#[derive(Clone)]
pub struct Frame {
    /// Y, Cb, Cr.
    pub planes: [Plane; 3],
    /// `PicOrderCntVal`.
    pub poc: i32,
    /// Motion per 4x4 luma block, `w4` per row (all unused for an intra picture).
    pub motion: Vec<ColMotion>,
    /// Row length of `motion`.
    pub w4: usize,
}

impl Frame {
    /// A frame for pictures of this stream.
    pub fn new(sps: &Sps) -> Self {
        let (w, h) = (sps.width as usize, sps.height as usize);
        let w4 = w.div_ceil(4);
        Self {
            planes: [Plane::new(w, h), Plane::new(w / 2, h / 2), Plane::new(w / 2, h / 2)],
            poc: 0,
            motion: vec![ColMotion::default(); w4 * h.div_ceil(4)],
            w4,
        }
    }

    /// Whether this frame has the size a picture of `sps` needs.
    pub fn fits(&self, sps: &Sps) -> bool {
        self.planes[0].width == sps.width as usize && self.planes[0].height == sps.height as usize
    }

    /// The displayed part of the picture as a [`VideoFrame`] (cropped to the conformance window; `pts` still to set).
    pub fn to_video_frame(&self, sps: &Sps) -> Result<VideoFrame> {
        let (dw, dh) = sps.display_size();
        let (dw, dh) = ((dw & !1) as usize, (dh & !1) as usize);
        let (cl, ct) = (sps.crop[0] as usize, sps.crop[2] as usize);
        let ten = sps.bit_depth_luma > 8;
        let bytes = if ten { 2 } else { 1 };
        let mut planes: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut strides = [0usize; 3];
        for c in 0..3 {
            let (w, h, x0, y0) = if c == 0 { (dw, dh, cl, ct) } else { (dw / 2, dh / 2, cl / 2, ct / 2) };
            let p = &self.planes[c];
            let mut out = vec![0u8; w * h * bytes];
            for y in 0..h {
                let src = &p.data[(y0 + y) * p.stride + x0..(y0 + y) * p.stride + x0 + w];
                if ten {
                    for (x, s) in src.iter().enumerate() {
                        out[(y * w + x) * 2..(y * w + x) * 2 + 2].copy_from_slice(&s.to_le_bytes());
                    }
                } else {
                    for (x, s) in src.iter().enumerate() {
                        out[y * w + x] = *s as u8;
                    }
                }
            }
            planes[c] = out;
            strides[c] = w * bytes;
        }
        let (matrix, range) = matrix_of(sps);
        let format = format_of(sps);
        debug_assert!(matches!(format, PixelFormat::Yuv420p8 | PixelFormat::Yuv420p10));
        Ok(VideoFrame { width: dw as u32, height: dh as u32, format, matrix, range, planes, strides, pts: 0 })
    }
}
