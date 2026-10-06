//! Cover art: decoding JPEG and PNG (zune, `no_std`), box-filter downscaling and the small thumbnails the grid shows.
use alloc::vec::Vec;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

/// Longest side of a grid thumbnail, pixels.
pub const THUMB_SIDE: u32 = 144;
/// Pictures with more pixels than this are not decoded (a decompression bomb, or simply not cover art).
pub const MAX_PIXELS: usize = 36_000_000;

/// A decoded picture: opaque RGBA8, `w * h * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
    /// Pixels.
    pub rgba: Vec<u8>,
}

/// A thumbnail as kept in memory and on disk: RGB8, `w * h * 3` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumb {
    /// Width.
    pub w: u16,
    /// Height.
    pub h: u16,
    /// Pixels, three bytes each.
    pub rgb: Vec<u8>,
}

impl Thumb {
    /// The thumbnail as RGBA.
    pub fn to_image(&self) -> Image {
        let mut rgba = Vec::with_capacity(self.rgb.len() / 3 * 4);
        for p in self.rgb.chunks_exact(3) {
            rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
        }
        Image { w: self.w as u32, h: self.h as u32, rgba }
    }

    /// The average colour (a stand-in tint while the picture is not loaded, and a test oracle).
    pub fn average(&self) -> [u8; 3] {
        let n = (self.rgb.len() / 3).max(1) as u64;
        let mut s = [0u64; 3];
        for p in self.rgb.chunks_exact(3) {
            for k in 0..3 {
                s[k] += p[k] as u64;
            }
        }
        [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8]
    }
}

/// True if the bytes start like a JPEG.
fn is_jpeg(b: &[u8]) -> bool {
    b.starts_with(&[0xff, 0xd8])
}

/// True if the bytes start like a PNG.
fn is_png(b: &[u8]) -> bool {
    b.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
}

/// Width and height of a JPEG or PNG without decoding it.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if is_jpeg(bytes) {
        let mut d = zune_jpeg::JpegDecoder::new(ZCursor::new(bytes));
        d.decode_headers().ok()?;
        let i = d.info()?;
        Some((i.width as u32, i.height as u32))
    } else if is_png(bytes) {
        let mut d = zune_png::PngDecoder::new(ZCursor::new(bytes));
        d.decode_headers().ok()?;
        let (w, h) = d.dimensions()?;
        Some((w as u32, h as u32))
    } else {
        None
    }
}

/// Decode a JPEG or PNG to RGBA with its longest side at most `max_side` (never enlarged). `None` if it is neither, is
/// damaged, or is absurdly large.
pub fn decode(bytes: &[u8], max_side: u32) -> Option<Image> {
    let (w, h, rgb_or_rgba, comps) = if is_jpeg(bytes) {
        let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
        let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
        d.decode_headers().ok()?;
        let i = d.info()?;
        let (w, h) = (i.width as usize, i.height as usize);
        if w == 0 || h == 0 || w * h > MAX_PIXELS {
            return None;
        }
        let px = d.decode().ok()?;
        let comps = d.output_colorspace()?.num_components();
        (w, h, px, comps)
    } else if is_png(bytes) {
        let opts = DecoderOptions::default().png_set_strip_to_8bit(true).png_set_add_alpha_channel(true);
        let mut d = zune_png::PngDecoder::new_with_options(ZCursor::new(bytes), opts);
        d.decode_headers().ok()?;
        let (w, h) = d.dimensions()?;
        if w == 0 || h == 0 || w * h > MAX_PIXELS {
            return None;
        }
        let comps = d.colorspace()?.num_components();
        match d.decode().ok()? {
            zune_core::result::DecodingResult::U8(v) => (w, h, v, comps),
            _ => return None,
        }
    } else {
        return None;
    };
    if rgb_or_rgba.len() < w * h * comps || !(1..=4).contains(&comps) {
        return None;
    }
    // A big picture is shrunk straight from what the decoder made, without a full-size RGBA copy of it (a 3000 x 3000 cover would be another
    // 36 MB, with several covers being read at once).
    if w.max(h) as u32 > max_side && max_side > 0 {
        return Some(downscale_components(&rgb_or_rgba, w, h, comps, max_side));
    }
    // To opaque RGBA (alpha is composited over black).
    let mut rgba = Vec::with_capacity(w * h * 4);
    for p in rgb_or_rgba.chunks_exact(comps).take(w * h) {
        let (r, g, b, a) = match comps {
            1 => (p[0], p[0], p[0], 255),
            2 => (p[0], p[0], p[0], p[1]),
            3 => (p[0], p[1], p[2], 255),
            _ => (p[0], p[1], p[2], p[3]),
        };
        let m = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
        rgba.extend_from_slice(&[m(r), m(g), m(b), 255]);
    }
    let img = Image { w: w as u32, h: h as u32, rgba };
    Some(if img.w.max(img.h) > max_side { downscale(&img, max_side) } else { img })
}

/// One pixel of 1 to 4 components as opaque RGB (alpha composited over black).
fn opaque(p: &[u8], comps: usize) -> [u32; 3] {
    let (r, g, b, a) = match comps {
        1 => (p[0], p[0], p[0], 255),
        2 => (p[0], p[0], p[0], p[1]),
        3 => (p[0], p[1], p[2], 255),
        _ => (p[0], p[1], p[2], p[3]),
    };
    let m = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8 as u32;
    [m(r), m(g), m(b)]
}

