//! VP9 decoding behind [`rvp_core::VideoDecoder`], built on `rusty_vp9` (Apache-2.0, pure Rust, bit-exact against
//! the libvpx conformance vectors; chosen over `vp9dec` in M7, see `docs/PLAN.md`).
//!
//! What this wrapper adds on top of the crate:
//!
//! * the packet loop that `rusty_vp9` leaves to the caller (a superframe with a hidden alt-ref plus a shown frame is
//!   one packet that must be pulled twice);
//! * output in our [`VideoFrame`] (profile 0 8-bit 4:2:0, and profile 2 10-bit 4:2:0; other chroma layouts and
//!   12-bit are rejected with `Unsupported`), with the colour matrix and range read from the key frame header, which
//!   `rusty_vp9` does not expose;
//! * safety for untrusted streams: frames are refused until a key frame has been decoded, sizes are capped
//!   ([`MAX_PIXELS`]), and after any error the decoder starts over at the next key frame. `rusty_vp9` contains
//!   malformed-input panics with `catch_unwind`, which cannot work on `wasm32` (panic = abort), so everything that
//!   can be checked up front is checked here.
//!
//! Resolution changes at a key frame work: every [`VideoFrame`] carries its own size.
#![forbid(unsafe_code)]

/// Run the WebAssembly SIMD128 self-tests of the vendored decoder (0 mismatches expected; always 0 without SIMD128).
pub fn simd_selftest() -> u32 {
    rusty_vp9::simd_selftest()
}

use rusty_vp9::{Error as VpError, Vp9Decoder};
use rvp_core::{
    ColorMatrix, ColorRange, Error, Packet, PixelFormat, Result, StreamInfo, StreamKind, VideoCodec,
    VideoDecoder, VideoFrame,
};
use std::collections::VecDeque;

/// The codec this crate decodes.
pub const CODEC: VideoCodec = VideoCodec::Vp9;

/// Largest picture accepted (width times height): 8192 x 4352, a bit more than 8K at 16:9 would need, so 4K is
/// well inside and a hostile 65536 x 65536 header cannot make the decoder allocate gigabytes.
pub const MAX_PIXELS: u64 = 8192 * 4352;

/// Create a decoder for a VP9 stream (`info.codec == "vp9"`).
pub fn vp9_decoder(info: &StreamInfo) -> Result<Box<dyn VideoDecoder>> {
    if info.kind != StreamKind::Video || info.codec != "vp9" {
        return Err(Error::Unsupported(format!("not a VP9 stream: {}", info.codec)));
    }
    Ok(Box::new(Vp9VideoDecoder::new()))
}

/// [`VideoDecoder`] adapter around [`rusty_vp9::Vp9Decoder`].
pub struct Vp9VideoDecoder {
    dec: Vp9Decoder,
    out: VecDeque<VideoFrame>,
    /// A key frame has been decoded since the last reset, so inter frames have references.
    have_key: bool,
    matrix: ColorMatrix,
    range: ColorRange,
    /// The colour space was signalled as "unknown": pick the matrix by size.
    matrix_unknown: bool,
}

impl Default for Vp9VideoDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Vp9VideoDecoder {
    /// A decoder waiting for its first key frame.
    pub fn new() -> Self {
        Self {
            dec: Vp9Decoder::new(),
            out: VecDeque::new(),
            have_key: false,
            matrix: ColorMatrix::Bt601,
            range: ColorRange::Limited,
            matrix_unknown: true,
        }
    }

    fn reset(&mut self) {
        self.dec = Vp9Decoder::new();
        self.have_key = false;
    }

