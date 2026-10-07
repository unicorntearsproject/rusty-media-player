//! HEVC (and 10-bit H.264) through VideoToolbox on macOS. The frameworks are linked the ordinary way (they are part of every Mac); the
//! decoder is built from the stream's own parameter sets, fed one length-prefixed sample at a time, and gives back NV12 or P010 pixel
//! buffers in presentation order, which `semi_planar_frame` turns into ours.
#![allow(unsafe_code, non_snake_case, non_upper_case_globals)]
use crate::{SemiPlanar, semi_planar_frame};
use rvp_codec_hevc::ps::{Colour, Sps, hvcc_length_size, hvcc_units};
use rvp_core::{
    Error, Packet, PlatformSupport, PlatformVideo, Result as CoreResult, StreamInfo, VideoDecoder, VideoFrame,
};
use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

type OSStatus = i32;
type CFTypeRef = *const c_void;
type CFAllocatorRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

const CM_TIME_VALID: u32 = 1;

impl CMTime {
    fn us(v: i64) -> Self {
        CMTime { value: v, timescale: 1_000_000, flags: CM_TIME_VALID, epoch: 0 }
    }
    const INVALID: CMTime = CMTime { value: 0, timescale: 0, flags: 0, epoch: 0 };
}

#[repr(C)]
struct CMSampleTimingInfo {
    duration: CMTime,
    presentation_time_stamp: CMTime,
    decode_time_stamp: CMTime,
}

type OutputCallback = extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, *mut c_void, CMTime, CMTime);

#[repr(C)]
struct OutputCallbackRecord {
    callback: OutputCallback,
    refcon: *mut c_void,
}

