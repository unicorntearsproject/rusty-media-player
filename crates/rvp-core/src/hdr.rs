//! HDR to SDR: a 10-bit picture in the PQ (SMPTE ST 2084) or HLG (ARIB STD-B67) transfer, BT.2020 colours, mapped to an 8-bit sRGB-like
//! picture that looks right on an ordinary screen. Used for what platform decoders hand over (HEVC HDR10 and HLG streams).
//!
//! The steps per pixel: Y'CbCr to R'G'B' (BT.2020), the transfer function to light, a tone curve on the luminance (reference white 203 nits
//! lands at 1.0; highlights up to the stream's peak roll off smoothly instead of clipping), BT.2020 to BT.709 primaries, the sRGB curve.
use crate::media::{ColorRange, PixelFormat, VideoFrame};
use alloc::vec::Vec;

/// The `transfer_characteristics` value of PQ.
pub const TRANSFER_PQ: u8 = 16;
/// The `transfer_characteristics` value of HLG.
pub const TRANSFER_HLG: u8 = 18;

/// Whether a stream with this transfer function and matrix needs the mapping (BT.2020 matrix with PQ or HLG).
pub fn is_hdr(transfer: u8, matrix: u8) -> bool {
    matches!(transfer, TRANSFER_PQ | TRANSFER_HLG) && matches!(matrix, 9 | 10)
}

/// Reference white of SDR in nits (ITU-R BT.2408).
const REF_WHITE: f32 = 203.0;
/// What the tone curve treats as the brightest content, in nits.
const PEAK: f32 = 1000.0;

fn pq_eotf(n: f32) -> f32 {
    // Returns nits.
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 4096.0 * 128.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 4096.0 * 32.0;
    const C3: f32 = 2392.0 / 4096.0 * 32.0;
    let p = libm::powf(n.clamp(0.0, 1.0), 1.0 / M2);
    let num = (p - C1).max(0.0);
    let den = C2 - C3 * p;
    10000.0 * libm::powf(num / den, 1.0 / M1)
}

/// The HLG inverse OETF: signal to normalised scene light (0 to 1).
fn hlg_inverse_oetf(e: f32) -> f32 {
    const A: f32 = 0.178_832_77;
    const B: f32 = 0.284_668_92;
    const C: f32 = 0.559_910_7;
    let e = e.clamp(0.0, 1.0);
    if e <= 0.5 { e * e / 3.0 } else { (libm::expf((e - C) / A) + B) / 12.0 }
}

fn srgb_oetf(l: f32) -> f32 {
    let l = l.clamp(0.0, 1.0);
    if l <= 0.003_130_8 { 12.92 * l } else { 1.055 * libm::powf(l, 1.0 / 2.4) - 0.055 }
}

/// Everything per-pixel that does not depend on the pixel: lookup tables for the transfer functions.
struct Tables {
    /// Code value (0..1024) to light in units of reference white, for PQ; for HLG to scene light (0..1).
    light: Vec<f32>,
    /// Linear light 0..1 in 4096 steps to an 8-bit sRGB value.
    srgb: Vec<u8>,
    hlg: bool,
}

impl Tables {
    fn new(transfer: u8) -> Self {
        let hlg = transfer == TRANSFER_HLG;
        let light = (0..1024)
            .map(|i| {
                let n = i as f32 / 1023.0;
                if hlg { hlg_inverse_oetf(n) } else { pq_eotf(n) / REF_WHITE }
            })
            .collect();
        let srgb = (0..=4095).map(|i| (srgb_oetf(i as f32 / 4095.0) * 255.0 + 0.5) as u8).collect();
        Self { light, srgb, hlg }
    }
}

/// The tone curve on luminance `l` (1.0 is reference white): untouched up to 0.7, then a smooth shoulder that reaches 1.0 only at the
/// stream's peak, so reference white ends near the top of the SDR range and brighter highlights keep their shape.
fn tone(l: f32) -> f32 {
    const KNEE: f32 = 0.7;
    if l <= KNEE { l } else { KNEE + (1.0 - KNEE) * (1.0 - libm::expf(-(l - KNEE) / (1.0 - KNEE))) }
}

fn map_pixel(t: &Tables, y: u16, cb: u16, cr: u16, limited: bool) -> [u8; 3] {
    let (yn, cbn, crn) = if limited {
        ((y as f32 - 64.0) / 876.0, (cb as f32 - 512.0) / 896.0, (cr as f32 - 512.0) / 896.0)
    } else {
        (y as f32 / 1023.0, (cb as f32 - 512.0) / 1023.0, (cr as f32 - 512.0) / 1023.0)
    };
    // BT.2020 non-constant luminance.
    let r = yn + 1.4746 * crn;
    let b = yn + 1.8814 * cbn;
    let g = (yn - 0.2627 * r - 0.0593 * b) / 0.6780;
    let code = |v: f32| ((v.clamp(0.0, 1.0) * 1023.0 + 0.5) as usize).min(1023);
    let mut lin = [t.light[code(r)], t.light[code(g)], t.light[code(b)]];
    if t.hlg {
        // The HLG system gamma: display light follows the scene luminance to the power 0.2, scaled to a 1000-nit display, in units of
        // reference white.
        let ys = 0.2627 * lin[0] + 0.6780 * lin[1] + 0.0593 * lin[2];
        let k = if ys > 0.0 { libm::powf(ys, 0.2) * (PEAK / REF_WHITE) } else { 0.0 };
        lin = [lin[0] * k, lin[1] * k, lin[2] * k];
    }
    let l = 0.2627 * lin[0] + 0.6780 * lin[1] + 0.0593 * lin[2];
    let s = if l > 1e-6 { tone(l) / l } else { 1.0 };
    let (r, g, b) = (lin[0] * s, lin[1] * s, lin[2] * s);
    // BT.2020 to BT.709 primaries.
    let r7 = 1.6605 * r - 0.5876 * g - 0.0728 * b;
    let g7 = -0.1246 * r + 1.1329 * g - 0.0083 * b;
    let b7 = -0.0182 * r - 0.1006 * g + 1.1187 * b;
    let q = |v: f32| t.srgb[((v.clamp(0.0, 1.0) * 4095.0 + 0.5) as usize).min(4095)];
    [q(r7), q(g7), q(b7)]
}

