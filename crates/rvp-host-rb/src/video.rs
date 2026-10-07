//! The optional video layer (`video_present`, capability `VIDEO_YUV`): the picture goes to the OS as YUV, converted and scaled
//! natively, and the app's canvas has a hole where it shows.
//!
//! This is the accelerator behind plan A. Plan A (the default) lets the app compose picture and UI into one RGBA frame for
//! `canvas_present`; this layer is ready for the day the app draws a transparent hole instead (see `docs/host-api.md`). Nothing in
//! the default pipeline calls it yet.
use bucket_v0_sys::{self as sys, err, video};
use rvp_core::{ColorMatrix, ColorRange, PixelFormat, VideoFrame};
use rvp_host::Rect;

/// Build the 104-byte description of `frame` for `dest`. The planes must stay alive and unmoved until the call that gets the
/// struct returns. Returns `None` if a plane or stride does not fit the API's `u32` fields.
pub fn frame_raw(frame: &VideoFrame, dest: Rect, nearest: bool) -> Option<sys::VideoFrameRaw> {
    let mut planes = [0u32; 4];
    let mut lens = [0u32; 4];
    let mut strides = [0u32; 4];
    for i in 0..3 {
        let p = &frame.planes[i];
        planes[i] = sys::ptr32(p.as_ptr());
        lens[i] = u32::try_from(p.len()).ok()?;
        strides[i] = u32::try_from(frame.strides[i]).ok()?;
    }
    Some(sys::VideoFrameRaw {
        struct_size: 104,
        width: frame.width,
        height: frame.height,
        format: match frame.format {
            PixelFormat::Yuv420p8 => video::FORMAT_YUV420_8,
            PixelFormat::Yuv420p10 => video::FORMAT_YUV420_10,
            // Rusty Bucket has no platform decoder, so no packed picture reaches it; say so rather than guess a layout.
            PixelFormat::Rgba8 => return None,
        },
        matrix: match frame.matrix {
            ColorMatrix::Bt601 => video::MATRIX_BT601,
            ColorMatrix::Bt709 => video::MATRIX_BT709,
            ColorMatrix::Bt2020 => video::MATRIX_BT2020,
        },
        range: match frame.range {
            ColorRange::Limited => video::RANGE_LIMITED,
            ColorRange::Full => video::RANGE_FULL,
        },
        // No transfer or primaries: BT.2020 content is treated as SDR for now.
        flags: if nearest { video::FLAG_NEAREST } else { 0 },
        reserved: 0,
        planes,
        plane_lens: lens,
        strides,
        pts_us: frame.pts,
        dest: [dest.x, dest.y, dest.w.min(i32::MAX as u32) as i32, dest.h.min(i32::MAX as u32) as i32],
    })
}

/// The layer under the canvas.
#[derive(Debug, Default)]
pub struct VideoLayer {
    /// Frames shown.
    pub frames: u64,
    hidden: bool,
}

impl VideoLayer {
    /// A layer (nothing shown yet).
    pub fn new() -> Self {
        Self::default()
    }

    /// Show `frame` in `dest` (canvas pixels). Returns the call's result.
    pub fn present(&mut self, frame: &VideoFrame, dest: Rect, nearest: bool) -> i32 {
        let Some(raw) = frame_raw(frame, dest, nearest) else { return err::TOO_LARGE };
        // SAFETY: `raw` is the 104-byte struct; the planes it points to are `frame`'s and outlive the call.
        let r = unsafe { sys::video_present((&raw as *const sys::VideoFrameRaw).cast()) };
        if r >= 0 {
            self.frames += 1;
            self.hidden = false;
        }
        r
    }

    /// Hide the layer (an item without a picture): all-zero `dest`.
    pub fn hide(&mut self) -> i32 {
        if self.hidden {
            return 0;
        }
        let raw = sys::VideoFrameRaw { struct_size: 104, ..Default::default() };
        // SAFETY: `raw` is the 104-byte struct.
        let r = unsafe { sys::video_present((&raw as *const sys::VideoFrameRaw).cast()) };
        self.hidden = r >= 0;
        r
    }
}
