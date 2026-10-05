# Patches to rusty_vp9 0.1.1

Upstream: https://github.com/Remade-With-Rust/remade_ffmpeg_rs (Apache-2.0, see `LICENSE`). Vendored for rusty-video-player
(docs/PLAN.md M9) to add WebAssembly SIMD128 kernels; the decoder is otherwise unchanged and still bit-exact.

1. `src/wasm_simd.rs` (new): 8-tap motion-compensation filters (`predict_block_interior`, with the same edge-replicating tile
   the AVX2 path uses) and the loop filter's edge kernel (`filter_edge8`: eight positions per vector of 16-bit lanes,
   transposes for vertical edges), plus `selftest`, which compares them with the scalar code on random data inside
   WebAssembly (`cargo xtask wasm-smoke` runs it in Node). Only compiled for `wasm32` with `simd128`.
2. `src/inter.rs`: `predict_block` is now a wrapper that uses the wasm kernels when compiled for SIMD128 and otherwise calls
   `predict_block_scalar` (the previous function, unchanged).
3. `src/loopfilter.rs`: `filter_edge8` dispatches to the wasm kernel; the scalar body is `filter_edge8_scalar`.
4. `src/lib.rs`: `pub fn simd_selftest()`, `mod wasm_simd`, `#![allow(warnings)]` for the vendored code.
5. `Cargo.toml` rewritten as a standalone manifest; no dependencies.
