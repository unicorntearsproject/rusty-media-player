//! AV1 decoding with rav1d (BSD-2-Clause), vendored and patched for wasm32 in `third_party/rav1d`.
//!
//! The decoder runs single threaded (`n_threads = 1`, `max_frame_delay = 1`), which is what wasm32 without
//! threads needs, unless the host has installed a thread pool (`rvp_core::par`): then rav1d gets that many threads
//! (frame and tile threading) and starts them through `rvp_par` (Web Workers on wasm32). It applies film grain. Output is 4:2:0 (or 4:0:0 expanded to grey chroma) at 8 or 10 bits;
//! 4:2:2 and 4:4:4 streams are rejected with `Unsupported`.
//!
//! This crate contains the only `unsafe` code that talks to rav1d: the dav1d-style C API is the only public
//! entry point rav1d offers. Every call site documents why it is sound.
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_flush, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};
use rvp_core::{
    ColorMatrix, ColorRange, Error, Packet, PixelFormat, Result, StreamInfo, StreamKind, VideoCodec,
    VideoDecoder, VideoFrame,
};
use std::collections::VecDeque;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

/// The codec this crate decodes.
pub const CODEC: VideoCodec = VideoCodec::Av1;

const EAGAIN: i32 = -11;

/// Create a decoder for an AV1 stream.
pub fn av1_decoder(info: &StreamInfo) -> Result<Box<dyn VideoDecoder>> {
    if info.kind != StreamKind::Video || info.codec != "av1" {
        return Err(Error::Unsupported(format!("not an AV1 stream: {}", info.codec)));
    }
    Ok(Box::new(Av1Decoder::new()?))
}

/// An AV1 decoder that uses no threads of its own and keeps its memory small (for a picture or two, such as a poster).
pub fn av1_decoder_light(info: &StreamInfo) -> Result<Box<dyn VideoDecoder>> {
    if info.kind != StreamKind::Video || info.codec != "av1" {
        return Err(Error::Unsupported(format!("not an AV1 stream: {}", info.codec)));
    }
    Ok(Box::new(Av1Decoder::open(1)?))
}

/// An AV1 decoder.
pub struct Av1Decoder {
    ctx: Option<Dav1dContext>,
    out: VecDeque<VideoFrame>,
}

impl Av1Decoder {
    /// Open a decoder.
    pub fn new() -> Result<Self> {
        let threads = if rvp_par::available() { rvp_core::par::threads().min(8) } else { 1 };
        Self::open(threads)
    }

    /// Open a decoder with `threads` threads (1: it decodes on the calling thread).
    fn open(threads: usize) -> Result<Self> {
        let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
        // SAFETY: `dav1d_default_settings` fully initialises the pointed-to settings.
        let mut settings = unsafe {
            dav1d_default_settings(NonNull::new_unchecked(settings.as_mut_ptr()));
            settings.assume_init()
        };
        // Several threads only where blocking is allowed and the host has said it has threads (see the module docs);
        // otherwise rav1d decodes inside `dav1d_send_data`/`dav1d_get_picture` on the calling thread.
        if threads > 1 {
            rav1d::src::lib::set_thread_spawn(rvp_par::spawn_boxed);
            settings.n_threads = threads as i32;
            settings.max_frame_delay = 0;
        } else {
            settings.n_threads = 1;
            settings.max_frame_delay = 1;
        }
        // Pictures above 8192 x 4352 are refused by the decoder (memory bound for hostile streams).
        settings.frame_size_limit = 8192 * 4352;
        let mut ctx: Option<Dav1dContext> = None;
        // SAFETY: both pointers are valid for the duration of the call; `ctx` is written on success.
        let r = unsafe { dav1d_open(Some(NonNull::from(&mut ctx)), Some(NonNull::from(&mut settings))) };
        if r.0 < 0 || ctx.is_none() {
            return Err(Error::Invalid(format!("rav1d: dav1d_open failed ({})", r.0)));
        }
        Ok(Self { ctx, out: VecDeque::new() })
    }

    /// Pull every picture currently available.
    fn pump(&mut self) -> Result<()> {
        loop {
            let mut pic = Dav1dPicture::default();
            // SAFETY: `ctx` came from `dav1d_open` and is not closed; `pic` is valid to write.
            let r = unsafe { dav1d_get_picture(self.ctx, Some(NonNull::from(&mut pic))) };
            // rav1d fills `pic` with a (reference-counted) empty picture even when it returns `EAGAIN`, so it is released in
            // every case, or one small allocation leaks per call.
            let frame = (r.0 == 0).then(|| convert(&pic));
            // SAFETY: `pic` was filled by `dav1d_get_picture` (also when it has no data), so it holds references to release.
            unsafe { dav1d_picture_unref(Some(NonNull::from(&mut pic))) };
            match r.0 {
                0 => self.out.push_back(frame.ok_or(Error::Invalid("rav1d: no picture".into()))??),
                EAGAIN => return Ok(()),
                e => return Err(Error::Invalid(format!("rav1d: get_picture failed ({e})"))),
            }
        }
    }
}

