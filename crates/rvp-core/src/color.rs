//! YUV 4:2:0 to RGBA conversion (BT.601 / BT.709 / BT.2020, limited or full range, 8 or 10 bit).
//!
//! Integer fixed point (16 fractional bits), chroma upsampled by replication. This is the reference
//! implementation (`scalar_rows`); the WebAssembly SIMD128 version must match it exactly (`simd_selftest`).
use crate::media::{ColorMatrix, ColorRange, PixelFormat, VideoFrame};

/// (Kr, Kb) luma coefficients of each matrix.
fn kr_kb(m: ColorMatrix) -> (f64, f64) {
    match m {
        ColorMatrix::Bt601 => (0.299, 0.114),
        ColorMatrix::Bt709 => (0.2126, 0.0722),
        ColorMatrix::Bt2020 => (0.2627, 0.0593),
    }
}

/// Fixed-point conversion constants of a frame (16 fractional bits).
#[derive(Clone, Copy, Debug)]
struct Coefs {
    bits: u32,
    y_off: i32,
    ys: i32,
    r_v: i32,
    b_u: i32,
    g_u: i32,
    g_v: i32,
}

const HALF: i32 = 1 << 15;

impl Coefs {
    fn of(frame: &VideoFrame) -> Self {
        let (kr, kb) = kr_kb(frame.matrix);
        let kg = 1.0 - kr - kb;
        let bits = match frame.format {
            PixelFormat::Yuv420p8 => 8,
            PixelFormat::Yuv420p10 => 10,
            PixelFormat::Rgba8 => 8,
        };
        let one = (1 << 16) as f64;
        // Scale to 8-bit code values first, then to full range.
        let (y_off, y_scale, c_scale) = match frame.range {
            ColorRange::Limited => (16.0, 255.0 / 219.0, 255.0 / 224.0),
            ColorRange::Full => (0.0, 1.0, 1.0),
        };
        Self {
            bits,
            y_off: y_off as i32,
            ys: (y_scale * one) as i32,
            r_v: (2.0 * (1.0 - kr) * c_scale * one) as i32,
            b_u: (2.0 * (1.0 - kb) * c_scale * one) as i32,
            g_u: (2.0 * kb * (1.0 - kb) / kg * c_scale * one) as i32,
            g_v: (2.0 * kr * (1.0 - kr) / kg * c_scale * one) as i32,
        }
    }
}

/// Convert `frame` to tightly packed RGBA8 (opaque) in `out` (`width * height * 4` bytes).
///
/// # Panics
/// If `out` is shorter than `width * height * 4`.
pub fn yuv420_to_rgba(frame: &VideoFrame, out: &mut [u8]) {
    let (w, h) = (frame.width as usize, frame.height as usize);
    assert!(out.len() >= w * h * 4, "output buffer too small");
    let out = &mut out[..w * h * 4];
    let threads = crate::par::threads();
    // Big pictures are converted in bands of rows by the pool (bands start on even rows, as chroma is shared by two).
    if threads > 1 && w * h >= 256 * 1024 {
        let rows = (h.div_ceil(threads * 2).max(16) + 1) & !1;
        crate::par::for_each_chunk_mut(out, rows * w * 4, &|i, band| {
            let y0 = i * rows;
            yuv420_rows_to_rgba(frame, band, y0, (y0 + rows).min(h));
        });
    } else {
        yuv420_rows_to_rgba(frame, out, 0, h);
    }
}

/// Convert the picture rows `y0..y1` (`y0` even) into `out`, which holds exactly those rows, tightly packed. Lets
/// several threads convert disjoint bands of one picture.
///
/// # Panics
/// If `y0` is odd, `y1` is past the picture, or `out` is not `(y1 - y0) * width * 4` bytes.
pub fn yuv420_rows_to_rgba(frame: &VideoFrame, out: &mut [u8], y0: usize, y1: usize) {
    let (w, h) = (frame.width as usize, frame.height as usize);
    assert!(y0 % 2 == 0 && y1 <= h && y0 <= y1, "bad row band");
    assert_eq!(out.len(), (y1 - y0) * w * 4, "output band has the wrong size");
    if frame.format == PixelFormat::Rgba8 {
        let s0 = frame.strides[0].max(w * 4);
        for y in y0..y1 {
            out[(y - y0) * w * 4..(y - y0 + 1) * w * 4]
                .copy_from_slice(&frame.planes[0][y * s0..y * s0 + w * 4]);
        }
        return;
    }
    let c = Coefs::of(frame);
    #[cfg(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64"))]
    if c.bits == 8 {
        return wasm::rows(frame, &c, out, y0, y1);
    }
    if c.bits == 8 {
        return rows_8(frame, &c, out, y0, y1);
    }
    scalar_rows(frame, &c, out, y0, y1, 0);
}

