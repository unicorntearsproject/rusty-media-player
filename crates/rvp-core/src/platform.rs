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

/// The bit depth an `avcC` record declares for luma (8 when the record does not say: only the High profiles above 8-bit carry it).
pub fn avcc_bit_depth(avcc: &[u8]) -> u8 {
    // configurationVersion, profile, compatibility, level, lengthSize, then the sets of parameter sets.
    if avcc.len() < 7 || !matches!(avcc[1], 100 | 110 | 122 | 144 | 244) {
        return 8;
    }
    let mut at = 5;
    let sps_count = (avcc[at] & 0x1f) as usize;
    at += 1;
    for _ in 0..sps_count {
        let Some(n) = avcc.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize) else {
            return 8;
        };
        at += 2 + n;
    }
    let Some(pps_count) = avcc.get(at).map(|b| *b as usize) else { return 8 };
    at += 1;
    for _ in 0..pps_count {
        let Some(n) = avcc.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize) else {
            return 8;
        };
        at += 2 + n;
    }
    // chroma_format (6 reserved bits + 2), bit_depth_luma_minus8 (5 reserved + 3).
    avcc.get(at + 1).map_or(8, |b| 8 + (b & 7))
}

/// A stream our own decoders cannot take though they would open it: 10-bit H.264 (its parameter sets say so up front, so the choice
/// is made before any decoder, and any thread, is built). `None` for everything else.
pub fn ours_refuses(info: &StreamInfo) -> Option<Error> {
    (info.codec == "h264" && avcc_bit_depth(&info.extra_data) > 8)
        .then(|| Error::Unsupported(String::from("bit depth above 8 or lossless coding")))
}

/// The RFC 6381 codec string of a stream from its configuration record (`hvc1.1.6.L93.B0`, `avc1.6e001f`), as WebCodecs and the
/// system decoders name a configuration. `None` when the record is missing or too short.
pub fn codec_string(info: &StreamInfo) -> Option<String> {
    let x = &info.extra_data;
    match info.codec.as_str() {
        "hevc" => {
            // hvcC: version, space/tier/profile, 32 compatibility flags, 48 constraint flags, level.
            if x.len() < 13 {
                return None;
            }
            let space = match x[1] >> 6 {
                1 => "A",
                2 => "B",
                3 => "C",
                _ => "",
            };
            let (tier, profile) = (if x[1] & 0x20 != 0 { 'H' } else { 'L' }, x[1] & 0x1f);
            let compat = u32::from_be_bytes([x[2], x[3], x[4], x[5]]).reverse_bits();
            let mut out = format!("hvc1.{space}{profile}.{compat:x}.{tier}{}", x[12]);
            let mut constraints: Vec<u8> = x[6..12].to_vec();
            while constraints.last() == Some(&0) {
                constraints.pop();
            }
            if constraints.is_empty() {
                constraints.push(0);
            }
            for b in constraints {
                out.push_str(&format!(".{b:X}"));
            }
            Some(out)
        }
        "h264" if x.len() >= 4 => Some(format!("avc1.{:02x}{:02x}{:02x}", x[1], x[2], x[3])),
        _ => None,
    }
}

/// The profile of an HEVC stream (`general_profile_idc`: 1 Main, 2 Main 10, 3 Main Still Picture), from its `hvcC`.
pub fn hevc_profile(info: &StreamInfo) -> Option<u8> {
    (info.codec == "hevc" && info.extra_data.len() >= 13).then(|| info.extra_data[1] & 0x1f)
}

/// The text a failed handoff adds to an `Unsupported` message: `video codec `hevc` [WebCodecs: no HEVC decoder here]`. The application
/// reads the square brackets to say what the user can do.
pub fn with_platform_reason(message: &str, platform: &str, why: &str) -> String {
    format!("{message} [{platform}: {why}]")
}

/// `ours` unless the stream is one of ours-refuses (see [`ours_refuses`]), in which case the refusal.
pub fn screened(
    ours: impl FnOnce() -> Result<Box<dyn VideoDecoder>>,
    info: &StreamInfo,
) -> Result<Box<dyn VideoDecoder>> {
    match ours_refuses(info) {
        Some(e) => Err(e),
        None => ours(),
    }
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
    fn codec_strings_follow_rfc_6381() {
        let mut i = info("hevc");
        // Main profile, level 4.0 (120), compat flags 0x60000000, constraints 0x90 00 00 00 00 00.
        i.extra_data = vec![1, 0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 120];
        assert_eq!(codec_string(&i).unwrap(), "hvc1.1.6.L120.90");
        assert_eq!(hevc_profile(&i), Some(1));
        // Main 10, high tier, level 5.1 (153), no constraint bits.
        i.extra_data = vec![1, 0x22, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 153];
        assert_eq!(codec_string(&i).unwrap(), "hvc1.2.4.H153.0");
        i.extra_data.truncate(5);
        assert_eq!(codec_string(&i), None);
        let mut a = info("h264");
        a.extra_data = vec![1, 0x6e, 0x00, 0x1f, 0xff];
        assert_eq!(codec_string(&a).unwrap(), "avc1.6e001f");
        assert_eq!(codec_string(&info("vp9")), None);
    }

    #[test]
    fn the_avcc_record_says_the_bit_depth_of_a_high_profile_stream() {
        // version 1, High 4:4:4... profile 110 (High 10), compat, level 31, 4-byte lengths, one SPS of 3 bytes, one PPS of 2, then
        // chroma_format 1 (4:2:0), bit_depth_luma_minus8 2, bit_depth_chroma_minus8 2, no extended SPS.
        let mut r = vec![1u8, 110, 0, 31, 0xff, 0xe1, 0, 3, 0x67, 0x6e, 0x1f, 1, 0, 2, 0x68, 0xee];
        r.extend_from_slice(&[0xfc | 1, 0xf8 | 2, 0xf8 | 2, 0]);
        assert_eq!(avcc_bit_depth(&r), 10);
        // The same record for an 8-bit High stream, a Main profile record (which has no such fields), and junk.
        let mut r8 = r.clone();
        let n = r8.len();
        r8[n - 3] = 0xf8;
        assert_eq!(avcc_bit_depth(&r8), 8);
        let main = vec![1u8, 77, 0, 31, 0xff, 0xe1, 0, 3, 0x67, 0x4d, 0x1f, 1, 0, 2, 0x68, 0xee];
        assert_eq!(avcc_bit_depth(&main), 8);
        assert_eq!(avcc_bit_depth(&[1, 110]), 8);
        assert_eq!(avcc_bit_depth(&r[..12]), 8, "a truncated record");
        let mut i = info("h264");
        i.extra_data = r;
        assert!(matches!(ours_refuses(&i), Some(Error::Unsupported(_))));
        i.extra_data = main;
        assert!(ours_refuses(&i).is_none());
        assert!(ours_refuses(&info("hevc")).is_none(), "only 10-bit H.264 is pre-screened");
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
