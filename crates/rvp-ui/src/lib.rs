//! The player UI, drawn by us into an RGBA framebuffer with Unicorn Tears tokens so it looks the same
//! in a browser and in Rusty Bucket's Canvas surface. Milestone 5 builds the widgets; this is the
//! framebuffer seed.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::vec::Vec;
use theme::Rgba;

/// An RGBA8 framebuffer.
#[derive(Debug, Clone)]
pub struct FrameBuffer {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA, straight alpha.
    pub pixels: Vec<u8>,
}

impl FrameBuffer {
    /// A buffer filled with the page background (`ink-900`).
    pub fn new(width: u32, height: u32) -> Self {
        let mut fb = Self { width, height, pixels: alloc::vec![0; width as usize * height as usize * 4] };
        fb.clear(theme::tokens::BG_PAGE);
        fb
    }

    /// Fill everything with `c`.
    pub fn clear(&mut self, c: Rgba) {
        for px in self.pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[c.r, c.g, c.b, c.a]);
        }
    }

    /// Fill a rectangle, clipped to the buffer, with opaque `c`.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, c: Rgba) {
        let x0 = x.max(0) as u32;
        let y0 = y.max(0) as u32;
        let x1 = (x as i64 + w as i64).clamp(0, self.width as i64) as u32;
        let y1 = (y as i64 + h as i64).clamp(0, self.height as i64) as u32;
        for yy in y0..y1 {
            for xx in x0..x1 {
                let i = (yy as usize * self.width as usize + xx as usize) * 4;
                self.pixels[i..i + 4].copy_from_slice(&[c.r, c.g, c.b, c.a]);
            }
        }
    }

    /// The pixel at (x, y).
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        Rgba::new(self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theme::tokens;

    #[test]
    fn starts_on_ink_and_fill_rect_clips() {
        let mut fb = FrameBuffer::new(8, 8);
        assert_eq!(fb.pixel(3, 3), tokens::INK_900);
        fb.fill_rect(-2, 6, 100, 100, tokens::MAGENTA_500);
        assert_eq!(fb.pixel(0, 7), tokens::MAGENTA_500);
        assert_eq!(fb.pixel(7, 5), tokens::INK_900);
    }
}