/// The 8-bit conversion written for the compiler: chunks of 16 pixels with fixed-size arrays, so every step is a loop over lanes it can
/// vectorize (the chroma terms are computed once per pixel pair). Same arithmetic as [`scalar_rows`], bit for bit.
fn rows_8(frame: &VideoFrame, c: &Coefs, out: &mut [u8], y0: usize, y1: usize) {
    let w = frame.width as usize;
    let (s0, s1, s2) = (frame.strides[0], frame.strides[1], frame.strides[2]);
    const N: usize = 16;
    for y in y0..y1 {
        let yrow = &frame.planes[0][y * s0..y * s0 + w];
        let cw = w.div_ceil(2);
        let urow = &frame.planes[1][(y / 2) * s1..(y / 2) * s1 + cw];
        let vrow = &frame.planes[2][(y / 2) * s2..(y / 2) * s2 + cw];
        let orow = &mut out[(y - y0) * w * 4..(y - y0 + 1) * w * 4];
        let mut x = 0;
        while x + N <= w {
            let yb: [u8; N] = yrow[x..x + N].try_into().unwrap_or([0; N]);
            let ub: [u8; N / 2] = urow[x / 2..x / 2 + N / 2].try_into().unwrap_or([0; N / 2]);
            let vb: [u8; N / 2] = vrow[x / 2..x / 2 + N / 2].try_into().unwrap_or([0; N / 2]);
            let mut rv = [0i32; N];
            let mut guv = [0i32; N];
            let mut bu = [0i32; N];
            for k in 0..N {
                let u = ub[k / 2] as i32 - 128;
                let v = vb[k / 2] as i32 - 128;
                rv[k] = c.r_v * v + HALF;
                guv[k] = HALF - c.g_u * u - c.g_v * v;
                bu[k] = c.b_u * u + HALF;
            }
            let ob = &mut orow[x * 4..(x + N) * 4];
            for k in 0..N {
                let yl = (yb[k] as i32 - c.y_off) * c.ys;
                ob[k * 4] = ((yl + rv[k]) >> 16).clamp(0, 255) as u8;
                ob[k * 4 + 1] = ((yl + guv[k]) >> 16).clamp(0, 255) as u8;
                ob[k * 4 + 2] = ((yl + bu[k]) >> 16).clamp(0, 255) as u8;
                ob[k * 4 + 3] = 255;
            }
            x += N;
        }
        // The columns that are left (the end of the row).
        if x < w {
            scalar_tail(frame, c, orow, y, x);
        }
    }
}

/// Columns `x_from..width` of row `y`, into the row `orow` (the reference arithmetic).
fn scalar_tail(frame: &VideoFrame, c: &Coefs, orow: &mut [u8], y: usize, x_from: usize) {
    let w = frame.width as usize;
    for x in x_from..w {
        let yy = frame.planes[0][y * frame.strides[0] + x] as i32;
        let u = frame.planes[1][(y / 2) * frame.strides[1] + x / 2] as i32 - 128;
        let v = frame.planes[2][(y / 2) * frame.strides[2] + x / 2] as i32 - 128;
        let yl = (yy - c.y_off) * c.ys;
        orow[x * 4] = ((yl + c.r_v * v + HALF) >> 16).clamp(0, 255) as u8;
        orow[x * 4 + 1] = ((yl - c.g_u * u - c.g_v * v + HALF) >> 16).clamp(0, 255) as u8;
        orow[x * 4 + 2] = ((yl + c.b_u * u + HALF) >> 16).clamp(0, 255) as u8;
        orow[x * 4 + 3] = 255;
    }
}

