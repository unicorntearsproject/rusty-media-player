#![deny(missing_docs)]
#![allow(unsafe_code)]
//! The system's own video decoders, for what the player does not decode itself (HEVC): VA-API on Linux (opened at run time, so the app
//! needs no VA-API package to start), VideoToolbox on macOS, Media Foundation on Windows. Each is one [`rvp_core::PlatformVideo`]; the
//! application asks it, and says plainly why when it cannot.
use rvp_core::PlatformVideo;
use std::rc::Rc;

#[cfg(windows)]
pub mod mediafoundation;
#[cfg(target_os = "linux")]
pub mod vaapi;
#[cfg(target_os = "macos")]
pub mod videotoolbox;

/// The platform decoder of this system, if it has one the player can use (none is not an error: the message says what to do).
pub fn system_video() -> Option<Rc<dyn PlatformVideo>> {
    #[cfg(target_os = "linux")]
    {
        return Some(Rc::new(vaapi::VaapiPlatform::new()));
    }
    #[cfg(target_os = "macos")]
    {
        return Some(Rc::new(videotoolbox::VideoToolboxPlatform::new()));
    }
    #[cfg(windows)]
    {
        return Some(Rc::new(mediafoundation::MediaFoundationPlatform::new()));
    }
    #[allow(unreachable_code)]
    None
}

use rvp_codec_hevc::ps::Colour;
use rvp_core::{ColorMatrix, ColorRange, Error, PixelFormat, VideoFrame};

/// The layout of a semi-planar picture in memory (NV12 for 8 bits, P010 for 10: a plane of luma, then a plane of interleaved Cb and
/// Cr), which is what every system decoder hands over.
pub(crate) struct SemiPlanar {
    /// Displayed size in luma samples (even).
    pub width: usize,
    /// Displayed size in luma samples (even).
    pub height: usize,
    /// Samples to skip at the left and the top (the conformance window).
    pub crop: (usize, usize),
    /// Offset and pitch in bytes of the luma plane.
    pub y: (usize, usize),
    /// Offset and pitch in bytes of the chroma plane.
    pub uv: (usize, usize),
    /// P010 (ten bits in the top of 16-bit words) instead of NV12.
    pub ten: bool,
    /// The stream's colour description.
    pub colour: Colour,
}