    /// Decode the frames of one packet, returning the first error after resetting the decoder.
    fn decode(&mut self, packet: &Packet) -> Result<()> {
        let frames = split_superframe(&packet.data);
        // Check every coded frame first, so a bad one never reaches the decoder (a panic aborts on wasm).
        let mut have_key = self.have_key;
        for f in &frames {
            let h = peek_header(f)?;
            if h.unsupported_profile {
                return Err(Error::Unsupported(
                    "VP9 profiles 1 and 3 (4:2:2, 4:4:0, 4:4:4) and 12-bit".into(),
                ));
            }
            if let Some((w, hgt)) = h.size {
                if w as u64 * hgt as u64 > MAX_PIXELS {
                    return Err(Error::Unsupported(format!("VP9 picture of {w}x{hgt}")));
                }
            }
            if h.show_existing || !h.key_frame {
                if !have_key {
                    return Err(Error::Invalid("VP9 inter frame before a key frame".into()));
                }
            } else {
                have_key = true;
            }
        }
        // Colour config comes from key frames.
        for f in &frames {
            if let Ok(h) = peek_header(f) {
                if let Some(c) = h.color {
                    (self.matrix, self.matrix_unknown) = match c.space {
                        1 | 3 => (ColorMatrix::Bt601, false),
                        2 => (ColorMatrix::Bt709, false),
                        5 => (ColorMatrix::Bt2020, false),
                        _ => (ColorMatrix::Bt601, true),
                    };
                    self.range = if c.full_range { ColorRange::Full } else { ColorRange::Limited };
                }
            }
        }
        self.have_key = have_key;
        if let Err(e) = self.dec.push(&packet.data, Some(packet.pts)) {
            self.reset();
            return Err(map_err(e));
        }
        for _ in 0..frames.len() {
            match self.dec.next_frame() {
                Ok(f) => match self.convert(f, packet.pts) {
                    Ok(v) => self.out.push_back(v),
                    Err(e) => {
                        self.reset();
                        return Err(e);
                    }
                },
                Err(VpError::Again | VpError::Eof) => {}
                Err(e) => {
                    self.reset();
                    return Err(map_err(e));
                }
            }
        }
        Ok(())
    }

    fn convert(&self, f: rusty_vp9::DecodedFrame, fallback_pts: i64) -> Result<VideoFrame> {
        if f.subsampling_x != 1 || f.subsampling_y != 1 || f.planes.len() != 3 {
            return Err(Error::Unsupported("VP9 chroma layout other than 4:2:0".into()));
        }
        let format = match f.bit_depth {
            8 => PixelFormat::Yuv420p8,
            10 => PixelFormat::Yuv420p10,
            b => return Err(Error::Unsupported(format!("VP9 bit depth {b}"))),
        };
        let matrix = if self.matrix_unknown {
            if f.height >= 720 || f.width >= 1280 { ColorMatrix::Bt709 } else { ColorMatrix::Bt601 }
        } else {
            self.matrix
        };
        let mut planes = f.planes.into_iter();
        let (y, u, v) = (planes.next().unwrap(), planes.next().unwrap(), planes.next().unwrap());
        Ok(VideoFrame {
            width: f.width,
            height: f.height,
            format,
            matrix,
            range: self.range,
            strides: [f.strides[0], f.strides[1], f.strides[2]],
            planes: [y, u, v],
            pts: f.pts.unwrap_or(fallback_pts),
        })
    }
}

fn map_err(e: VpError) -> Error {
    match e {
        VpError::Unsupported(s) => Error::Unsupported(s),
        VpError::Unimplemented(s) => Error::Unsupported(s.into()),
        other => Error::Invalid(format!("vp9: {other:?}")),
    }
}

impl VideoDecoder for Vp9VideoDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        self.decode(packet)
    }

    fn receive_frame(&mut self) -> Result<Option<VideoFrame>> {
        Ok(self.out.pop_front())
    }

    fn flush(&mut self) {
        self.out.clear();
        self.reset();
    }

    fn drain(&mut self) -> Result<()> {
        // Every packet is decoded as it arrives and VP9 has no frame reordering delay, so nothing is held back.
        Ok(())
    }
}

/// Split a packet into its coded frames (the superframe index of VP9 Annex B), or return it whole.
fn split_superframe(data: &[u8]) -> Vec<&[u8]> {
    let n = data.len();
    if n >= 2 {
        let marker = data[n - 1];
        if marker & 0xe0 == 0xc0 {
            let frames = (marker & 7) as usize + 1;
            let mag = ((marker >> 3) & 3) as usize + 1;
            let index = 2 + mag * frames;
            if n >= index && data[n - index] == marker {
                let mut out = Vec::with_capacity(frames);
                let (mut off, mut x) = (0usize, n - index + 1);
                for _ in 0..frames {
                    let mut sz = 0usize;
                    for j in 0..mag {
                        sz |= (data[x] as usize) << (8 * j);
                        x += 1;
                    }
                    if off + sz <= n {
                        out.push(&data[off..off + sz]);
                    }
                    off += sz;
                }
                if !out.is_empty() {
                    return out;
                }
            }
        }
    }
    vec![data]
}

