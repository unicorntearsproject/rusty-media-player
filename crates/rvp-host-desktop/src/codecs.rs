//! The decoders of the desktop build, and the worker pool.
//!
//! Video decodes on a thread of its own (so the window never waits for a frame), H.264 is split into a parsing side and a
//! reconstruction side on two threads, and the pixel kernels spread over a pool; the UI thread never blocks on any of it.
use rvp_core::{AudioDecoder, CodecFactory, Error, Result as CoreResult, StreamInfo, VideoDecoder};

/// The decoders linked into `rusty-wave`.
pub struct DesktopCodecs;

impl CodecFactory for DesktopCodecs {
    fn audio(&self, info: &StreamInfo) -> CoreResult<Box<dyn AudioDecoder>> {
        rvp_codec_audio::audio_decoder(info)
    }

    fn video_light(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        // Posters: decoded on the calling thread, one picture at a time (the pooled decoders hold a lot of memory for as long as they live).
        match info.codec.as_str() {
            "av1" => rvp_codec_av1::av1_decoder_light(info),
            "h264" => rvp_codec_h264::h264_decoder_with(info, None, None),
            _ => build_video(info),
        }
    }

    fn video(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        if matches!(info.codec.as_str(), "av1" | "h264" | "vp9") {
            let info = info.clone();
            return Ok(Box::new(rvp_par::ThreadedVideoDecoder::new(Box::new(move || build_video(&info)))));
        }
        build_video(info)
    }
}

fn build_video(info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
    match info.codec.as_str() {
        "av1" => rvp_codec_av1::av1_decoder(info),
        "h264" => rvp_par::h264::h264_pipelined(info),
        "vp9" => rvp_codec_vp9::vp9_decoder(info),
        other => Err(Error::Unsupported(format!("video codec `{other}`"))),
    }
}

/// Start the pool the kernels share (once). Called on the UI thread.
pub fn start_pool() -> usize {
    rvp_par::mark_ui_thread();
    let hw = std::thread::available_parallelism().map_or(2, |n| n.get());
    let size = rvp_par::pool_size(hw);
    rvp_par::Pool::new(size).install();
    size
}
