//! Hosts' platform decoders in the factory, and a scripted one for tests. See `rvp_core::platform` for the seam itself.
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use rvp_core::{
    AudioDecoder, CodecFactory, ColorMatrix, ColorRange, Error, FallbackVideo, Packet, PixelFormat,
    PlatformSupport, PlatformVideo, Result, StreamInfo, VideoDecoder, VideoFrame, open_video,
};

/// A codec factory with the host's platform decoders behind it: what `inner` decodes itself it decodes; the rest goes to the platform.
pub struct WithPlatform<F> {
    inner: F,
    platform: Option<Rc<dyn PlatformVideo>>,
}

impl<F> WithPlatform<F> {
    /// `platform` is `None` on a host with no decoder service (the factory then behaves exactly as `inner`).
    pub fn new(inner: F, platform: Option<Rc<dyn PlatformVideo>>) -> Self {
        Self { inner, platform }
    }

    /// The platform, if the host has one.
    pub fn platform(&self) -> Option<&Rc<dyn PlatformVideo>> {
        self.platform.as_ref()
    }

    fn wrap(&self, ours: Result<Box<dyn VideoDecoder>>, info: &StreamInfo) -> Result<Box<dyn VideoDecoder>> {
        let ours = match rvp_core::ours_refuses(info) {
            Some(e) => Err(e),
            None => ours,
        };
        let dec = open_video(ours, self.platform.as_ref(), info)?;
        // A codec of ours that opens and then refuses the stream (10-bit H.264) moves to the platform on the first packet.
        if info.codec == "h264" && self.platform.is_some() {
            return Ok(Box::new(FallbackVideo::new(dec, self.platform.clone(), info.clone())));
        }
        Ok(dec)
    }
}

impl<F: CodecFactory> CodecFactory for WithPlatform<F> {
    fn audio(&self, info: &StreamInfo) -> Result<Box<dyn AudioDecoder>> {
        self.inner.audio(info)
    }

    fn video(&self, info: &StreamInfo) -> Result<Box<dyn VideoDecoder>> {
        self.wrap(self.inner.video(info), info)
    }

    fn video_light(&self, info: &StreamInfo) -> Result<Box<dyn VideoDecoder>> {
        self.wrap(self.inner.video_light(info), info)
    }
}

/// A platform for tests: it supports the codecs it is given and "decodes" every packet to a flat gray frame the size of the stream (with
/// the packet's time), or says why it cannot.
#[derive(Debug, Clone, Default)]
pub struct ScriptedPlatform {
    /// Codecs it can decode (`"hevc"`).
    pub codecs: Vec<String>,
    /// Why it cannot decode anything else ("no HEVC decoder here").
    pub why_not: String,
    /// What it calls itself in messages.
    pub label: String,
}

impl ScriptedPlatform {
    /// A platform that decodes `codecs`.
    pub fn decoding(codecs: &[&str]) -> Self {
        Self {
            codecs: codecs.iter().map(|c| c.to_string()).collect(),
            why_not: "nothing here decodes that".into(),
            label: "TestKit".into(),
        }
    }

    /// A platform that decodes nothing, for `why`.
    pub fn refusing(why: &str) -> Self {
        Self { codecs: Vec::new(), why_not: why.into(), label: "TestKit".into() }
    }
}

impl PlatformVideo for ScriptedPlatform {
    fn name(&self) -> &str {
        &self.label
    }

    fn supports(&self, info: &StreamInfo) -> PlatformSupport {
        if self.codecs.iter().any(|c| *c == info.codec) {
            PlatformSupport::Yes
        } else {
            PlatformSupport::No(self.why_not.clone())
        }
    }

    fn open(&self, info: &StreamInfo) -> Result<Box<dyn VideoDecoder>> {
        let v = info.video.ok_or_else(|| Error::Invalid("no video info".into()))?;
        Ok(Box::new(GrayDecoder { w: v.width.max(2) & !1, h: v.height.max(2) & !1, ready: Vec::new() }))
    }
}

struct GrayDecoder {
    w: u32,
    h: u32,
    ready: Vec<i64>,
}

impl VideoDecoder for GrayDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        self.ready.push(packet.pts);
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Option<VideoFrame>> {
        if self.ready.is_empty() {
            return Ok(None);
        }
        let pts = self.ready.remove(0);
        let (w, h) = (self.w as usize, self.h as usize);
        Ok(Some(VideoFrame {
            width: self.w,
            height: self.h,
            format: PixelFormat::Yuv420p8,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            planes: [vec![110; w * h], vec![128; w * h / 4], vec![128; w * h / 4]],
            strides: [w, w / 2, w / 2],
            pts,
        }))
    }

    fn flush(&mut self) {
        self.ready.clear();
    }
}