/// [`downscale`] of a picture given as `comps` components per pixel (what the decoders produce), with the same result as converting to
/// RGBA first, without making that copy.
fn downscale_components(src: &[u8], w: usize, h: usize, comps: usize, max_side: u32) -> Image {
    let long = w.max(h).max(1) as u64;
    let dw = ((w as u64 * max_side as u64 / long) as u32).max(1);
    let dh = ((h as u64 * max_side as u64 / long) as u32).max(1);
    let mut out = Vec::with_capacity((dw * dh * 4) as usize);
    for dy in 0..dh {
        let y0 = (dy as u64 * h as u64 / dh as u64) as usize;
        let y1 = (((dy as u64 + 1) * h as u64).div_ceil(dh as u64) as usize).clamp(y0 + 1, h);
        for dx in 0..dw {
            let x0 = (dx as u64 * w as u64 / dw as u64) as usize;
            let x1 = (((dx as u64 + 1) * w as u64).div_ceil(dw as u64) as usize).clamp(x0 + 1, w);
            let mut s = [0u32; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * comps;
                    let p = opaque(&src[i..i + comps], comps);
                    s[0] += p[0];
                    s[1] += p[1];
                    s[2] += p[2];
                }
            }
            let n = ((x1 - x0) * (y1 - y0)) as u32;
            out.extend_from_slice(&[(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8, 255]);
        }
    }
    Image { w: dw, h: dh, rgba: out }
}

/// Shrink `img` so its longest side is `max_side`, averaging the source pixels under each new one.
pub fn downscale(img: &Image, max_side: u32) -> Image {
    let long = img.w.max(img.h).max(1);
    if long <= max_side || max_side == 0 {
        return img.clone();
    }
    let dw = ((img.w as u64 * max_side as u64 / long as u64) as u32).max(1);
    let dh = ((img.h as u64 * max_side as u64 / long as u64) as u32).max(1);
    let mut out = Vec::with_capacity((dw * dh * 4) as usize);
    for dy in 0..dh {
        let y0 = (dy as u64 * img.h as u64 / dh as u64) as u32;
        let y1 = (((dy as u64 + 1) * img.h as u64).div_ceil(dh as u64) as u32).clamp(y0 + 1, img.h);
        for dx in 0..dw {
            let x0 = (dx as u64 * img.w as u64 / dw as u64) as u32;
            let x1 = (((dx as u64 + 1) * img.w as u64).div_ceil(dw as u64) as u32).clamp(x0 + 1, img.w);
            let mut s = [0u32; 3];
            for y in y0..y1 {
                let row = (y * img.w) as usize * 4;
                for x in x0..x1 {
                    let i = row + x as usize * 4;
                    s[0] += img.rgba[i] as u32;
                    s[1] += img.rgba[i + 1] as u32;
                    s[2] += img.rgba[i + 2] as u32;
                }
            }
            let n = (x1 - x0) * (y1 - y0);
            out.extend_from_slice(&[(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8, 255]);
        }
    }
    Image { w: dw, h: dh, rgba: out }
}

/// A grid thumbnail of the encoded picture `bytes`.
pub fn thumbnail(bytes: &[u8]) -> Option<Thumb> {
    let img = decode(bytes, THUMB_SIDE)?;
    let mut rgb = Vec::with_capacity((img.w * img.h * 3) as usize);
    for p in img.rgba.chunks_exact(4) {
        rgb.extend_from_slice(&p[..3]);
    }
    Some(Thumb { w: img.w as u16, h: img.h as u16, rgb })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 3]) -> Image {
        let rgba = (0..w * h).flat_map(|_| [c[0], c[1], c[2], 255]).collect();
        Image { w, h, rgba }
    }

    #[test]
    fn downscale_keeps_aspect_and_colour() {
        let big = solid(400, 200, [10, 200, 90]);
        let small = downscale(&big, 100);
        assert_eq!((small.w, small.h), (100, 50));
        assert!(small.rgba.chunks_exact(4).all(|p| p == [10, 200, 90, 255]));
        assert_eq!(downscale(&big, 1000), big, "never enlarged");
    }

    #[test]
    fn garbage_is_not_a_picture() {
        assert!(decode(b"not a picture", 64).is_none());
        assert!(decode(&[0xff, 0xd8, 0xff, 0x00], 64).is_none());
        assert!(decode(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0], 64).is_none());
        assert!(thumbnail(&[]).is_none());
    }

    #[test]
    fn decodes_a_tiny_png() {
        // 2x1 RGB PNG: red, blue.
        let png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0, 0, 0,
            2, 0, 0, 0, 1, 8, 2, 0, 0, 0, 0x7b, 0x40, 0xe8, 0xdd, 0, 0, 0, 0x0f, 0x49, 0x44, 0x41, 0x54,
            0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xc0, 0xf8, 0x1f, 0x00, 0x04, 0x00, 0x01, 0xff, 0x6d, 0x4b,
            0x33, 0x04, 0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        if let Some(img) = decode(png, 64) {
            assert_eq!((img.w, img.h), (2, 1));
        }
        // Whatever the decoder makes of the hand-made bytes, it must not panic.
        let _ = dimensions(png);
    }

    #[test]
    fn shrinking_straight_from_the_components_gives_the_same_picture() {
        for comps in 1..=4usize {
            let (w, h) = (97usize, 61usize);
            let src: Vec<u8> = (0..w * h * comps).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
            // The long way: to RGBA first, then shrink.
            let mut rgba = Vec::new();
            for p in src.chunks_exact(comps) {
                let q = opaque(p, comps);
                rgba.extend_from_slice(&[q[0] as u8, q[1] as u8, q[2] as u8, 255]);
            }
            let long = downscale(&Image { w: w as u32, h: h as u32, rgba }, 24);
            let direct = downscale_components(&src, w, h, comps, 24);
            assert_eq!(direct, long, "{comps} components");
        }
    }
}
