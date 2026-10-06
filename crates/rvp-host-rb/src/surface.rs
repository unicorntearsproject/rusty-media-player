//! The canvas: `canvas_info`, `canvas_present` (plan A: the app composes picture and UI into one RGBA frame) and fullscreen.
use crate::api;
use bucket_v0_sys::{self as sys, err};
use rvp_host::{Rect, Surface};

/// `Surface` over the App API canvas.
pub struct RbSurface {
    /// Physical width, height and the scale.
    pub size: (u32, u32, f32),
    /// The canvas is visible (not hidden by `SUSPEND`, `VISIBILITY` or a minimized window): no frames are sent while it is not.
    pub visible: bool,
    /// Frames shown.
    pub presents: u64,
    /// Frames that were not sent because the canvas was hidden or a resize was pending (the driver redraws when it matters).
    pub skipped: u64,
    /// The display's refresh in millihertz (0 = unknown).
    pub refresh_mhz: u32,
    complained: bool,
}

impl RbSurface {
    /// Read the canvas (`canvas_info`); a canvas that cannot be read counts as 1280x720.
    pub fn new() -> Self {
        let mut s = Self {
            size: (1280, 720, 1.0),
            visible: true,
            presents: 0,
            skipped: 0,
            refresh_mhz: 0,
            complained: false,
        };
        s.refresh();
        s
    }

    /// Read the canvas again. Returns false if the call failed.
    pub fn refresh(&mut self) -> bool {
        let mut info = sys::CanvasInfo { struct_size: 32, ..Default::default() };
        // SAFETY: `info` is the 32-byte struct.
        let r = unsafe { sys::canvas_info((&mut info as *mut sys::CanvasInfo).cast()) };
        if r < 0 || info.width == 0 || info.height == 0 {
            return false;
        }
        self.size = (info.width, info.height, if info.scale > 0.0 { info.scale } else { 1.0 });
        self.refresh_mhz = info.refresh_mhz;
        self.visible = info.flags & sys::canvas_flags::VISIBLE != 0 || info.struct_size < 28;
        true
    }

    /// Whether the canvas is fullscreen according to `canvas_info`.
    pub fn is_fullscreen() -> bool {
        let mut info = sys::CanvasInfo { struct_size: 32, ..Default::default() };
        // SAFETY: `info` is the 32-byte struct.
        let r = unsafe { sys::canvas_info((&mut info as *mut sys::CanvasInfo).cast()) };
        r >= 0 && info.flags & sys::canvas_flags::FULLSCREEN != 0
    }
}

impl Default for RbSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl Surface for RbSurface {
    fn size(&self) -> (u32, u32, f32) {
        self.size
    }

    fn present_rgba(&mut self, rgba: &[u8], dirty: Rect) {
        let (w, h, _) = self.size;
        if !self.visible || rgba.len() != w as usize * h as usize * 4 {
            // Hidden, or a frame for an old size: the driver draws everything again when the canvas is back (`invalidate`), and
            // a `RESIZE` makes the app draw at the new size.
            self.skipped += 1;
            return;
        }
        // SAFETY: the buffer is exactly `w * h * 4` bytes, as the call requires.
        let r = unsafe {
            sys::canvas_present(
                rgba.as_ptr(),
                api::len32(rgba.len()),
                dirty.x,
                dirty.y,
                dirty.w.min(i32::MAX as u32) as i32,
                dirty.h.min(i32::MAX as u32) as i32,
            )
        };
        match r {
            0 => self.presents += 1,
            // A resize is pending: nothing was drawn, and the `RESIZE` event is on its way.
            err::BUSY => self.skipped += 1,
            e => {
                self.skipped += 1;
                if !self.complained {
                    self.complained = true;
                    api::warn(&format!("canvas_present failed: {}", api::code_name(e)));
                }
            }
        }
    }

    fn set_fullscreen(&mut self, on: bool) {
        // SAFETY: no pointers. The result arrives as a `RESIZE` with the fullscreen flag.
        unsafe { sys::canvas_fullscreen(on as i32) };
    }
}
