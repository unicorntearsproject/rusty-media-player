//! A [`VideoSink`] that keeps the latest decoded picture as RGBA, for hosts whose UI composites the video
//! itself (the browser host and Rusty Bucket: one canvas, one pixel surface).
use crate::VideoSink;
use alloc::vec::Vec;
use rvp_core::{Timestamp, VideoFrame, color::yuv420_to_rgba};

/// Holds the most recently presented frame, converted to opaque RGBA8.
#[derive(Debug, Default)]
pub struct FrameSink {
    /// `width * height * 4` bytes of the latest frame.
    pub rgba: Vec<u8>,
    /// Width of `rgba` in pixels (0 before the first frame).
    pub width: u32,
    /// Height of `rgba` in pixels.
    pub height: u32,
    /// Presentation time of the latest frame.
    pub pts: Timestamp,
    /// Frames received so far; the consumer compares it with what it last drew.
    pub count: u64,
}

impl FrameSink {
    /// An empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the picture (a new file was opened, or playback stopped).
    pub fn clear(&mut self) {
        self.rgba.clear();
        self.width = 0;
        self.height = 0;
        self.count += 1;
    }
}

impl VideoSink for FrameSink {
    fn present(&mut self, frame: &VideoFrame) {
        let need = frame.width as usize * frame.height as usize * 4;
        self.rgba.resize(need, 0);
        yuv420_to_rgba(frame, &mut self.rgba);
        self.width = frame.width;
        self.height = frame.height;
        self.pts = frame.pts;
        self.count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvp_core::{ColorMatrix, ColorRange, PixelFormat};

    #[test]
    fn converts_and_counts() {
        let frame = VideoFrame {
            width: 2,
            height: 2,
            format: PixelFormat::Yuv420p8,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Full,
            planes: [alloc::vec![255; 4], alloc::vec![128], alloc::vec![128]],
            strides: [2, 1, 1],
            pts: 40_000,
        };
        let mut s = FrameSink::new();
        s.present(&frame);
        assert_eq!((s.width, s.height, s.count, s.pts), (2, 2, 1, 40_000));
        assert_eq!(&s.rgba[..4], &[255, 255, 255, 255]);
        s.clear();
        assert_eq!((s.width, s.count), (0, 2));
    }
}