/// Reference conversion of rows `y0..y1`, starting at column `x_from` (a multiple of 2; the vector code uses this for
/// the columns it leaves over).
fn scalar_rows(frame: &VideoFrame, c: &Coefs, out: &mut [u8], y0: usize, y1: usize, x_from: usize) {
    let (w, shift) = (frame.width as usize, c.bits - 8);
    let sample = |plane: &[u8], stride: usize, x: usize, y: usize| -> i32 {
        match c.bits {
            8 => plane[y * stride + x] as i32,
            _ => {
                let i = y * stride + x * 2;
                (u16::from_le_bytes([plane[i], plane[i + 1]]) as i32) >> shift
            }
        }
    };
    for y in y0..y1 {
        for x in x_from..w {
            let yy = sample(&frame.planes[0], frame.strides[0], x, y);
            let u = sample(&frame.planes[1], frame.strides[1], x / 2, y / 2) - 128;
            let v = sample(&frame.planes[2], frame.strides[2], x / 2, y / 2) - 128;
            let yl = (yy - c.y_off) * c.ys;
            let r = (yl + c.r_v * v + HALF) >> 16;
            let g = (yl - c.g_u * u - c.g_v * v + HALF) >> 16;
            let b = (yl + c.b_u * u + HALF) >> 16;
            let o = ((y - y0) * w + x) * 4;
            out[o] = r.clamp(0, 255) as u8;
            out[o + 1] = g.clamp(0, 255) as u8;
            out[o + 2] = b.clamp(0, 255) as u8;
            out[o + 3] = 255;
        }
    }
}

/// WebAssembly SIMD128 conversion of 8-bit pictures: 16 pixels per step, two rows sharing their chroma terms.
#[cfg(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64"))]
mod wasm {
    use super::*;
    use crate::simd::*;

    /// R, G or B of four pixels: `(yl + chroma) >> 16` as i32 lanes.
    #[inline(always)]
    fn shr16(v: v128) -> v128 {
        i32x4_shr(v, 16)
    }

    /// Four luma samples (bytes `k * 4..` of `y16`'s source) as `(y - y_off) * ys`.
    #[inline(always)]
    fn luma4(y: v128, part: usize, off: v128, ys: v128) -> v128 {
        // `y` holds 16 bytes; widen the wanted quarter to i32.
        let w16 = if part < 2 { u16x8_extend_low_u8x16(y) } else { u16x8_extend_high_u8x16(y) };
        let w32 = if part % 2 == 0 { u32x4_extend_low_u16x8(w16) } else { u32x4_extend_high_u16x8(w16) };
        i32x4_mul(i32x4_sub(w32, off), ys)
    }

    /// Interleave 16 R, G, B bytes and opaque alpha into 64 bytes of RGBA.
    #[inline(always)]
    fn store_rgba(out: &mut [u8], r: v128, g: v128, b: v128) {
        let a = u8x16_splat(255);
        let rg_lo = i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(r, g);
        let rg_hi = i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(r, g);
        let ba_lo = i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(b, a);
        let ba_hi = i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(b, a);
        let px = |rg: v128, ba: v128| {
            (
                i8x16_shuffle::<0, 1, 16, 17, 2, 3, 18, 19, 4, 5, 20, 21, 6, 7, 22, 23>(rg, ba),
                i8x16_shuffle::<8, 9, 24, 25, 10, 11, 26, 27, 12, 13, 28, 29, 14, 15, 30, 31>(rg, ba),
            )
        };
        let (p0, p1) = px(rg_lo, ba_lo);
        let (p2, p3) = px(rg_hi, ba_hi);
        store(&mut out[0..16], p0);
        store(&mut out[16..32], p1);
        store(&mut out[32..48], p2);
        store(&mut out[48..64], p3);
    }