#[repr(C)]
struct CFDictionaryKeyCallBacks {
    _opaque: [usize; 5],
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFAllocatorNull: CFAllocatorRef;
    static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFDictionaryKeyCallBacks;
    fn CFRelease(cf: CFTypeRef);
    fn CFNumberCreate(allocator: CFAllocatorRef, the_type: i32, value_ptr: *const c_void) -> CFTypeRef;
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        n: isize,
        key_callbacks: *const CFDictionaryKeyCallBacks,
        value_callbacks: *const CFDictionaryKeyCallBacks,
    ) -> CFTypeRef;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        allocator: CFAllocatorRef,
        count: usize,
        sets: *const *const u8,
        sizes: *const usize,
        nal_header_length: i32,
        extensions: CFTypeRef,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: CFAllocatorRef,
        count: usize,
        sets: *const *const u8,
        sizes: *const usize,
        nal_header_length: i32,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn CMBlockBufferCreateWithMemoryBlock(
        allocator: CFAllocatorRef,
        memory_block: *mut c_void,
        block_length: usize,
        block_allocator: CFAllocatorRef,
        custom_block_source: *const c_void,
        offset_to_data: usize,
        data_length: usize,
        flags: u32,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn CMSampleBufferCreateReady(
        allocator: CFAllocatorRef,
        data_buffer: CFTypeRef,
        format_description: CFTypeRef,
        num_samples: isize,
        num_timing: isize,
        timing: *const CMSampleTimingInfo,
        num_sizes: isize,
        sizes: *const usize,
        out: *mut CFTypeRef,
    ) -> OSStatus;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFTypeRef;
    fn CVPixelBufferLockBaseAddress(buf: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buf: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddressOfPlane(buf: *mut c_void, plane: usize) -> *const u8;
    fn CVPixelBufferGetBytesPerRowOfPlane(buf: *mut c_void, plane: usize) -> usize;
    fn CVPixelBufferGetHeightOfPlane(buf: *mut c_void, plane: usize) -> usize;
    fn CVPixelBufferGetWidth(buf: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(buf: *mut c_void) -> usize;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTDecompressionSessionCreate(
        allocator: CFAllocatorRef,
        format_description: CFTypeRef,
        decoder_specification: CFTypeRef,
        destination_attributes: CFTypeRef,
        callback: *const OutputCallbackRecord,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn VTDecompressionSessionDecodeFrame(
        session: *mut c_void,
        sample: CFTypeRef,
        decode_flags: u32,
        source_frame_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    fn VTDecompressionSessionWaitForAsynchronousFrames(session: *mut c_void) -> OSStatus;
    fn VTDecompressionSessionFinishDelayedFrames(session: *mut c_void) -> OSStatus;
    fn VTDecompressionSessionInvalidate(session: *mut c_void);
}

/// kVTDecodeFrame_EnableTemporalProcessing: frames come out in presentation order.
const DECODE_TEMPORAL: u32 = 1 << 3;
/// kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange ('420v') and kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange ('x420').
const PIXEL_NV12: i32 = 0x3432_3076;
const PIXEL_P010: i32 = 0x7834_3230;

/// The pixel buffers the callback produced, and what to turn them into.
struct Shared {
    frames: RefCell<Vec<VideoFrame>>,
    error: RefCell<Option<String>>,
    ten: bool,
    colour: Colour,
}

extern "C" fn on_frame(
    refcon: *mut c_void,
    _source: *mut c_void,
    status: OSStatus,
    _flags: u32,
    buffer: *mut c_void,
    pts: CMTime,
    _dur: CMTime,
) {
    // SAFETY: `refcon` is the `Shared` the decoder boxed and keeps alive until the session is invalidated.
    let shared = unsafe { &*(refcon as *const Shared) };
    if status != 0 {
        *shared.error.borrow_mut() = Some(format!("VideoToolbox status {status}"));
        return;
    }
    if buffer.is_null() {
        return;
    }
    // SAFETY: `buffer` is a CVPixelBuffer valid for the duration of the callback; locked while it is read.
    let frame = unsafe {
        CVPixelBufferLockBaseAddress(buffer, 1);
        let (w, h) = (CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer));
        let (y, yp) =
            (CVPixelBufferGetBaseAddressOfPlane(buffer, 0), CVPixelBufferGetBytesPerRowOfPlane(buffer, 0));
        let (uv, uvp) =
            (CVPixelBufferGetBaseAddressOfPlane(buffer, 1), CVPixelBufferGetBytesPerRowOfPlane(buffer, 1));
        let uvh = CVPixelBufferGetHeightOfPlane(buffer, 1);
        let r = if y.is_null() || uv.is_null() {
            Err(Error::Invalid(String::from("VideoToolbox gave a picture without planes")))
        } else {
            // The two planes copied into one block, so the converter sees the layout `SemiPlanar` describes.
            let ysz = yp * h;
            let mut data = Vec::with_capacity(ysz + uvp * uvh);
            data.extend_from_slice(std::slice::from_raw_parts(y, ysz));
            data.extend_from_slice(std::slice::from_raw_parts(uv, uvp * uvh));
            semi_planar_frame(
                &data,
                SemiPlanar {
                    width: w & !1,
                    height: h & !1,
                    crop: (0, 0),
                    y: (0, yp),
                    uv: (ysz, uvp),
                    ten: shared.ten,
                    colour: shared.colour,
                },
            )
        };
        CVPixelBufferUnlockBaseAddress(buffer, 1);
        r
    };
    match frame {
        Ok(mut f) => {
            f.pts = if pts.timescale > 0 { pts.value * 1_000_000 / pts.timescale as i64 } else { 0 };
            shared.frames.borrow_mut().push(f);
        }
        Err(e) => *shared.error.borrow_mut() = Some(e.to_string()),
    }
}

/// VideoToolbox as a [`PlatformVideo`].
pub struct VideoToolboxPlatform;

impl VideoToolboxPlatform {
    /// There is nothing to open ahead of time.
    pub fn new() -> Self {
        Self
    }
}

impl Default for VideoToolboxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

/// The format description of a stream from its configuration record.
fn format_description(info: &StreamInfo) -> Result<(CFTypeRef, usize), String> {
    let record = &info.extra_data;
    let mut out: CFTypeRef = std::ptr::null();
    match info.codec.as_str() {
        "hevc" => {
            let units: Vec<&[u8]> = hvcc_units(record)
                .into_iter()
                .filter(|(k, _)| matches!(k, 32..=34))
                .map(|(_, u)| u)
                .collect();
            if units.is_empty() {
                return Err(String::from("the HEVC stream has no parameter sets"));
            }
            let ptrs: Vec<*const u8> = units.iter().map(|u| u.as_ptr()).collect();
            let sizes: Vec<usize> = units.iter().map(|u| u.len()).collect();
            let ls = hvcc_length_size(record);
            // SAFETY: the arrays are as long as `count` says and outlive the call.
            let st = unsafe {
                CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    std::ptr::null(),
                    units.len(),
                    ptrs.as_ptr(),
                    sizes.as_ptr(),
                    ls as i32,
                    std::ptr::null(),
                    &mut out,
                )
            };
            if st != 0 || out.is_null() {
                return Err(format!("macOS does not accept this HEVC stream's parameter sets (status {st})"));
            }
            Ok((out, ls))
        }
        "h264" => {
            // avcC: version, profile, compat, level, lengthSize, SPS count and sets, PPS count and sets.
            if record.len() < 8 {
                return Err(String::from("the H.264 stream has no parameter sets"));
            }
            let ls = (record[4] & 3) as usize + 1;
            let mut units: Vec<&[u8]> = Vec::new();
            let mut at = 5;
            let n_sps = (record[at] & 0x1f) as usize;
            at += 1;
            for _ in 0..n_sps {
                let Some(len) = record.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
                else {
                    break;
                };
                at += 2;
                if let Some(u) = record.get(at..at + len) {
                    units.push(u);
                }
                at += len;
            }
            let n_pps = record.get(at).copied().unwrap_or(0) as usize;
            at += 1;
            for _ in 0..n_pps {
                let Some(len) = record.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
                else {
                    break;
                };
                at += 2;
                if let Some(u) = record.get(at..at + len) {
                    units.push(u);
                }
                at += len;
            }
            let ptrs: Vec<*const u8> = units.iter().map(|u| u.as_ptr()).collect();
            let sizes: Vec<usize> = units.iter().map(|u| u.len()).collect();
            // SAFETY: as above.
            let st = unsafe {
                CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    std::ptr::null(),
                    units.len(),
                    ptrs.as_ptr(),
                    sizes.as_ptr(),
                    ls as i32,
                    &mut out,
                )
            };
            if st != 0 || out.is_null() {
                return Err(format!(
                    "macOS does not accept this H.264 stream's parameter sets (status {st})"
                ));
            }
            Ok((out, ls))
        }
        other => Err(format!("VideoToolbox is only used for HEVC and 10-bit H.264 here, not {other}")),
    }
}

impl PlatformVideo for VideoToolboxPlatform {
    fn name(&self) -> &str {
        "VideoToolbox"
    }

    fn supports(&self, info: &StreamInfo) -> PlatformSupport {
        match format_description(info) {
            Ok((fd, _)) => {
                // SAFETY: released once; created just above.
                unsafe { CFRelease(fd) };
                PlatformSupport::Yes
            }
            Err(why) => PlatformSupport::No(why),
        }
    }

    fn open(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        let unsupported =
            |why: String| Error::Unsupported(format!("video codec `{}` [VideoToolbox: {why}]", info.codec));
        let (fd, length_size) = format_description(info).map_err(unsupported)?;
        let ten = if info.codec == "hevc" {
            matches!(rvp_core::hevc_profile(info), Some(2))
        } else {
            rvp_core::avcc_bit_depth(&info.extra_data) > 8
        };
        let colour = if info.codec == "hevc" {
            hvcc_units(&info.extra_data).into_iter().find(|(k, _)| *k == 33).and_then(|(_, u)| {
                let mut rbsp = Vec::new();
                let mut rem = Vec::new();
                rvp_codec_hevc::nal::unescape(&u[2..], &mut rbsp, &mut rem);
                Sps::parse(&rbsp).ok().map(|s| s.colour)
            })
        } else {
            None
        }
        .unwrap_or_default();
        let shared =
            Rc::new(Shared { frames: RefCell::new(Vec::new()), error: RefCell::new(None), ten, colour });
        // Ask for the biplanar layouts we know how to read.
        let fmt = if ten { PIXEL_P010 } else { PIXEL_NV12 };
        // SAFETY: the dictionary is built from valid CF objects and released after the session took what it needs.
        let (session, attrs) = unsafe {
            let num = CFNumberCreate(std::ptr::null(), 3, &fmt as *const i32 as *const c_void);
            let keys = [kCVPixelBufferPixelFormatTypeKey];
            let values = [num];
            let attrs = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(num);
            let record =
                OutputCallbackRecord { callback: on_frame, refcon: Rc::as_ptr(&shared) as *mut c_void };
            let mut session: *mut c_void = std::ptr::null_mut();
            let st = VTDecompressionSessionCreate(
                std::ptr::null(),
                fd,
                std::ptr::null(),
                attrs,
                &record,
                &mut session,
            );
            if st != 0 || session.is_null() {
                CFRelease(attrs);
                CFRelease(fd);
                return Err(unsupported(format!(
                    "macOS could not start a decoder for this stream (status {st})"
                )));
            }
            (session, attrs)
        };
        Ok(Box::new(VtDecoder { session, format: fd, attrs, shared, length_size }))
    }
}

struct VtDecoder {
    session: *mut c_void,
    format: CFTypeRef,
    attrs: CFTypeRef,
    shared: Rc<Shared>,
    #[allow(dead_code)]
    length_size: usize,
}

impl VideoDecoder for VtDecoder {
    fn send_packet(&mut self, packet: &Packet) -> CoreResult<()> {
        if let Some(e) = self.shared.error.borrow_mut().take() {
            return Err(Error::Invalid(e));
        }
        let mut data = packet.data.clone();
        let size = data.len();
        let timing = CMSampleTimingInfo {
            duration: CMTime::INVALID,
            presentation_time_stamp: CMTime::us(packet.pts),
            decode_time_stamp: CMTime::us(packet.dts),
        };
        // SAFETY: `data` outlives the block buffer (kCFAllocatorNull: VideoToolbox does not free it) because we wait for the frame below
        // before dropping it; every CF object created here is released.
        unsafe {
            let mut block: CFTypeRef = std::ptr::null();
            let st = CMBlockBufferCreateWithMemoryBlock(
                std::ptr::null(),
                data.as_mut_ptr() as *mut c_void,
                size,
                kCFAllocatorNull,
                std::ptr::null(),
                0,
                size,
                0,
                &mut block,
            );
            if st != 0 {
                return Err(Error::Invalid(format!("VideoToolbox could not wrap a sample (status {st})")));
            }
            let mut sample: CFTypeRef = std::ptr::null();
            let st = CMSampleBufferCreateReady(
                std::ptr::null(),
                block,
                self.format,
                1,
                1,
                &timing,
                1,
                &size,
                &mut sample,
            );
            if st != 0 {
                CFRelease(block);
                return Err(Error::Invalid(format!("VideoToolbox could not make a sample (status {st})")));
            }
            let mut info = 0u32;
            let st = VTDecompressionSessionDecodeFrame(
                self.session,
                sample,
                DECODE_TEMPORAL,
                std::ptr::null_mut(),
                &mut info,
            );
            VTDecompressionSessionWaitForAsynchronousFrames(self.session);
            CFRelease(sample);
            CFRelease(block);
            if st != 0 {
                return Err(Error::Invalid(format!("VideoToolbox could not decode a picture (status {st})")));
            }
        }
        Ok(())
    }

    fn receive_frame(&mut self) -> CoreResult<Option<VideoFrame>> {
        if let Some(e) = self.shared.error.borrow_mut().take() {
            return Err(Error::Invalid(e));
        }
        let mut frames = self.shared.frames.borrow_mut();
        if frames.is_empty() {
            return Ok(None);
        }
        Ok(Some(frames.remove(0)))
    }

    fn flush(&mut self) {
        // SAFETY: the session is valid until drop.
        unsafe {
            VTDecompressionSessionFinishDelayedFrames(self.session);
            VTDecompressionSessionWaitForAsynchronousFrames(self.session);
        }
        self.shared.frames.borrow_mut().clear();
        *self.shared.error.borrow_mut() = None;
    }

    fn drain(&mut self) -> CoreResult<()> {
        // SAFETY: as above.
        unsafe {
            VTDecompressionSessionFinishDelayedFrames(self.session);
            VTDecompressionSessionWaitForAsynchronousFrames(self.session);
        }
        Ok(())
    }
}

impl Drop for VtDecoder {
    fn drop(&mut self) {
        // SAFETY: the session, the format description and the attributes were created in `open` and are released once.
        unsafe {
            VTDecompressionSessionInvalidate(self.session);
            CFRelease(self.session as CFTypeRef);
            CFRelease(self.format);
            CFRelease(self.attrs);
        }
    }
}
