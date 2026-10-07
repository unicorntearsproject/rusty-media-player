//! The software decoder: a [`Backend`] that reconstructs pictures on the CPU (CABAC parsing, intra and inter prediction, inverse
//! transforms, deblocking and SAO), written from the ITU-T H.265 text.
//!
//! Samples are kept as `u16` for both 8 and 10 bits, so one code path serves both.
mod ctu;
mod deblock;
mod frame;
mod inter;
mod intra;
mod mv;
mod pic;
mod residual;
mod sao;
mod transform;

pub use frame::{Frame, Plane};

use crate::ps::Sps;
use crate::stream::{Backend, HevcStream, Picture};
use crate::{Error, Result};
use alloc::boxed::Box;
use alloc::rc::Rc;
use core::cell::RefCell;
use rvp_core::{ColorMatrix, ColorRange, PixelFormat, VideoFrame};

/// A decoded picture shared between the buffer and the pictures that predict from it.
pub type Surface = Rc<RefCell<Frame>>;

/// The software backend.
#[derive(Default)]
pub struct SwBackend {
    pool: Rc<RefCell<alloc::vec::Vec<Frame>>>,
}

impl SwBackend {
    /// A backend with an empty buffer pool.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Backend for SwBackend {
    type Surface = Surface;

    fn alloc(&mut self, sps: &Sps) -> Result<Surface> {
        let frame = {
            let mut pool = self.pool.borrow_mut();
            match pool.iter().position(|f| f.fits(sps)) {
                Some(i) => pool.swap_remove(i),
                None => Frame::new(sps),
            }
        };
        Ok(Rc::new(RefCell::new(frame)))
    }

    fn decode(&mut self, pic: &Picture<'_, Surface>) -> Result<()> {
        pic::decode_picture(pic)
    }

    fn read(&mut self, surface: &Surface, sps: &Sps) -> Result<VideoFrame> {
        surface.borrow().to_video_frame(sps)
    }
}

/// A [`rvp_core::VideoDecoder`] around the stream layer and the software backend.
pub struct HevcDecoder {
    stream: HevcStream<SwBackend>,
}

/// Create the software HEVC decoder for a stream whose configuration record is `hvcc`.
pub fn hevc_decoder(info: &rvp_core::StreamInfo) -> rvp_core::Result<Box<dyn rvp_core::VideoDecoder>> {
    if info.codec != "hevc" {
        return Err(rvp_core::Error::Unsupported(alloc::format!("not an HEVC stream: {}", info.codec)));
    }
    let stream = HevcStream::new(SwBackend::new(), &info.extra_data)?;
    Ok(Box::new(HevcDecoder { stream }))
}

impl rvp_core::VideoDecoder for HevcDecoder {
    fn send_packet(&mut self, packet: &rvp_core::Packet) -> rvp_core::Result<()> {
        self.stream.push_sample(&packet.data, packet.pts).map_err(Into::into)
    }

    fn receive_frame(&mut self) -> rvp_core::Result<Option<VideoFrame>> {
        Ok(self.stream.receive())
    }

    fn flush(&mut self) {
        self.stream.flush();
    }

    fn drain(&mut self) -> rvp_core::Result<()> {
        self.stream.drain().map_err(Into::into)
    }
}

pub(crate) fn matrix_of(sps: &Sps) -> (ColorMatrix, ColorRange) {
    let m = match sps.colour.matrix {
        9 | 10 => ColorMatrix::Bt2020,
        5 | 6 => ColorMatrix::Bt601,
        _ => ColorMatrix::Bt709,
    };
    (m, if sps.colour.full_range { ColorRange::Full } else { ColorRange::Limited })
}

pub(crate) fn format_of(sps: &Sps) -> PixelFormat {
    if sps.bit_depth_luma > 8 { PixelFormat::Yuv420p10 } else { PixelFormat::Yuv420p8 }
}

pub(crate) fn invalid<T>(what: &'static str) -> Result<T> {
    Err(Error::Invalid(what))
}
