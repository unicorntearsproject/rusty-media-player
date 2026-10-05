//! Browser host: File API `Source`, WebAudio sink (AudioWorklet, with a ScriptProcessor fallback in
//! `web/audio.js`), `<canvas>` surface, DOM input forwarded by `web/main.js`, `requestAnimationFrame` ticks.
//!
//! The page owns the DOM and the audio device; this crate owns everything else: it blits the pixel surface
//! that [`rvp_app::App`] composes (picture and UI), reads the dropped file in chunks, and forwards input.
//! On non-wasm targets this crate is empty so the workspace builds everywhere.
#![cfg_attr(not(target_arch = "wasm32"), allow(unused))]

/// Name of the host, shown in diagnostics.
pub const HOST_NAME: &str = "web";

#[cfg(target_arch = "wasm32")]
mod audio;
#[cfg(target_arch = "wasm32")]
mod host;
#[cfg(target_arch = "wasm32")]
mod media;
#[cfg(target_arch = "wasm32")]
mod player;
#[cfg(target_arch = "wasm32")]
mod source;
#[cfg(target_arch = "wasm32")]
mod threads;

#[cfg(target_arch = "wasm32")]
pub use player::WebPlayer;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::prelude::*;

    /// Version of the wasm module, callable from JS.
    #[wasm_bindgen]
    pub fn rvp_version() -> String {
        env!("CARGO_PKG_VERSION").into()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn host_name() {
        assert_eq!(super::HOST_NAME, "web");
    }
}
