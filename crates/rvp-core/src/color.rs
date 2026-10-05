//! YUV 4:2:0 to RGBA conversion (BT.601 / BT.709 / BT.2020, limited or full range, 8 or 10 bit).
//!
//! Integer fixed point (16 fractional bits), chroma upsampled by replication. This is the reference
//! implementation; SIMD versions come in M9 and must match it.
use crate::media::{ColorMatrix, ColorRange, PixelFormat, VideoFrame};

/// (Kr, Kb) luma coefficients of each matrix.
fn kr_kb(m: ColorMatrix) -> (f64, f64) {
    match m {
        ColorMatrix::Bt601 => (0.299, 0.114),
        ColorMatrix::Bt709 => (0.2126, 0.0722),
        ColorMatrix::Bt2020 => (0.2627, 0.0593),
    }
}

/// Convert `frame` to tightly packed RGBA8 (opaque) in `out` (`width * height * 4` bytes).
///
/// # Panics
/// If `out` is shorter than `width * height * 4`.
pub fn yuv420_to_rgba(frame: &VideoFrame, out: &mut [u8]) {
    let (w, h) = (frame.width as usize, frame.height as usize);
    assert!(out.len() >= w * h * 4, "output buffer too small");
    let (kr, kb) = kr_kb(frame.matrix);
    let kg = 1.0 - kr - kb;
    let bits = match frame.format {
        PixelFormat::Yuv420p8 => 8,
        PixelFormat::Yuv420p10 => 10,
    };
    let shift = bits - 8;
    let one = (1 << 16) as f64;
    // Scale to 8-bit code values first, then to full range.
    let (y_off, y_scale, c_scale) = match frame.range {
        ColorRange::Limited => (16.0, 255.0 / 219.0, 255.0 / 224.0),
        ColorRange::Full => (0.0, 1.0, 1.0),
    };
    let ys = (y_scale * one) as i32;
    let r_v = (2.0 * (1.0 - kr) * c_scale * one) as i32;
    let b_u = (2.0 * (1.0 - kb) * c_scale * one) as i32;
    let g_u = (2.0 * kb * (1.0 - kb) / kg * c_scale * one) as i32;
    let g_v = (2.0 * kr * (1.0 - kr) / kg * c_scale * one) as i32;
    let half = 1 << 15;
    let y_off = y_off as i32;

    let sample = |plane: &[u8], stride: usize, x: usize, y: usize| -> i32 {
        match bits {
            8 => plane[y * stride + x] as i32,
            _ => {
                let i = y * stride + x * 2;
                (u16::from_le_bytes([plane[i], plane[i + 1]]) as i32) >> shift
            }
        }
    };
    for y in 0..h {
        for x in 0..w {
            let yy = sample(&frame.planes[0], frame.strides[0], x, y);
            let u = sample(&frame.planes[1], frame.strides[1], x / 2, y / 2) - 128;
            let v = sample(&frame.planes[2], frame.strides[2], x / 2, y / 2) - 128;
            let yl = (yy - y_off) * ys;
            let r = (yl + r_v * v + half) >> 16;
            let g = (yl - g_u * u - g_v * v + half) >> 16;
            let b = (yl + b_u * u + half) >> 16;
            let o = (y * w + x) * 4;
            out[o] = r.clamp(0, 255) as u8;
            out[o + 1] = g.clamp(0, 255) as u8;
            out[o + 2] = b.clamp(0, 255) as u8;
            out[o + 3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn solid(y: u8, u: u8, v: u8, m: ColorMatrix, r: ColorRange) -> VideoFrame {
        VideoFrame {
            width: 4,
            height: 2,
            format: PixelFormat::Yuv420p8,
            matrix: m,
            range: r,
            planes: [vec![y; 8], vec![u; 2], vec![v; 2]],
            strides: [4, 2, 2],
            pts: 0,
        }
    }

    fn px(f: &VideoFrame) -> [u8; 4] {
        let mut out = vec![0u8; 32];
        yuv420_to_rgba(f, &mut out);
        out[..4].try_into().unwrap()
    }

    #[test]
    fn limited_range_black_white_grey() {
        let l = ColorRange::Limited;
        assert_eq!(px(&solid(16, 128, 128, ColorMatrix::Bt709, l)), [0, 0, 0, 255]);
        assert_eq!(px(&solid(235, 128, 128, ColorMatrix::Bt709, l)), [255, 255, 255, 255]);
        let g = px(&solid(126, 128, 128, ColorMatrix::Bt601, l));
        assert!(g[0] == g[1] && g[1] == g[2] && (g[0] as i32 - 128).abs() <= 1, "{g:?}");
    }

    #[test]
    fn primaries_bt709_and_bt601() {
        // BT.709 limited red: Y=63, Cb=102, Cr=240.
        let r = px(&solid(63, 102, 240, ColorMatrix::Bt709, ColorRange::Limited));
        assert!(r[0] >= 253 && r[1] <= 2 && r[2] <= 2, "{r:?}");
        // BT.601 limited blue: Y=41, Cb=240, Cr=110.
        let b = px(&solid(41, 240, 110, ColorMatrix::Bt601, ColorRange::Limited));
        assert!(b[2] >= 253 && b[0] <= 2 && b[1] <= 2, "{b:?}");
        // The same code values mean different colours under different matrices.
        assert_ne!(
            px(&solid(63, 102, 240, ColorMatrix::Bt709, ColorRange::Limited)),
            px(&solid(63, 102, 240, ColorMatrix::Bt601, ColorRange::Limited))
        );
    }

    #[test]
    fn full_range_and_ten_bit_agree_with_eight_bit() {
        assert_eq!(px(&solid(0, 128, 128, ColorMatrix::Bt709, ColorRange::Full)), [0, 0, 0, 255]);
        assert_eq!(px(&solid(255, 128, 128, ColorMatrix::Bt709, ColorRange::Full)), [255, 255, 255, 255]);
        let f8 = solid(63, 102, 240, ColorMatrix::Bt709, ColorRange::Limited);
        let le = |v: u8| -> Vec<u8> { (0..8).flat_map(|_| ((v as u16) << 2).to_le_bytes()).collect() };
        let f10 = VideoFrame {
            format: PixelFormat::Yuv420p10,
            planes: [le(63), le(102)[..4].to_vec(), le(240)[..4].to_vec()],
            strides: [8, 4, 4],
            ..f8.clone()
        };
        assert_eq!(px(&f8), px(&f10));
    }

    #[test]
    fn chroma_is_replicated_over_2x2() {
        let mut f = solid(126, 128, 128, ColorMatrix::Bt709, ColorRange::Limited);
        f.planes[2] = vec![128, 240]; // second chroma column is red-ish
        let mut out = vec![0u8; 32];
        yuv420_to_rgba(&f, &mut out);
        assert_eq!(out[0..4], out[4..8]); // x=0,1 share chroma
        assert_ne!(out[0..4], out[8..12]); // x=2 differs
        assert_eq!(out[8..12], out[12..16]);
    }
}
