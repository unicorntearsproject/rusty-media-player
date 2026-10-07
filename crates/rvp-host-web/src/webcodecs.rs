//! The platform video decoder of the browser: a thin Rust side over `web/webcodecs.js` (WebCodecs). HEVC (Main and Main 10) and 10-bit
//! H.264 go through it when the browser has a decoder for them; everything else stays with our own decoders.
use rvp_core::{
    Error, Packet, PixelFormat, PlatformSupport, PlatformVideo, Result as CoreResult, StreamInfo,
    VideoDecoder, VideoFrame, codec_string, hevc_profile,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// The page's platform decoder service (`RvpPlatformVideo`).
    pub type JsPlatformVideo;
    /// 1 supported, 0 not probed yet, -1 not supported.
    #[wasm_bindgen(method)]
    fn can(this: &JsPlatformVideo, key: &str) -> i32;
    /// Why WebCodecs as a whole is unavailable ("this browser has no WebCodecs"), or "".
    #[wasm_bindgen(method)]
    fn reason(this: &JsPlatformVideo) -> String;
    #[wasm_bindgen(method, getter)]
    fn available(this: &JsPlatformVideo) -> bool;
    #[wasm_bindgen(method, catch)]
    fn open(
        this: &JsPlatformVideo,
        codec: &str,
        width: u32,
        height: u32,
        description: &[u8],
    ) -> Result<JsDecoder, JsValue>;

    /// One decoder, for one stream.
    pub type JsDecoder;
    #[wasm_bindgen(method, catch)]
    fn push(this: &JsDecoder, key: bool, pts_us: f64, data: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(method)]
    fn has(this: &JsDecoder) -> bool;
    #[wasm_bindgen(method, js_name = frameWidth)]
    fn frame_width(this: &JsDecoder) -> u32;
    #[wasm_bindgen(method, js_name = frameHeight)]
    fn frame_height(this: &JsDecoder) -> u32;
    #[wasm_bindgen(method, js_name = framePts)]
    fn frame_pts(this: &JsDecoder) -> f64;
    #[wasm_bindgen(method, js_name = frameData)]
    fn frame_data(this: &JsDecoder) -> js_sys::Uint8Array;
    #[wasm_bindgen(method)]
    fn pop(this: &JsDecoder);
    #[wasm_bindgen(method)]
    fn pending(this: &JsDecoder) -> u32;
    #[wasm_bindgen(method)]
    fn drain(this: &JsDecoder);
    #[wasm_bindgen(method)]
    fn reset(this: &JsDecoder);
    #[wasm_bindgen(method)]
    fn close(this: &JsDecoder);
    #[wasm_bindgen(method)]
    fn error(this: &JsDecoder) -> String;
}

/// WebCodecs as a [`PlatformVideo`].
pub struct WebPlatform {
    js: JsPlatformVideo,
}

impl WebPlatform {
    /// Wrap the page's service.
    pub fn new(js: JsPlatformVideo) -> Self {
        Self { js }
    }

    /// The probe key of a stream (what `RvpPlatformVideo.can` answers for), or why it has none.
    fn key(info: &StreamInfo) -> std::result::Result<&'static str, String> {
        match info.codec.as_str() {
            "hevc" => match hevc_profile(info) {
                Some(1) => Ok("hevc-main"),
                Some(2) => Ok("hevc-main10"),
                Some(p) => Err(format!("this HEVC profile ({p}) isn't supported")),
                None => Err("the HEVC stream has no usable configuration".into()),
            },
            "h264" => Ok("h264-high10"),
            other => Err(format!("{other} isn't offered to WebCodecs")),
        }
    }
}

impl PlatformVideo for WebPlatform {
    fn name(&self) -> &str {
        "WebCodecs"
    }

    fn supports(&self, info: &StreamInfo) -> PlatformSupport {
        if !self.js.available() {
            let why = self.js.reason();
            return PlatformSupport::No(if why.is_empty() {
                "this browser has no WebCodecs".into()
            } else {
                why
            });
        }
        let key = match Self::key(info) {
            Ok(k) => k,
            Err(why) => return PlatformSupport::No(why),
        };
        if codec_string(info).is_none() {
            return PlatformSupport::No("the stream has no codec configuration".into());
        }
        match self.js.can(key) {
            1 => PlatformSupport::Yes,
            0 => PlatformSupport::No(
                "this browser is still checking what it can decode; try again in a moment".into(),
            ),
            _ => PlatformSupport::No(match info.codec.as_str() {
                "hevc" => "this browser has no HEVC decoder".into(),
                _ => "this browser can't decode 10-bit H.264".into(),
            }),
        }
    }

    fn open(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        let codec = codec_string(info).ok_or_else(|| Error::Invalid("no codec configuration".into()))?;
        let v = info.video.ok_or_else(|| Error::Invalid("no video info".into()))?;
        let js = self.js.open(&codec, v.width, v.height, &info.extra_data).map_err(|e| {
            Error::Unsupported(format!("video codec `{}` [WebCodecs: {}]", info.codec, js_text(&e)))
        })?;
        Ok(Box::new(WebDecoder { js, failed: false }))
    }
}

fn js_text(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::Reflect::get(e, &"message".into()).ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| "unknown error".into())
}

struct WebDecoder {
    js: JsDecoder,
    failed: bool,
}

impl VideoDecoder for WebDecoder {
    fn send_packet(&mut self, packet: &Packet) -> CoreResult<()> {
        let e = self.js.error();
        if !e.is_empty() {
            self.failed = true;
        }
        if self.failed {
            return Err(Error::Unsupported(format!(
                "video codec [WebCodecs: {}]",
                if e.is_empty() { "the decoder stopped".into() } else { e }
            )));
        }
        self.js
            .push(packet.keyframe, packet.pts as f64, &packet.data)
            .map_err(|e| Error::Invalid(format!("WebCodecs refused a packet: {}", js_text(&e))))
    }

    fn receive_frame(&mut self) -> CoreResult<Option<VideoFrame>> {
        if !self.js.has() {
            return Ok(None);
        }
        let (w, h) = (self.js.frame_width(), self.js.frame_height());
        let data = self.js.frame_data().to_vec();
        let pts = self.js.frame_pts() as i64;
        self.js.pop();
        Ok(Some(VideoFrame {
            width: w,
            height: h,
            format: PixelFormat::Rgba8,
            matrix: rvp_core::ColorMatrix::Bt709,
            range: rvp_core::ColorRange::Full,
            planes: [data, Vec::new(), Vec::new()],
            strides: [w as usize * 4, 0, 0],
            pts,
        }))
    }

    fn flush(&mut self) {
        self.js.reset();
    }

    fn drain(&mut self) -> CoreResult<()> {
        self.js.drain();
        Ok(())
    }

    fn pending(&self) -> usize {
        self.js.pending() as usize
    }
}

impl Drop for WebDecoder {
    fn drop(&mut self) {
        self.js.close();
    }
}
