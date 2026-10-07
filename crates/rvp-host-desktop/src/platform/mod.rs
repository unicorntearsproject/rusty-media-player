//! The system's own video decoders, for what the player does not decode itself (HEVC): VA-API on Linux (opened at run time, so the app
//! needs no VA-API package to start), VideoToolbox on macOS, Media Foundation on Windows. Each is one [`rvp_core::PlatformVideo`]; the
//! application asks it, and says plainly why when it cannot.
use rvp_core::PlatformVideo;
use std::rc::Rc;

#[cfg(target_os = "linux")]
pub mod vaapi;

/// The platform decoder of this system, if it has one the player can use (none is not an error: the message says what to do).
pub fn system_video() -> Option<Rc<dyn PlatformVideo>> {
    #[cfg(target_os = "linux")]
    {
        return Some(Rc::new(vaapi::VaapiPlatform::new()));
    }
    #[allow(unreachable_code)]
    None
}
