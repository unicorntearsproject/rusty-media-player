//! Safe slice load/store helpers for WebAssembly SIMD128, used by the pixel kernels in this and the codec crates.
//!
//! Only compiled when the target has `simd128` enabled (`-C target-feature=+simd128`, which `cargo xtask web` sets).
//! Every kernel using these keeps a scalar twin that defines the exact result; the `selftest` functions in each
//! crate compare the two inside WebAssembly (`cargo xtask wasm-smoke` runs them in Node).
#![cfg(all(target_arch = "wasm32", target_feature = "simd128"))]

pub use core::arch::wasm32::*;

/// Load 16 bytes from the start of `s` (panics if `s` is shorter).
#[inline(always)]
pub fn load(s: &[u8]) -> v128 {
    let s = &s[..16];
    // SAFETY: `s` has exactly 16 readable bytes, and `v128_load` has no alignment requirement.
    unsafe { v128_load(s.as_ptr().cast()) }
}

/// Load 8 bytes into the low half (the high half is zero).
#[inline(always)]
pub fn load8(s: &[u8]) -> v128 {
    let s = &s[..8];
    // SAFETY: 8 readable bytes.
    unsafe { v128_load64_zero(s.as_ptr().cast()) }
}

/// Load 4 bytes into the low lane (the rest is zero).
#[inline(always)]
pub fn load4(s: &[u8]) -> v128 {
    let s = &s[..4];
    // SAFETY: 4 readable bytes.
    unsafe { v128_load32_zero(s.as_ptr().cast()) }
}

/// Store all 16 bytes to the start of `s`.
#[inline(always)]
pub fn store(s: &mut [u8], v: v128) {
    let s = &mut s[..16];
    // SAFETY: 16 writable bytes.
    unsafe { v128_store(s.as_mut_ptr().cast(), v) }
}

/// Store the low 8 bytes.
#[inline(always)]
pub fn store8(s: &mut [u8], v: v128) {
    let s = &mut s[..8];
    // SAFETY: 8 writable bytes.
    unsafe { v128_store64_lane::<0>(v, s.as_mut_ptr().cast()) }
}

/// Store the low 4 bytes.
#[inline(always)]
pub fn store4(s: &mut [u8], v: v128) {
    let s = &mut s[..4];
    // SAFETY: 4 writable bytes.
    unsafe { v128_store32_lane::<0>(v, s.as_mut_ptr().cast()) }
}

/// Replace the high 8 bytes of `v` with the first 8 bytes of `s`.
#[inline(always)]
pub fn load8_hi(v: v128, s: &[u8]) -> v128 {
    let s = &s[..8];
    // SAFETY: 8 readable bytes.
    unsafe { v128_load64_lane::<1>(v, s.as_ptr().cast()) }
}