    pub(super) fn rows(frame: &VideoFrame, c: &Coefs, out: &mut [u8], y0: usize, y1: usize) {
        let w = frame.width as usize;
        let vec_w = w & !15;
        let (off, ys) = (i32x4_splat(c.y_off), i32x4_splat(c.ys));
        let (krv, kbu, kgu, kgv) =
            (i32x4_splat(c.r_v), i32x4_splat(c.b_u), i32x4_splat(c.g_u), i32x4_splat(c.g_v));
        let (half, c128) = (i32x4_splat(HALF), i16x8_splat(128));
        let [py, pu, pv] = &frame.planes;
        let [sy, su, sv] = frame.strides;
        let mut y = y0;
        while y < y1 {
            let two = y + 1 < y1;
            let cy = y / 2;
            for x in (0..vec_w).step_by(16) {
                let cx = x / 2;
                let u16s = i16x8_sub(u16x8_extend_low_u8x16(load8(&pu[cy * su + cx..])), c128);
                let v16s = i16x8_sub(u16x8_extend_low_u8x16(load8(&pv[cy * sv + cx..])), c128);
                // Chroma terms for chroma samples 0..4 and 4..8, each as i32x4.
                let mut rt = [i32x4_splat(0); 2];
                let mut gt = [i32x4_splat(0); 2];
                let mut bt = [i32x4_splat(0); 2];
                for k in 0..2 {
                    let (u, v) = if k == 0 {
                        (i32x4_extend_low_i16x8(u16s), i32x4_extend_low_i16x8(v16s))
                    } else {
                        (i32x4_extend_high_i16x8(u16s), i32x4_extend_high_i16x8(v16s))
                    };
                    rt[k] = i32x4_add(i32x4_mul(v, krv), half);
                    bt[k] = i32x4_add(i32x4_mul(u, kbu), half);
                    gt[k] = i32x4_sub(i32x4_sub(half, i32x4_mul(u, kgu)), i32x4_mul(v, kgv));
                }
                // Duplicate each chroma term over two pixels: pixel group g (4 pixels) uses chroma samples
                // 2g..2g+2 of the 8.
                let dup = |t: &[v128; 2], g: usize| -> v128 {
                    let src = t[g / 2];
                    if g % 2 == 0 {
                        i32x4_shuffle::<0, 0, 1, 1>(src, src)
                    } else {
                        i32x4_shuffle::<2, 2, 3, 3>(src, src)
                    }
                };
                let rtd = [dup(&rt, 0), dup(&rt, 1), dup(&rt, 2), dup(&rt, 3)];
                let gtd = [dup(&gt, 0), dup(&gt, 1), dup(&gt, 2), dup(&gt, 3)];
                let btd = [dup(&bt, 0), dup(&bt, 1), dup(&bt, 2), dup(&bt, 3)];
                for row in 0..(1 + two as usize) {
                    let yrow = y + row;
                    let ybytes = load(&py[yrow * sy + x..]);
                    let mut r16 = [i32x4_splat(0); 4];
                    let mut g16 = r16;
                    let mut b16 = r16;
                    for g in 0..4 {
                        let yl = luma4(ybytes, g, off, ys);
                        r16[g] = shr16(i32x4_add(yl, rtd[g]));
                        g16[g] = shr16(i32x4_add(yl, gtd[g]));
                        b16[g] = shr16(i32x4_add(yl, btd[g]));
                    }
                    let pack = |q: &[v128; 4]| -> v128 {
                        u8x16_narrow_i16x8(i16x8_narrow_i32x4(q[0], q[1]), i16x8_narrow_i32x4(q[2], q[3]))
                    };
                    let o = ((yrow - y0) * w + x) * 4;
                    store_rgba(&mut out[o..o + 64], pack(&r16), pack(&g16), pack(&b16));
                }
            }
            if vec_w < w {
                let rows_end = if two { y + 2 } else { y + 1 };
                // The leftover columns of these rows, with `out` still indexed from y0.
                scalar_rows_band(frame, c, out, y, rows_end, y0, vec_w);
            }
            y += if two { 2 } else { 1 };
        }
    }

