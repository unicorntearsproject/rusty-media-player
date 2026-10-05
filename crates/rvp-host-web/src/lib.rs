//! Browser host: File API `Source`, WebAudio sink, `<canvas>` surface, DOM input, requestAnimationFrame.
//! Milestone 5. On non-wasm targets this crate is empty so the workspace builds everywhere.
#![cfg_attr(not(target_arch = "wasm32"), allow(unused))]

/// Name of the host, shown in diagnostics.
pub const HOST_NAME: &str = "web";

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
