//! The optional video layer (`video_present`): the 104-byte layout, plane access and hiding.
mod common;
use bucket_v0_sys::err;
use common::Harness;
use rvp_core::{ColorMatrix, ColorRange, PixelFormat, VideoFrame};
use rvp_host::Rect;
use rvp_host_rb::video::{VideoLayer, frame_raw};

fn frame(format: PixelFormat) -> VideoFrame {
    let bytes = if format == PixelFormat::Yuv420p10 { 2 } else { 1 };
    let (w, h) = (6usize, 4usize);
    VideoFrame {
        width: w as u32,
        height: h as u32,
        format,
        matrix: ColorMatrix::Bt2020,
        range: ColorRange::Full,
        planes: [
            (0..w * h * bytes).map(|i| i as u8).collect(),
            vec![128; w / 2 * h / 2 * bytes],
            vec![64; w / 2 * h / 2 * bytes],
        ],
        strides: [w * bytes, w / 2 * bytes, w / 2 * bytes],
        pts: 1_234_567,
    }
}

#[test]
fn the_frame_is_described_in_the_apis_layout() {
    let h = Harness::new();
    let mut layer = VideoLayer::new();
    let f = frame(PixelFormat::Yuv420p8);
    let r = layer.present(&f, Rect { x: 10, y: 20, w: 300, h: 160 }, false);
    assert_eq!(r, 0);
    let rec = h.mock.with(|s| s.videos[0].clone());
    let raw = rec.raw;
    assert_eq!((raw.struct_size, raw.width, raw.height), (104, 6, 4));
    assert_eq!((raw.format, raw.matrix, raw.range, raw.flags), (0, 2, 1, 0));
    assert_eq!(raw.plane_lens, [24, 6, 6, 0]);
    assert_eq!(raw.strides, [6, 3, 3, 0], "strides are bytes");
    assert_eq!(raw.pts_us, 1_234_567);
    assert_eq!(raw.dest, [10, 20, 300, 160]);
    assert_eq!(raw.planes[3], 0, "the fourth plane is unused");
    assert_eq!(rec.y_first_row, f.planes[0][..6], "the OS can read the planes during the call");
    assert_eq!(layer.frames, 1);
}

#[test]
fn ten_bit_frames_nearest_scaling_and_hiding() {
    let h = Harness::new();
    let mut layer = VideoLayer::new();
    let f = frame(PixelFormat::Yuv420p10);
    layer.present(&f, Rect { x: 0, y: 0, w: 6, h: 4 }, true);
    let raw = h.mock.with(|s| s.videos[0].raw);
    assert_eq!(raw.format, 1);
    assert_eq!(raw.strides, [12, 6, 6, 0], "bytes, not samples: 10-bit samples are two bytes");
    assert_eq!(raw.flags & (1 << 8), 1 << 8, "bit 8 asks for nearest-neighbour scaling");
    // An item without a picture hides the layer: an all-zero destination, once.
    assert_eq!(layer.hide(), 0);
    assert_eq!(layer.hide(), 0);
    let videos = h.mock.with(|s| s.videos.clone());
    assert_eq!(videos.len(), 2);
    assert_eq!(videos[1].raw.dest, [0; 4]);
    assert_eq!(videos[1].raw.struct_size, 104);
}

#[test]
fn without_the_capability_the_layer_is_unsupported_and_oversize_planes_are_refused() {
    let h = Harness::with(|s| s.caps = bucket_v0_sys::caps::CANVAS);
    let mut layer = VideoLayer::new();
    assert_eq!(layer.present(&frame(PixelFormat::Yuv420p8), Rect::default(), false), err::UNSUPPORTED);
    assert_eq!(layer.frames, 0);
    assert!(h.mock.with(|s| s.videos.is_empty()));
    let mut f = frame(PixelFormat::Yuv420p8);
    f.strides[0] = u32::MAX as usize + 1;
    assert!(frame_raw(&f, Rect::default(), false).is_none());
    assert_eq!(layer.present(&f, Rect::default(), false), err::TOO_LARGE);
}
