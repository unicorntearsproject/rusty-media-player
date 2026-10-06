//! 128-bit vector operations for the pixel kernels (colour conversion, picture scaling, H.264 prediction and deblocking), written once against
//! the names of WebAssembly SIMD128 (`i16x8_add`, `u8x16_avgr`, `v128_bitselect` ...) and available as:
//!
//! * the WebAssembly intrinsics themselves, when the target has `simd128` (`-C target-feature=+simd128`, which `cargo xtask web` sets);
//! * an SSE2 layer with the same names and the same lane-for-lane results, on x86_64 (SSE2 is part of that baseline: no run-time check).
//!
//! Every kernel using these keeps a scalar twin that defines the exact result; the tests compare the two (inside WebAssembly with
//! `cargo xtask wasm-smoke`, natively as ordinary unit tests). Other targets (aarch64) keep the scalar kernels for now.
#![cfg(any(all(target_arch = "wasm32", target_feature = "simd128"), target_arch = "x86_64"))]

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
mod wasm;
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
pub use wasm::*;

#[cfg(target_arch = "x86_64")]
mod x86;
#[cfg(target_arch = "x86_64")]
pub use x86::*;