#[derive(Debug, Default, Clone, Copy)]
struct Color {
    space: u32,
    full_range: bool,
}

#[derive(Debug, Default)]
struct Peek {
    show_existing: bool,
    key_frame: bool,
    /// An explicitly coded frame size (key frames, intra-only frames, inter frames not copying a reference's size).
    size: Option<(u32, u32)>,
    color: Option<Color>,
    unsupported_profile: bool,
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn f(&mut self, n: usize) -> Result<u32> {
        let mut v = 0;
        for _ in 0..n {
            let byte = *self.d.get(self.pos / 8).ok_or(Error::Truncated)?;
            v = (v << 1) | ((byte >> (7 - self.pos % 8)) & 1) as u32;
            self.pos += 1;
        }
        Ok(v)
    }
}

/// Read what the wrapper needs from the start of an uncompressed header (VP9 bitstream specification, 6.2): whether
/// the frame is a key frame, its explicit size, and the colour configuration of key frames.
fn peek_header(data: &[u8]) -> Result<Peek> {
    let mut b = Bits { d: data, pos: 0 };
    if b.f(2)? != 2 {
        return Err(Error::Invalid("VP9 frame marker".into()));
    }
    let lo = b.f(1)?;
    let hi = b.f(1)?;
    let profile = (hi << 1) | lo;
    if profile == 3 {
        b.f(1)?;
    }
    let mut p = Peek { unsupported_profile: profile == 1 || profile == 3, ..Peek::default() };
    if b.f(1)? == 1 {
        b.f(3)?;
        p.show_existing = true;
        return Ok(p);
    }
    p.key_frame = b.f(1)? == 0;
    let show_frame = b.f(1)? == 1;
    let error_res = b.f(1)? == 1;
    let sync = |b: &mut Bits| -> Result<()> {
        if b.f(24)? != 0x49_8342 {
            return Err(Error::Invalid("VP9 sync code".into()));
        }
        Ok(())
    };
    let color_config = |b: &mut Bits, p: &mut Peek| -> Result<()> {
        if profile >= 2 {
            let ten_or_twelve = b.f(1)?;
            if ten_or_twelve == 1 {
                p.unsupported_profile = true; // 12-bit
            }
        }
        let space = b.f(3)?;
        if space != 7 {
            let full_range = b.f(1)? == 1;
            p.color = Some(Color { space, full_range });
            if profile == 1 || profile == 3 {
                b.f(3)?;
            }
        } else {
            p.unsupported_profile = true; // RGB means 4:4:4
        }
        Ok(())
    };
    let frame_size = |b: &mut Bits, p: &mut Peek| -> Result<()> {
        let w = b.f(16)? + 1;
        let h = b.f(16)? + 1;
        p.size = Some((w, h));
        Ok(())
    };
    if p.key_frame {
        sync(&mut b)?;
        color_config(&mut b, &mut p)?;
        frame_size(&mut b, &mut p)?;
    } else {
        let intra_only = if show_frame { false } else { b.f(1)? == 1 };
        if !error_res {
            b.f(2)?; // reset_frame_context
        }
        if intra_only {
            sync(&mut b)?;
            if profile > 0 {
                color_config(&mut b, &mut p)?;
            }
            b.f(8)?; // refresh_frame_flags
            frame_size(&mut b, &mut p)?;
        } else {
            b.f(8)?; // refresh_frame_flags
            for _ in 0..3 {
                b.f(4)?; // ref_frame_idx, sign bias
            }
            let mut found = false;
            for _ in 0..3 {
                if b.f(1)? == 1 {
                    found = true;
                    break;
                }
            }
            if !found {
                frame_size(&mut b, &mut p)?;
            }
        }
    }
    Ok(p)
}
