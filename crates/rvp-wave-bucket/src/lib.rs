//! Rusty Wave as a Rusty Bucket app. The module exports `bucket_main` (and, in the threads build, `bucket_thread_start`) and
//! imports the whole `bucket_v0` API; everything else lives in `rvp-host-rb` and the player crates.
//!
//! On native targets this crate is empty, so the workspace builds everywhere. `cargo xtask bucket` builds the wasm module(s) and
//! packages them into `Rusty Wave.bucket`.
#![cfg_attr(not(target_arch = "wasm32"), allow(unused))]

/// The app ID (reverse-DNS, the one the desktop file, AppStream, MPRIS and Flatpak use too).
pub const APP_ID: &str = "io.github.unicorntearsproject.RustyWave";

#[cfg(all(target_arch = "wasm32", feature = "smoke"))]
mod smoke;

#[cfg(target_arch = "wasm32")]
mod module {
    use bucket_v0_sys as sys;
    use rvp_core::{AudioDecoder, CodecFactory, Error, Result as CoreResult, StreamInfo, VideoDecoder};
    use rvp_host_rb::RbPlayer;
    use rvp_ui::UiConfig;
    use std::rc::Rc;

    /// The decoders linked into the module.
    struct RbCodecs;

    impl CodecFactory for RbCodecs {
        fn audio(&self, info: &StreamInfo) -> CoreResult<Box<dyn AudioDecoder>> {
            #[cfg(feature = "codecs")]
            return rvp_codec_audio::audio_decoder(info);
            #[cfg(not(feature = "codecs"))]
            Err(Error::Unsupported(format!("audio codec `{}` (this build has no decoders)", info.codec)))
        }

        fn video(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
            #[cfg(feature = "codecs")]
            {
                // With threads the decoder runs on a thread of its own, so decoding never competes with the UI thread.
                if rvp_par::available() && matches!(info.codec.as_str(), "av1" | "h264" | "hevc" | "vp9") {
                    let info = info.clone();
                    return Ok(Box::new(rvp_par::ThreadedVideoDecoder::new(Box::new(move || {
                        build_video(&info)
                    }))));
                }
                build_video(info)
            }
            #[cfg(not(feature = "codecs"))]
            Err(Error::Unsupported(format!("video codec `{}` (this build has no decoders)", info.codec)))
        }
    }

    #[cfg(feature = "codecs")]
    fn build_video(info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        match info.codec.as_str() {
            "av1" => rvp_codec_av1::av1_decoder(info),
            "h264" if rvp_par::available() => rvp_par::h264::h264_pipelined(info),
            "h264" => rvp_codec_h264::h264_decoder(info),
            "vp9" => rvp_codec_vp9::vp9_decoder(info),
            "hevc" => rvp_codec_hevc::sw::hevc_decoder(info),
            other => Err(Error::Unsupported(format!("video codec `{other}`"))),
        }
    }

    /// Called once by the OS on the main thread; the return value is the exit status.
    #[unsafe(no_mangle)]
    pub extern "C" fn bucket_main() -> i32 {
        rvp_host_rb::install_panic_hook();
        // The threads build: the main thread needs a TLS block of its own before anything thread-local is touched.
        rvp_host_rb::threads::init_main_thread();
        // The module imports the whole documented API (see `bucket_v0_sys::link_all`), so its link requirements are exactly the
        // documented set; the condition cannot be folded, the call never runs.
        if rvp_host_rb::api::caps() == i64::MIN {
            std::hint::black_box(sys::link_all());
        }
        if rvp_host_rb::api::caps() & sys::caps::THREADS != 0 {
            // SAFETY: no arguments.
            let cpus = unsafe { sys::cpu_count() }.max(1) as usize;
            let max = rvp_host_rb::shared::Limits::read().threads;
            let pool = rvp_host_rb::threads::init(cpus, max);
            rvp_host_rb::api::trace(&format!("threads: pool={pool} cpus={cpus} limit={max}"));
        }
        let mut player = RbPlayer::new(Rc::new(RbCodecs), UiConfig { reduce_motion: false });
        player.run()
    }

    /// Called by the OS before a hot reload: nothing is kept in memory (the queue, the position and the library are in the
    /// key-value store), so only the queued stores are made durable. Returns 0: no state.
    #[unsafe(no_mangle)]
    pub extern "C" fn bucket_save_state(_buf: *mut u8, _cap: i32) -> i32 {
        rvp_host_rb::save_state_hook()
    }

    /// Called by the OS in the new instance of a hot reload, before `bucket_main`, with what `bucket_save_state` returned. There is
    /// nothing (the queue, the position and the library are in the key-value store), but a module that exports the save without the
    /// restore makes the Simulator note that it drops state, so the pair is complete. Returns 0.
    #[unsafe(no_mangle)]
    pub extern "C" fn bucket_restore_state(_ptr: *const u8, _len: i32) -> i32 {
        0
    }

    /// The first code of every thread but the main one (the threads build only).
    #[cfg(target_feature = "atomics")]
    #[unsafe(no_mangle)]
    pub extern "C" fn bucket_thread_start(tid: i32, arg: i32) {
        rvp_host_rb::threads::thread_entry(tid, arg);
    }
}
