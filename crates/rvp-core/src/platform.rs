//! The platform decoder handoff: a host may offer to decode codecs this player does not decode itself (HEVC first), through the
//! operating system or the browser. Our own decoders always come first; the platform is asked only when ours answers `Unsupported`.
//!
//! The pieces: [`PlatformVideo`] is what a host implements; [`open_video`] is what a [`crate::CodecFactory`] calls to build a decoder
//! (ours, else the platform's, else an error that says why); [`FallbackVideo`] covers the codecs whose support is only known from the
//! first packet (10-bit H.264: the decoder opens fine and refuses the stream's parameter sets).
use crate::codec::VideoDecoder;
use crate::error::{Error, Result};
use crate::media::{Packet, StreamInfo, VideoFrame};
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

/// Whether the platform can decode a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformSupport {
    /// It can.
    Yes,
    /// It cannot; the text says why in a few words, for the message the user reads ("the Windows HEVC Video Extensions are not
    /// installed", "this browser has no HEVC decoder").
    No(String),
}

/// A decoder service of the host's platform (WebCodecs, VideoToolbox, Media Foundation, VA-API).
pub trait PlatformVideo {
    /// A short name for messages: "WebCodecs", "VideoToolbox", "Media Foundation", "VA-API".
    fn name(&self) -> &str;
    /// Whether this platform can decode `info` (codec, profile and size are in `info`; the codec configuration record is
    /// `info.extra_data`). Must be cheap and must not block for long.
    fn supports(&self, info: &StreamInfo) -> PlatformSupport;
    /// A decoder for `info`. Frames come out in presentation order as 8-bit or 10-bit 4:2:0 (see [`crate::PixelFormat`]); HDR is
    /// already mapped to SDR.
    fn open(&self, info: &StreamInfo) -> Result<Box<dyn VideoDecoder>>;
}

/// The codecs a host may leave to its platform. Anything else is never offered there.
pub fn platform_codec(codec: &str) -> bool {
    matches!(codec, "hevc" | "h264")
}

/// The text a failed handoff adds to an `Unsupported` message: `video codec `hevc` [WebCodecs: no HEVC decoder here]`. The application
/// reads the square brackets to say what the user can do.
pub fn with_platform_reason(message: &str, platform: &str, why: &str) -> String {
    format!("{message} [{platform}: {why}]")
}

/// Build the video decoder for `info`: `ours` (the result of the codec crates), else the platform's when ours does not know the codec,
/// else an error that names the platform's reason.
pub fn open_video(
    ours: Result<Box<dyn VideoDecoder>>,
    platform: Option<&Rc<dyn PlatformVideo>>,
    info: &StreamInfo,
) -> Result<Box<dyn VideoDecoder>> {
    let message = match ours {
        Err(Error::Unsupported(m)) => m,
        other => return other,
    };
    let Some(p) = platform.filter(|_| platform_codec(&info.codec)) else {
        return Err(Error::Unsupported(message));
    };
    match p.supports(info) {
        PlatformSupport::Yes => p.open(info),
        PlatformSupport::No(why) => Err(Error::Unsupported(with_platform_reason(&message, p.name(), &why))),
    }
}

/// Our decoder with the platform's behind it: the first stream the inner decoder refuses with `Unsupported` moves to the platform
/// (the refused packet is sent again there), and the stream stays there. Without a platform that accepts it, the error goes on.
pub struct FallbackVideo {
    ours: Box<dyn VideoDecoder>,
    platform: Option<Rc<dyn PlatformVideo>>,
    info: StreamInfo,
    theirs: Option<Box<dyn VideoDecoder>>,
    /// The packets since the last key frame, so a switch in the middle of a group can start the platform decoder cleanly.
    since_key: Vec<Packet>,
}

impl FallbackVideo {
    /// Wrap `ours`; `platform` is used only for what `ours` refuses.
    pub fn new(
        ours: Box<dyn VideoDecoder>,
        platform: Option<Rc<dyn PlatformVideo>>,
        info: StreamInfo,
    ) -> Self {
        Self { ours, platform, info, theirs: None, since_key: Vec::new() }
    }

    fn current(&mut self) -> &mut dyn VideoDecoder {
        match self.theirs.as_mut() {
            Some(t) => t.as_mut(),
            None => self.ours.as_mut(),
        }
    }
}

