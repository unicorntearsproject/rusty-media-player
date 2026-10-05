//! A WebAssembly smoke test for the codec stack: demux an AV1 (rav1d) or H.264 (our own decoder) file held in
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