    /// `scalar_rows` for rows `y..y_end` of a band that starts at row `band_y0`.
    fn scalar_rows_band(
        frame: &VideoFrame,
        c: &Coefs,
        out: &mut [u8],
        y: usize,
        y_end: usize,
        band_y0: usize,
        x_from: usize,
    ) {
        let w = frame.width as usize;
        let start = (y - band_y0) * w * 4;
        let end = (y_end - band_y0) * w * 4;
        scalar_rows(frame, c, &mut out[start..end], y, y_end, x_from);
    }

    /// Compare the vector conversion with the reference on pseudo-random pictures; returns the number of
    /// mismatching pictures.
    pub fn selftest() -> u32 {
        let mut bad = 0;
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for (w, h) in [(16usize, 2usize), (32, 4), (33, 5), (50, 7), (1, 1), (2, 2), (130, 9)] {
            for (m, r) in [
                (ColorMatrix::Bt601, ColorRange::Limited),
                (ColorMatrix::Bt709, ColorRange::Limited),
                (ColorMatrix::Bt2020, ColorRange::Full),
            ] {
                let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
                let mut plane = |n: usize| (0..n).map(|_| rnd() as u8).collect::<alloc::vec::Vec<u8>>();
                let frame = VideoFrame {
                    width: w as u32,
                    height: h as u32,
                    format: PixelFormat::Yuv420p8,
                    matrix: m,
                    range: r,
                    planes: [plane(w * h), plane(cw * ch), plane(cw * ch)],
                    strides: [w, cw, cw],
                    pts: 0,
                };
                let c = Coefs::of(&frame);
                let mut a = alloc::vec![0u8; w * h * 4];
                let mut b = alloc::vec![0u8; w * h * 4];
                scalar_rows(&frame, &c, &mut a, 0, h, 0);
                rows(&frame, &c, &mut b, 0, h);
                if a != b {
                    bad += 1;
                }
            }
        }
        bad
    }
}

/// Run the WebAssembly SIMD128 self-tests of this crate (0 mismatches expected). Always 0 where there is no
/// SIMD128 code.
pub fn simd_selftest() -> u32 {
    #[cfg(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64"))]
    return wasm::selftest();
    #[cfg(not(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64")))]
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simd_kernels_match_the_scalar_reference() {
        assert_eq!(simd_selftest(), 0);
    }
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

    /// The chunked 8-bit path gives exactly the bytes of the reference loop, for odd and even sizes, strides wider than the picture, every
    /// matrix and both ranges.
    #[test]
    fn the_fast_8_bit_path_is_the_reference_bit_for_bit() {
        let mut seed = 0x2545_f491u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as u8
        };
        for (w, h) in [(1usize, 2usize), (2, 2), (15, 4), (16, 4), (17, 6), (33, 8), (64, 2), (130, 10)] {
            for m in [ColorMatrix::Bt601, ColorMatrix::Bt709, ColorMatrix::Bt2020] {
                for r in [ColorRange::Limited, ColorRange::Full] {
                    let (s0, s1) = (w + 3, w.div_ceil(2) + 5);
                    let f = VideoFrame {
                        width: w as u32,
                        height: h as u32,
                        format: PixelFormat::Yuv420p8,
                        matrix: m,
                        range: r,
                        planes: [
                            (0..s0 * h).map(|_| rnd()).collect(),
                            (0..s1 * h.div_ceil(2)).map(|_| rnd()).collect(),
                            (0..s1 * h.div_ceil(2)).map(|_| rnd()).collect(),
                        ],
                        strides: [s0, s1, s1],
                        pts: 0,
                    };
                    let c = Coefs::of(&f);
                    let (mut fast, mut slow) = (vec![0u8; w * h * 4], vec![0u8; w * h * 4]);
                    rows_8(&f, &c, &mut fast, 0, h);
                    scalar_rows(&f, &c, &mut slow, 0, h, 0);
                    assert_eq!(fast, slow, "{w}x{h} {m:?} {r:?}");
                    // A band of rows.
                    if h >= 4 {
                        let (mut a, mut b) = (vec![0u8; w * 2 * 4], vec![0u8; w * 2 * 4]);
                        rows_8(&f, &c, &mut a, 2, 4);
                        scalar_rows(&f, &c, &mut b, 2, 4, 0);
                        assert_eq!(a, b, "{w}x{h} band");
                    }
                }
            }
        }
    }
}