impl VideoDecoder for FallbackVideo {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if self.platform.is_some() && self.theirs.is_none() {
            if packet.keyframe {
                self.since_key.clear();
            }
            if self.since_key.len() < 256 {
                self.since_key.push(packet.clone());
            }
        }
        let r = self.current().send_packet(packet);
        let Err(Error::Unsupported(message)) = &r else { return r };
        let (Some(p), None) =
            (self.platform.clone().filter(|_| platform_codec(&self.info.codec)), &self.theirs)
        else {
            return r;
        };
        match p.supports(&self.info) {
            PlatformSupport::Yes => {
                let mut d = p.open(&self.info)?;
                for q in &self.since_key {
                    d.send_packet(q)?;
                }
                self.since_key = Vec::new();
                self.theirs = Some(d);
                Ok(())
            }
            PlatformSupport::No(why) => {
                Err(Error::Unsupported(with_platform_reason(message, p.name(), &why)))
            }
        }
    }

    fn receive_frame(&mut self) -> Result<Option<VideoFrame>> {
        self.current().receive_frame()
    }

    fn flush(&mut self) {
        self.since_key.clear();
        self.current().flush()
    }

    fn drain(&mut self) -> Result<()> {
        self.current().drain()
    }

    fn pending(&self) -> usize {
        match &self.theirs {
            Some(t) => t.pending(),
            None => self.ours.pending(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{ColorMatrix, ColorRange, PixelFormat, VideoInfo};
    use crate::time::Rational;
    use alloc::string::ToString;
    use alloc::vec;

    fn info(codec: &str) -> StreamInfo {
        StreamInfo {
            id: 1,
            kind: crate::StreamKind::Video,
            codec: codec.to_string(),
            time_base: Rational { num: 1, den: 1000 },
            language: None,
            extra_data: Vec::new(),
            video: Some(VideoInfo { width: 16, height: 16 }),
            audio: None,
            duration_us: None,
        }
    }

    fn pkt(key: bool) -> Packet {
        Packet { stream_id: 1, pts: 0, dts: 0, duration: 0, keyframe: key, discard_end_us: 0, data: vec![1] }
    }

    /// Decodes everything to a gray frame.
    struct Gray(Option<u8>);
    impl VideoDecoder for Gray {
        fn send_packet(&mut self, _: &Packet) -> Result<()> {
            self.0 = Some(128);
            Ok(())
        }
        fn receive_frame(&mut self) -> Result<Option<VideoFrame>> {
            Ok(self.0.take().map(|_| VideoFrame {
                width: 2,
                height: 2,
                format: PixelFormat::Yuv420p8,
                matrix: ColorMatrix::Bt709,
                range: ColorRange::Limited,
                planes: [vec![128; 4], vec![128], vec![128]],
                strides: [2, 1, 1],
                pts: 0,
            }))
        }
        fn flush(&mut self) {}
    }

    /// Refuses every packet.
    struct Refuses;
    impl VideoDecoder for Refuses {
        fn send_packet(&mut self, _: &Packet) -> Result<()> {
            Err(Error::Unsupported("bit depth above 8".to_string()))
        }
        fn receive_frame(&mut self) -> Result<Option<VideoFrame>> {
            Ok(None)
        }
        fn flush(&mut self) {}
    }

    struct Plat(PlatformSupport);
    impl PlatformVideo for Plat {
        fn name(&self) -> &str {
            "TestKit"
        }
        fn supports(&self, _: &StreamInfo) -> PlatformSupport {
            self.0.clone()
        }
        fn open(&self, _: &StreamInfo) -> Result<Box<dyn VideoDecoder>> {
            Ok(Box::new(Gray(None)))
        }
    }

    #[test]
    fn ours_comes_first_and_the_platform_only_gets_what_ours_refuses() {
        let yes: Rc<dyn PlatformVideo> = Rc::new(Plat(PlatformSupport::Yes));
        let ours: Result<Box<dyn VideoDecoder>> = Ok(Box::new(Refuses));
        assert!(
            open_video(ours, Some(&yes), &info("h264")).is_ok(),
            "ours opened: the platform is not asked"
        );
        let none: Result<Box<dyn VideoDecoder>> = Err(Error::Unsupported("video codec `hevc`".to_string()));
        let mut d = open_video(none, Some(&yes), &info("hevc")).unwrap();
        d.send_packet(&pkt(true)).unwrap();
        assert!(d.receive_frame().unwrap().is_some());
    }

    #[test]
    fn a_refusal_names_the_platform_and_the_reason() {
        let no: Rc<dyn PlatformVideo> =
            Rc::new(Plat(PlatformSupport::No("no HEVC decoder here".to_string())));
        let none: Result<Box<dyn VideoDecoder>> = Err(Error::Unsupported("video codec `hevc`".to_string()));
        let Err(Error::Unsupported(m)) = open_video(none, Some(&no), &info("hevc")) else { panic!() };
        assert_eq!(m, "video codec `hevc` [TestKit: no HEVC decoder here]");
        // No platform at all, or a codec the platform is never offered: the plain message.
        let none: Result<Box<dyn VideoDecoder>> = Err(Error::Unsupported("video codec `wmv3`".to_string()));
        let Err(Error::Unsupported(m)) = open_video(none, Some(&no), &info("wmv3")) else { panic!() };
        assert_eq!(m, "video codec `wmv3`");
        let none: Result<Box<dyn VideoDecoder>> = Err(Error::Unsupported("video codec `hevc`".to_string()));
        assert!(
            matches!(open_video(none, None, &info("hevc")), Err(Error::Unsupported(m)) if m == "video codec `hevc`")
        );
        // Another error is not turned into a handoff.
        let yes: Rc<dyn PlatformVideo> = Rc::new(Plat(PlatformSupport::Yes));
        let bad: Result<Box<dyn VideoDecoder>> = Err(Error::Invalid("x".to_string()));
        assert!(matches!(open_video(bad, Some(&yes), &info("hevc")), Err(Error::Invalid(_))));
    }

    #[test]
    fn a_stream_ours_refuses_on_the_first_packet_moves_to_the_platform() {
        let yes: Rc<dyn PlatformVideo> = Rc::new(Plat(PlatformSupport::Yes));
        let mut d = FallbackVideo::new(Box::new(Refuses), Some(yes.clone()), info("h264"));
        d.send_packet(&pkt(true)).unwrap();
        assert!(d.receive_frame().unwrap().is_some(), "the refused packet was decoded by the platform");
        d.send_packet(&pkt(false)).unwrap();
        // Without a platform the refusal goes on; with one that says no, it carries the reason.
        let mut d = FallbackVideo::new(Box::new(Refuses), None, info("h264"));
        assert_eq!(d.send_packet(&pkt(true)), Err(Error::Unsupported("bit depth above 8".to_string())));
        let no: Rc<dyn PlatformVideo> = Rc::new(Plat(PlatformSupport::No("not installed".to_string())));
        let mut d = FallbackVideo::new(Box::new(Refuses), Some(no.clone()), info("h264"));
        assert_eq!(
            d.send_packet(&pkt(true)),
            Err(Error::Unsupported("bit depth above 8 [TestKit: not installed]".to_string()))
        );
    }
}