/// Semi-planar to planar 4:2:0 (`Yuv420p8`, or `Yuv420p10` with the ten bits moved down). A 10-bit PQ or HLG stream (HDR) is tone-mapped
/// to SDR and comes back as `Rgba8`.
pub(crate) fn semi_planar_frame(data: &[u8], l: SemiPlanar) -> Result<VideoFrame, Error> {
    let (w, h) = (l.width, l.height);
    let (cl, ct) = l.crop;
    let (cw, ch) = (w / 2, h / 2);
    let bytes = if l.ten { 2 } else { 1 };
    let (o0, p0) = l.y;
    let (o1, p1) = l.uv;
    if o0 + p0 * (ct + h) > data.len()
        || o1 + p1 * (ct / 2 + ch) > data.len()
        || p0 < (cl + w) * bytes
        || p1 < (cl / 2 + cw) * 2 * bytes
    {
        return Err(Error::Invalid(String::from("the decoder's picture is smaller than expected")));
    }
    let (matrix, full) = match (l.colour.matrix, l.colour.full_range) {
        (9 | 10, f) => (ColorMatrix::Bt2020, f),
        (5 | 6, f) => (ColorMatrix::Bt601, f),
        (_, f) => (ColorMatrix::Bt709, f),
    };
    let range = if full { ColorRange::Full } else { ColorRange::Limited };
    if !l.ten {
        let mut y = vec![0u8; w * h];
        let (mut u, mut v) = (vec![0u8; cw * ch], vec![0u8; cw * ch]);
        for r in 0..h {
            let src = o0 + (ct + r) * p0 + cl;
            y[r * w..(r + 1) * w].copy_from_slice(&data[src..src + w]);
        }
        for r in 0..ch {
            let src = o1 + (ct / 2 + r) * p1 + (cl / 2) * 2;
            for c in 0..cw {
                u[r * cw + c] = data[src + 2 * c];
                v[r * cw + c] = data[src + 2 * c + 1];
            }
        }
        return Ok(VideoFrame {
            width: w as u32,
            height: h as u32,
            format: PixelFormat::Yuv420p8,
            matrix,
            range,
            planes: [y, u, v],
            strides: [w, cw, cw],
            pts: 0,
        });
    }
    let rd = |off: usize| u16::from_le_bytes([data[off], data[off + 1]]) >> 6;
    let mut y = vec![0u8; w * h * 2];
    let (mut u, mut v) = (vec![0u8; cw * ch * 2], vec![0u8; cw * ch * 2]);
    for r in 0..h {
        let src = o0 + (ct + r) * p0 + cl * 2;
        for c in 0..w {
            y[(r * w + c) * 2..(r * w + c) * 2 + 2].copy_from_slice(&rd(src + c * 2).to_le_bytes());
        }
    }
    for r in 0..ch {
        let src = o1 + (ct / 2 + r) * p1 + (cl / 2) * 4;
        for c in 0..cw {
            u[(r * cw + c) * 2..(r * cw + c) * 2 + 2].copy_from_slice(&rd(src + c * 4).to_le_bytes());
            v[(r * cw + c) * 2..(r * cw + c) * 2 + 2].copy_from_slice(&rd(src + c * 4 + 2).to_le_bytes());
        }
    }
    let frame = VideoFrame {
        width: w as u32,
        height: h as u32,
        format: PixelFormat::Yuv420p10,
        matrix,
        range,
        planes: [y, u, v],
        strides: [w * 2, cw * 2, cw * 2],
        pts: 0,
    };
    if !rvp_core::hdr::is_hdr(l.colour.transfer, l.colour.matrix) {
        return Ok(frame);
    }
    let mut rgba = vec![0u8; w * h * 4];
    rvp_core::hdr::tonemap_to_rgba(&frame, l.colour.transfer, &mut rgba);
    Ok(VideoFrame {
        width: w as u32,
        height: h as u32,
        format: PixelFormat::Rgba8,
        matrix,
        range,
        planes: [rgba, Vec::new(), Vec::new()],
        strides: [w * 4, 0, 0],
        pts: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(
        w: usize,
        h: usize,
        ten: bool,
        crop: (usize, usize),
        pitch_pad: usize,
    ) -> (Vec<u8>, SemiPlanar) {
        let bytes = if ten { 2 } else { 1 };
        let (cw, ch) = (w + 2 * crop.0, h + 2 * crop.1);
        let p0 = cw * bytes + pitch_pad;
        let p1 = p0;
        let mut data = vec![0u8; p0 * ch + p1 * ch / 2];
        // Luma: x + y*3, chroma: Cb = 100 + x, Cr = 200 + x (the value is the displayed position).
        for y in 0..ch {
            for x in 0..cw {
                let v = ((x + y * 3) % 200) as u16;
                if ten {
                    let o = y * p0 + x * 2;
                    data[o..o + 2].copy_from_slice(&((v + 300) << 6).to_le_bytes());
                } else {
                    data[y * p0 + x] = v as u8;
                }
            }
        }
        for y in 0..ch / 2 {
            for x in 0..cw / 2 {
                let (cb, cr) = (100 + x as u16, 200 + x as u16 % 50);
                if ten {
                    let o = p0 * ch + y * p1 + x * 4;
                    data[o..o + 2].copy_from_slice(&((cb + 300) << 6).to_le_bytes());
                    data[o + 2..o + 4].copy_from_slice(&((cr + 300) << 6).to_le_bytes());
                } else {
                    let o = p0 * ch + y * p1 + x * 2;
                    data[o] = cb as u8;
                    data[o + 1] = cr as u8;
                }
            }
        }
        let l = SemiPlanar {
            width: w,
            height: h,
            crop,
            y: (0, p0),
            uv: (p0 * ch, p1),
            ten,
            colour: Colour::default(),
        };
        (data, l)
    }

    #[test]
    fn nv12_and_p010_become_planar_with_the_window_cut_out() {
        for ten in [false, true] {
            let (data, l) = layout(8, 6, ten, (2, 2), 6);
            let f = semi_planar_frame(&data, l).unwrap();
            assert_eq!((f.width, f.height), (8, 6));
            assert_eq!(f.format, if ten { PixelFormat::Yuv420p10 } else { PixelFormat::Yuv420p8 });
            let get = |p: usize, i: usize| {
                if ten {
                    u16::from_le_bytes([f.planes[p][i * 2], f.planes[p][i * 2 + 1]]) as usize
                } else {
                    f.planes[p][i] as usize
                }
            };
            let off = if ten { 300 } else { 0 };
            // First displayed luma sample is at (2, 2) of the decoder's picture.
            assert_eq!(get(0, 0), (2 + 2 * 3) % 200 + off);
            assert_eq!(get(0, 8 + 1), (3 + 3 * 3) % 200 + off);
            // First chroma sample is the one at (1, 1) of the decoder's chroma plane.
            assert_eq!(get(1, 0), 101 + off);
            assert_eq!(get(2, 0), 200 + 1 + off);
            assert_eq!(get(1, 4 + 3), 104 + off, "one row down, last column");
        }
    }

    #[test]
    fn a_picture_smaller_than_the_layout_says_is_an_error_not_a_crash() {
        let (data, l) = layout(8, 6, false, (0, 0), 0);
        assert!(semi_planar_frame(&data[..data.len() - 1], l).is_err());
    }
}