/// Map a 10-bit 4:2:0 frame (`Yuv420p10`) with the given transfer function to packed RGBA in `out` (`width * height * 4` bytes).
///
/// # Panics
/// If the frame is not `Yuv420p10` or `out` is too short.
pub fn tonemap_to_rgba(frame: &VideoFrame, transfer: u8, out: &mut [u8]) {
    assert_eq!(frame.format, PixelFormat::Yuv420p10, "tone mapping takes 10-bit frames");
    let (w, h) = (frame.width as usize, frame.height as usize);
    assert!(out.len() >= w * h * 4);
    let t = Tables::new(transfer);
    let limited = frame.range == ColorRange::Limited;
    let rd = |plane: &[u8], stride: usize, row: usize, col: usize| -> u16 {
        let o = row * stride + col * 2;
        u16::from_le_bytes([plane[o], plane[o + 1]]) & 0x3ff
    };
    let rows_per_band = 16;
    crate::par::for_each_chunk_mut(&mut out[..w * h * 4], rows_per_band * w * 4, &|band, chunk| {
        let y0 = band * rows_per_band;
        for (ry, row) in chunk.chunks_exact_mut(w * 4).enumerate() {
            let y = y0 + ry;
            for x in 0..w {
                let yy = rd(&frame.planes[0], frame.strides[0], y, x);
                let cb = rd(&frame.planes[1], frame.strides[1], y / 2, x / 2);
                let cr = rd(&frame.planes[2], frame.strides[2], y / 2, x / 2);
                let [r, g, b] = map_pixel(&t, yy, cb, cr, limited);
                row[x * 4..x * 4 + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::ColorMatrix;
    use alloc::vec;

    fn flat(y: u16, cb: u16, cr: u16) -> VideoFrame {
        let plane = |v: u16, n: usize| (0..n).flat_map(|_| v.to_le_bytes()).collect::<Vec<u8>>();
        VideoFrame {
            width: 4,
            height: 2,
            format: PixelFormat::Yuv420p10,
            matrix: ColorMatrix::Bt2020,
            range: ColorRange::Limited,
            planes: [plane(y, 8), plane(cb, 2), plane(cr, 2)],
            strides: [8, 4, 4],
            pts: 0,
        }
    }

    fn rgb(f: &VideoFrame, transfer: u8) -> [u8; 3] {
        let mut out = vec![0u8; 4 * 2 * 4];
        tonemap_to_rgba(f, transfer, &mut out);
        [out[0], out[1], out[2]]
    }

    #[test]
    fn pq_reference_white_is_white_and_black_is_black() {
        // PQ code for 203 nits is about 0.5807 -> 10-bit limited luma 64 + 0.5807*876 = 573.
        let white = rgb(&flat(573, 512, 512), TRANSFER_PQ);
        assert!(
            white.iter().all(|c| (225..=255).contains(c)),
            "{white:?}: reference white is near the top, a little below for the roll-off"
        );
        let black = rgb(&flat(64, 512, 512), TRANSFER_PQ);
        assert_eq!(black, [0, 0, 0]);
        // Brighter than reference white is brighter, never darker, and never wraps.
        let mut last = 0;
        for y in (64..=940).step_by(32) {
            let v = rgb(&flat(y, 512, 512), TRANSFER_PQ)[1];
            assert!(v >= last, "monotonic at {y}: {v} < {last}");
            last = v;
        }
        assert_eq!(last, 255, "the peak clips to white");
    }

    #[test]
    fn hlg_mid_signal_is_a_mid_grey_and_colours_keep_their_hue() {
        let mid = rgb(&flat(64 + 438, 512, 512), TRANSFER_HLG);
        assert!(mid[0] == mid[1] && mid[1] == mid[2], "grey stays grey: {mid:?}");
        assert!((90..=230).contains(&mid[0]), "{mid:?}");
        // A red-ish colour: red channel dominates after the mapping.
        let red = rgb(&flat(300, 400, 800), TRANSFER_PQ);
        assert!(red[0] > red[1] && red[0] > red[2], "{red:?}");
    }

    #[test]
    fn only_bt2020_pq_and_hlg_need_it() {
        assert!(is_hdr(16, 9) && is_hdr(18, 9) && is_hdr(16, 10));
        assert!(!is_hdr(1, 1) && !is_hdr(16, 1) && !is_hdr(2, 2));
    }
}