/// Copy a decoded picture into a [`VideoFrame`].
fn convert(pic: &Dav1dPicture) -> Result<VideoFrame> {
    let (w, h) = (pic.p.w as usize, pic.p.h as usize);
    let bytes = match pic.p.bpc {
        8 => 1,
        10 => 2,
        b => return Err(Error::Unsupported(format!("AV1 bit depth {b}"))),
    };
    let layout = pic.p.layout; // 0 = 4:0:0, 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4
    if layout > 1 {
        return Err(Error::Unsupported("AV1 4:2:2 / 4:4:4".into()));
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let copy = |idx: usize, pw: usize, ph: usize| -> Result<Vec<u8>> {
        let base =
            pic.data[idx].ok_or(Error::Invalid("AV1 picture without data".into()))?.as_ptr() as *const u8;
        let stride = pic.stride[(idx > 0) as usize];
        let mut v = Vec::with_capacity(pw * ph * bytes);
        for y in 0..ph {
            // SAFETY: rav1d guarantees `ph` rows of at least `pw * bytes` readable bytes, `stride` apart,
            // for each plane of a valid picture; `y < ph`.
            let row = unsafe { std::slice::from_raw_parts(base.offset(y as isize * stride), pw * bytes) };
            v.extend_from_slice(row);
        }
        Ok(v)
    };
    let y = copy(0, w, h)?;
    let (u, v) = if layout == 0 {
        let grey = if bytes == 1 { vec![128u8; cw * ch] } else { (512u16).to_le_bytes().repeat(cw * ch) };
        (grey.clone(), grey)
    } else {
        (copy(1, cw, ch)?, copy(2, cw, ch)?)
    };
    let (matrix, range) = match pic.seq_hdr {
        // SAFETY: `seq_hdr` points to the sequence header kept alive by the picture's reference.
        Some(h) => {
            let h = unsafe { h.as_ref() };
            let m = match h.mtrx {
                1 => ColorMatrix::Bt709,
                9 | 10 => ColorMatrix::Bt2020,
                5 | 6 => ColorMatrix::Bt601,
                _ if h_is_hd(pic) => ColorMatrix::Bt709,
                _ => ColorMatrix::Bt601,
            };
            (m, if h.color_range != 0 { ColorRange::Full } else { ColorRange::Limited })
        }
        None => (ColorMatrix::Bt709, ColorRange::Limited),
    };
    Ok(VideoFrame {
        width: w as u32,
        height: h as u32,
        format: if bytes == 1 { PixelFormat::Yuv420p8 } else { PixelFormat::Yuv420p10 },
        matrix,
        range,
        planes: [y, u, v],
        strides: [w * bytes, cw * bytes, cw * bytes],
        pts: pic.m.timestamp,
    })
}

/// Unspecified matrix coefficients: HD content is BT.709, SD is BT.601.
fn h_is_hd(pic: &Dav1dPicture) -> bool {
    pic.p.h >= 720 || pic.p.w >= 1280
}

impl VideoDecoder for Av1Decoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        // rav1d rejects (and, as an input-validation failure, aborts on) zero-length data.
        if packet.data.is_empty() {
            return Ok(());
        }
        let mut data = Dav1dData::default();
        // SAFETY: `data` is valid to write; the returned buffer has room for `packet.data.len()` bytes.
        let buf = unsafe { dav1d_data_create(Some(NonNull::from(&mut data)), packet.data.len()) };
        if buf.is_null() {
            return Err(Error::Invalid("rav1d: cannot allocate packet".into()));
        }
        // SAFETY: `buf` points to `packet.data.len()` writable bytes we just got from `dav1d_data_create`.
        unsafe { std::ptr::copy_nonoverlapping(packet.data.as_ptr(), buf, packet.data.len()) };
        data.m.timestamp = packet.pts;
        loop {
            // SAFETY: `ctx` is open; `data` holds a valid dav1d data buffer.
            let r = unsafe { dav1d_send_data(self.ctx, Some(NonNull::from(&mut data))) };
            if r.0 < 0 && r.0 != EAGAIN {
                // SAFETY: releases whatever reference `data` still holds.
                unsafe { dav1d_data_unref(Some(NonNull::from(&mut data))) };
                return Err(Error::Invalid(format!("rav1d: send_data failed ({})", r.0)));
            }
            self.pump()?;
            if data.sz == 0 {
                return Ok(());
            }
        }
    }

    fn receive_frame(&mut self) -> Result<Option<VideoFrame>> {
        Ok(self.out.pop_front())
    }

    fn flush(&mut self) {
        self.out.clear();
        if let Some(c) = self.ctx {
            // SAFETY: `c` is open.
            unsafe { dav1d_flush(c) };
        }
    }

    fn drain(&mut self) -> Result<()> {
        self.pump()
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        // SAFETY: `ctx` is either None (a no-op) or an open context, closed exactly once here.
        unsafe { dav1d_close(Some(NonNull::from(&mut self.ctx))) };
    }
}
