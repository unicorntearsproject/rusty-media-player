//! HEVC (and 10-bit H.264) through Media Foundation on Windows. The HEVC decoder there is a Store extension ("HEVC Video Extensions"):
//! when it is not installed there is no decoder to find, and the message says how to get it. Samples go in as Annex B (the format the
//! Microsoft decoder takes), NV12 or P010 comes out, and `semi_planar_frame` turns that into ours.
#![allow(unsafe_code)]
use crate::{SemiPlanar, semi_planar_frame};
use rvp_codec_hevc::nal;
use rvp_codec_hevc::ps::{Colour, Sps, hvcc_length_size, hvcc_units};
use rvp_core::{
    Error, Packet, PlatformSupport, PlatformVideo, Result as CoreResult, StreamInfo, VideoDecoder, VideoFrame,
};
use std::collections::VecDeque;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::core::GUID;

const MISSING: &str = "the HEVC Video Extensions are not installed (get them from the Microsoft Store, or install a graphics driver that comes with HEVC decoding)";

/// Media Foundation as a [`PlatformVideo`].
pub struct MediaFoundationPlatform;

impl MediaFoundationPlatform {
    /// Nothing is started until a stream asks.
    pub fn new() -> Self {
        Self
    }
}

impl Default for MediaFoundationPlatform {
    fn default() -> Self {
        Self::new()
    }
}

fn startup() -> Result<(), String> {
    // SAFETY: plain COM and Media Foundation start-up; a thread already in another apartment mode is fine for our use.
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(format!("COM could not start ({hr:?})"));
        }
        MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| format!("Media Foundation could not start ({e})"))
    }
}

/// The first decoder transform for `subtype`, if the system has one.
fn find_decoder(subtype: GUID) -> Result<Option<IMFTransform>, String> {
    startup()?;
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: the out pointers are valid; the array MFTEnumEx allocates is released with CoTaskMemFree after use.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            None,
            &mut activates,
            &mut count,
        )
        .map_err(|e| format!("Media Foundation could not list decoders ({e})"))?;
        let mut found = None;
        if !activates.is_null() {
            let list = std::slice::from_raw_parts(activates, count as usize);
            for a in list.iter().flatten() {
                if found.is_none() {
                    if let Ok(t) = a.ActivateObject::<IMFTransform>() {
                        found = Some(t);
                    }
                }
            }
            // Drop the activation objects, then free the array they sat in.
            for i in 0..count as usize {
                std::ptr::drop_in_place(activates.add(i));
            }
            CoTaskMemFree(Some(activates as *const _));
        }
        Ok(found)
    }
}

fn subtype_of(info: &StreamInfo) -> Option<GUID> {
    match info.codec.as_str() {
        "hevc" => Some(MFVideoFormat_HEVC),
        "h264" => Some(MFVideoFormat_H264),
        _ => None,
    }
}

impl PlatformVideo for MediaFoundationPlatform {
    fn name(&self) -> &str {
        "Media Foundation"
    }

    fn supports(&self, info: &StreamInfo) -> PlatformSupport {
        let Some(sub) = subtype_of(info) else {
            return PlatformSupport::No(format!(
                "Media Foundation is only used for HEVC and 10-bit H.264 here, not {}",
                info.codec
            ));
        };
        match find_decoder(sub) {
            Ok(Some(_)) => PlatformSupport::Yes,
            Ok(None) if info.codec == "hevc" => PlatformSupport::No(MISSING.into()),
            Ok(None) => PlatformSupport::No("Windows has no H.264 decoder that takes this stream".into()),
            Err(e) => PlatformSupport::No(e),
        }
    }

