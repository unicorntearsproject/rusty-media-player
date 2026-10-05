//! A WebAssembly smoke test for the codec stack: demux an AV1 (rav1d), H.264 (our own decoder) or VP9 (rusty_vp9) file held in
//! memory and decode every frame, returning a count and a hash. Native and wasm32 builds must agree
//! (`cargo xtask wasm-smoke`). The same module doubles as the wasm benchmark (`tools/wasm-smoke.mjs ... --bench`).
//!
//! The wasm exports take raw pointers because the module is driven from plain JavaScript without any binding
//! generator.
use rvp_core::task::block_on;
use rvp_core::{StreamKind, VideoFrame};
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;

/// Decode all video frames of an AV1 or H.264 file in MP4/WebM/MKV. Returns `(frame count, FNV-1a hash of all planes)`.
pub fn decode_av1(file: Vec<u8>) -> Result<(u32, u64), String> {
    decode_video(file)
}

/// Decode all video frames of the first video stream with the matching decoder.
pub fn decode_video(file: Vec<u8>) -> Result<(u32, u64), String> {
    block_on(async {
        let mut d = open(MemSource::new(file)).await.map_err(|e| e.to_string())?;
        let info =
            d.streams().iter().find(|s| s.kind == StreamKind::Video).cloned().ok_or("no video stream")?;
        let mut dec = match info.codec.as_str() {
            "h264" => rvp_codec_h264::h264_decoder(&info),
            "vp9" => rvp_codec_vp9::vp9_decoder(&info),
            _ => rvp_codec_av1::av1_decoder(&info),
        }
        .map_err(|e| e.to_string())?;
        let (mut n, mut h) = (0u32, 0xcbf2_9ce4_8422_2325u64);
        let mut eat = |f: VideoFrame| {
            n += 1;
            for p in &f.planes {
                for &b in p {
                    h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
                }
            }
        };
        while let Some(p) = d.next_packet().await.map_err(|e| e.to_string())? {
            if p.stream_id == info.id {
                dec.send_packet(&p).map_err(|e| e.to_string())?;
                while let Some(f) = dec.receive_frame().map_err(|e| e.to_string())? {
                    eat(f);
                }
            }
        }
        dec.drain().map_err(|e| e.to_string())?;
        while let Some(f) = dec.receive_frame().map_err(|e| e.to_string())? {
            eat(f);
        }
        Ok((n, h))
    })
}

static mut RESULT: (u32, u64) = (0, 0);

/// Allocate `len` bytes for the host to write the file into.
#[unsafe(no_mangle)]
pub extern "C" fn smoke_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Decode the `len` bytes at `ptr` (from `smoke_alloc`). Returns the frame count, or -1 on error.
///
/// # Safety
/// `ptr` must come from `smoke_alloc(len)` with all `len` bytes initialised, and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn smoke_run(ptr: *mut u8, len: usize) -> i32 {
    // SAFETY: the host wrote exactly `len` bytes to a buffer from `smoke_alloc(len)`; ownership returns to us.
    let file = unsafe { Vec::from_raw_parts(ptr, len, len) };
    match decode_video(file) {
        Ok(r) => {
            // SAFETY: wasm32-unknown-unknown without threads: no concurrent access.
            unsafe { *std::ptr::addr_of_mut!(RESULT) = r };
            r.0 as i32
        }
        Err(_) => -1,
    }
}

/// Low 32 bits of the hash of the last successful run.
#[unsafe(no_mangle)]
pub extern "C" fn smoke_hash_lo() -> u32 {
    // SAFETY: single-threaded read of a plain value.
    unsafe { (*std::ptr::addr_of!(RESULT)).1 as u32 }
}

/// High 32 bits of the hash of the last successful run.
#[unsafe(no_mangle)]
pub extern "C" fn smoke_hash_hi() -> u32 {
    // SAFETY: single-threaded read of a plain value.
    unsafe { ((*std::ptr::addr_of!(RESULT)).1 >> 32) as u32 }
}

/// Run one crate's WebAssembly SIMD128 self-test (vector kernels against their scalar twins): 0 colour conversion and
/// scaler, 1 H.264, 2 VP9. Returns the number of mismatches; 0 also when this build has no SIMD128 code.
#[unsafe(no_mangle)]
pub extern "C" fn smoke_selftest_part(part: u32) -> u32 {
    match part {
        0 => rvp_core::color::simd_selftest() + rvp_ui::gfx::simd_selftest(),
        1 => rvp_codec_h264::simd_selftest(),
        2 => rvp_codec_vp9::simd_selftest(),
        _ => 0,
    }
}

/// Run every self-test; the sum of the mismatches.
#[unsafe(no_mangle)]
pub extern "C" fn smoke_selftest() -> u32 {
    smoke_selftest_part(0) + smoke_selftest_part(1) + smoke_selftest_part(2)
}

/// 1 when this module was built with SIMD128, else 0.
#[unsafe(no_mangle)]
pub extern "C" fn smoke_has_simd() -> u32 {
    cfg!(all(target_arch = "wasm32", target_feature = "simd128")) as u32
}

/// The picture path of the player, `reps` times: convert a synthetic `w` x `h` 4:2:0 frame to RGBA and scale it into a
/// `dw` x `dh` surface (`mode` bit 0: convert, bit 1: scale). The JavaScript side times the call.
#[unsafe(no_mangle)]
pub extern "C" fn smoke_present_bench(w: u32, h: u32, dw: u32, dh: u32, reps: u32, mode: u32) -> u32 {
    use rvp_core::{ColorMatrix, ColorRange, PixelFormat};
    let (wz, hz) = (w as usize, h as usize);
    let plane = |n: usize, k: usize| (0..n).map(|i| (i * k) as u8).collect::<Vec<u8>>();
    let frame = VideoFrame {
        width: w,
        height: h,
        format: PixelFormat::Yuv420p8,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        planes: [plane(wz * hz, 7), plane(wz * hz / 4, 13), plane(wz * hz / 4, 31)],
        strides: [wz, wz / 2, wz / 2],
        pts: 0,
    };
    let mut rgba = vec![0u8; wz * hz * 4];
    let mut fb = rvp_ui::gfx::FrameBuffer::new(dw, dh);
    let mut sum = 0u32;
    for _ in 0..reps {
        if mode & 1 != 0 {
            rvp_core::color::yuv420_to_rgba(&frame, &mut rgba);
        }
        if mode & 2 != 0 {
            fb.blit_scaled(rvp_ui::gfx::RectF::new(0.0, 0.0, dw as f32, dh as f32), &rgba, w, h);
        }
        sum = sum.wrapping_add(fb.pixels[(dw as usize * dh as usize * 2) & !3] as u32);
    }
    sum
}