    fn open(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        let unsupported = |why: String| {
            Error::Unsupported(format!("video codec `{}` [Media Foundation: {why}]", info.codec))
        };
        let sub = subtype_of(info).ok_or_else(|| unsupported(String::from("not offered here")))?;
        let mft = find_decoder(sub).map_err(unsupported)?.ok_or_else(|| unsupported(MISSING.into()))?;
        let v = info.video.ok_or_else(|| Error::Invalid(String::from("no video info")))?;
        let (parameter_sets, length_size, sps) = if info.codec == "hevc" {
            let units = hvcc_units(&info.extra_data);
            let sps = units.iter().find(|(k, _)| *k == 33).and_then(|(_, u)| {
                let (mut rbsp, mut rem) = (Vec::new(), Vec::new());
                nal::unescape(&u[2..], &mut rbsp, &mut rem);
                Sps::parse(&rbsp).ok()
            });
            let mut ps = Vec::new();
            for (k, u) in &units {
                if matches!(k, 32..=34) {
                    ps.extend_from_slice(&[0, 0, 0, 1]);
                    ps.extend_from_slice(u);
                }
            }
            (ps, hvcc_length_size(&info.extra_data), sps)
        } else {
            (
                avcc_parameter_sets(&info.extra_data),
                (info.extra_data.get(4).copied().unwrap_or(3) & 3) as usize + 1,
                None,
            )
        };
        let ten = if info.codec == "hevc" {
            matches!(rvp_core::hevc_profile(info), Some(2))
        } else {
            rvp_core::avcc_bit_depth(&info.extra_data) > 8
        };
        // SAFETY: COM calls on interfaces we own.
        unsafe {
            let input = MFCreateMediaType().map_err(|e| unsupported(e.to_string()))?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| unsupported(e.to_string()))?;
            input.SetGUID(&MF_MT_SUBTYPE, &sub).map_err(|e| unsupported(e.to_string()))?;
            input
                .SetUINT64(&MF_MT_FRAME_SIZE, ((v.width as u64) << 32) | v.height as u64)
                .map_err(|e| unsupported(e.to_string()))?;
            input
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| unsupported(e.to_string()))?;
            mft.SetInputType(0, &input, 0)
                .map_err(|e| unsupported(format!("the decoder does not accept this stream ({e})")))?;
        }
        let mut dec = MfDecoder {
            mft,
            ten,
            length_size,
            parameter_sets,
            sent_parameter_sets: false,
            out: VecDeque::new(),
            sps,
            output_ready: false,
            stride: v.width as usize,
        };
        dec.choose_output().map_err(unsupported)?;
        // SAFETY: control messages on our transform.
        unsafe {
            let _ = dec.mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            let _ = dec.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            let _ = dec.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
        }
        Ok(Box::new(dec))
    }
}

fn avcc_parameter_sets(avcc: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if avcc.len() < 8 {
        return out;
    }
    let mut at = 5;
    let n_sps = (avcc[at] & 0x1f) as usize;
    at += 1;
    for pass in 0..2 {
        let n = if pass == 0 { n_sps } else { avcc.get(at).copied().unwrap_or(0) as usize };
        if pass == 1 {
            at += 1;
        }
        for _ in 0..n {
            let Some(len) = avcc.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize) else {
                return out;
            };
            at += 2;
            if let Some(u) = avcc.get(at..at + len) {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(u);
            }
            at += len;
        }
    }
    out
}

struct MfDecoder {
    mft: IMFTransform,
    ten: bool,
    length_size: usize,
    parameter_sets: Vec<u8>,
    sent_parameter_sets: bool,
    out: VecDeque<VideoFrame>,
    sps: Option<Sps>,
    output_ready: bool,
    stride: usize,
}

impl MfDecoder {
    /// Pick the output type: P010 for ten bits, NV12 for eight.
    fn choose_output(&mut self) -> Result<(), String> {
        let want = if self.ten { MFVideoFormat_P010 } else { MFVideoFormat_NV12 };
        // SAFETY: COM calls on our transform; each type is released when dropped.
        unsafe {
            for i in 0..32 {
                let Ok(t) = self.mft.GetOutputAvailableType(0, i) else { break };
                if t.GetGUID(&MF_MT_SUBTYPE).ok() == Some(want) {
                    self.mft.SetOutputType(0, &t, 0).map_err(|e| {
                        format!("the decoder will not give {} ({e})", if self.ten { "P010" } else { "NV12" })
                    })?;
                    self.stride = t
                        .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                        .map(|s| (s as i32).unsigned_abs() as usize)
                        .unwrap_or(self.stride);
                    self.output_ready = true;
                    return Ok(());
                }
            }
        }
        Err(format!("the decoder offers no {} output", if self.ten { "P010" } else { "NV12" }))
    }

    fn pull(&mut self) -> Result<(), Error> {
        loop {
            // SAFETY: COM calls; the output sample (ours or the decoder's) is released after reading.
            unsafe {
                let info = self.mft.GetOutputStreamInfo(0).map_err(|e| Error::Invalid(e.to_string()))?;
                let provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
                let mut buffer = MFT_OUTPUT_DATA_BUFFER { dwStreamID: 0, ..Default::default() };
                if !provides {
                    let sample = MFCreateSample().map_err(|e| Error::Invalid(e.to_string()))?;
                    let buf = MFCreateMemoryBuffer(info.cbSize.max(1))
                        .map_err(|e| Error::Invalid(e.to_string()))?;
                    sample.AddBuffer(&buf).map_err(|e| Error::Invalid(e.to_string()))?;
                    buffer.pSample = std::mem::ManuallyDrop::new(Some(sample));
                }
                let mut status = 0u32;
                let r = self.mft.ProcessOutput(0, std::slice::from_mut(&mut buffer), &mut status);
                let sample = std::mem::ManuallyDrop::take(&mut buffer.pSample);
                let _events = std::mem::ManuallyDrop::take(&mut buffer.pEvents);
                match r {
                    Ok(()) => {}
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        self.choose_output().map_err(Error::Invalid)?;
                        continue;
                    }
                    Err(e) => {
                        return Err(Error::Invalid(format!(
                            "Media Foundation could not decode a picture ({e})"
                        )));
                    }
                }
                let Some(sample) = sample else { continue };
                let time = sample.GetSampleTime().unwrap_or(0) / 10;
                let contiguous =
                    sample.ConvertToContiguousBuffer().map_err(|e| Error::Invalid(e.to_string()))?;
                let (mut ptr, mut cur) = (std::ptr::null_mut(), 0u32);
                contiguous.Lock(&mut ptr, None, Some(&mut cur)).map_err(|e| Error::Invalid(e.to_string()))?;
                let data = std::slice::from_raw_parts(ptr, cur as usize).to_vec();
                let _ = contiguous.Unlock();
                let frame = self.frame_from(&data)?;
                self.out.push_back(VideoFrame { pts: time, ..frame });
            }
        }
    }

    fn frame_from(&self, data: &[u8]) -> Result<VideoFrame, Error> {
        let sps = self.sps.as_ref().ok_or(Error::Invalid(String::from("no sequence parameter set")))?;
        let (w, h) = sps.display_size();
        let stride = self.stride.max(1);
        // The chroma plane starts after the luma plane, which has as many rows as the buffer holds (the decoder pads the height).
        let rows = data.len() * 2 / (3 * stride);
        semi_planar_frame(
            data,
            SemiPlanar {
                width: (w & !1) as usize,
                height: (h & !1) as usize,
                crop: (sps.crop[0] as usize, sps.crop[2] as usize),
                y: (0, stride),
                uv: (stride * rows, stride),
                ten: self.ten,
                colour: sps.colour,
            },
        )
    }
}

impl VideoDecoder for MfDecoder {
    fn send_packet(&mut self, packet: &Packet) -> CoreResult<()> {
        // The sample as Annex B, with the parameter sets in front of the first and of every key frame.
        let mut data = Vec::with_capacity(packet.data.len() + self.parameter_sets.len() + 16);
        if !self.sent_parameter_sets || packet.keyframe {
            data.extend_from_slice(&self.parameter_sets);
            self.sent_parameter_sets = true;
        }
        for u in nal::split_length_prefixed(&packet.data, self.length_size) {
            data.extend_from_slice(&[0, 0, 0, 1]);
            data.extend_from_slice(u);
        }
        // SAFETY: COM calls on interfaces we own.
        unsafe {
            let sample = MFCreateSample().map_err(|e| Error::Invalid(e.to_string()))?;
            let buf = MFCreateMemoryBuffer(data.len() as u32).map_err(|e| Error::Invalid(e.to_string()))?;
            let (mut ptr, mut max) = (std::ptr::null_mut(), 0u32);
            buf.Lock(&mut ptr, Some(&mut max), None).map_err(|e| Error::Invalid(e.to_string()))?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            let _ = buf.Unlock();
            buf.SetCurrentLength(data.len() as u32).map_err(|e| Error::Invalid(e.to_string()))?;
            sample.AddBuffer(&buf).map_err(|e| Error::Invalid(e.to_string()))?;
            sample.SetSampleTime(packet.pts * 10).map_err(|e| Error::Invalid(e.to_string()))?;
            if packet.duration > 0 {
                let _ = sample.SetSampleDuration(packet.duration * 10);
            }
            let mut r = self.mft.ProcessInput(0, &sample, 0);
            if let Err(e) = &r {
                if e.code() == MF_E_NOTACCEPTING {
                    // Output has to be taken before more input is accepted.
                    self.pull()?;
                    r = self.mft.ProcessInput(0, &sample, 0);
                }
            }
            r.map_err(|e| Error::Invalid(format!("Media Foundation refused a sample ({e})")))?;
        }
        self.pull()
    }

    fn receive_frame(&mut self) -> CoreResult<Option<VideoFrame>> {
        Ok(self.out.pop_front())
    }

    fn flush(&mut self) {
        // SAFETY: control message on our transform.
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
        self.out.clear();
        self.sent_parameter_sets = false;
    }

    fn drain(&mut self) -> CoreResult<()> {
        // SAFETY: control message on our transform.
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
        }
        self.pull()
    }
}

#[allow(dead_code)]
fn _colour(_: Colour) {}
