# Patches to rav1d 1.1.0

Upstream: https://github.com/memorysafety/rav1d (BSD-2-Clause, see `COPYING`). Vendored for
rusty-video-player so that it builds for `wasm32-unknown-unknown` (docs/PLAN.md risk R1).

1. `lib.rs`: added a private `libc` module with the six type aliases and seven errno constants rav1d takes from
   the `libc` crate (`ptrdiff_t`, `intptr_t`, `uintptr_t`, `off_t`, `ENOENT`, `EIO`, `EAGAIN`, `ENOMEM`,
   `EINVAL`, `ERANGE`, `ENOPROTOOPT`); every `libc::` path in the sources became `crate::libc::`. The `libc`
   crate has none of these on wasm32-unknown-unknown, which was the only thing stopping the build.
2. `Cargo.toml`: rewritten as a standalone manifest: no `libc`, `raw-cpuid` or build dependencies, no `asm`
   features (the assembly needs `nasm`/`cc` and does not apply to wasm), no `staticlib`.
3. `lib.rs`: `#![allow(warnings)]` to keep builds quiet.
4. Removed `build.rs`, `rust-toolchain.toml` (it pinned an old nightly), `Cargo.lock`, `retranspile.sh`.

Everything else is unchanged. Use it with `n_threads = 1` and `max_frame_delay = 1` (no threads on wasm).
Upstreaming the `libc` change is the preferred end state.

## Threads (M9)

5. `src/lib.rs`: `THREAD_SPAWN` / `set_thread_spawn`: a hook that starts the worker threads (a Web Worker on wasm32 with
   shared memory, where `std::thread::spawn` is unsupported). Unset, `std::thread` is used as before. The workers no
   longer park until the opener unparks them (that needed a `std::thread::Thread` handle, which a foreign thread cannot
   give us); they wait for `thread_data.c` to be set instead, and `Rav1dContextTaskType::Worker` holds no handle.
6. `Cargo.toml`: `parking_lot_core` with the `nightly` feature, which selects its wasm32 `memory.atomic.wait32` thread
   parker (without it a worker thread would panic when it first waits on a lock).
   `rvp-codec-av1` opens the context with `n_threads` and frame delay set from the host's thread count (the single
   threaded setting is kept where there are no threads).
7. `src/decode.rs` (`rav1d_submit_frame`): a frame arriving without a sequence or frame header (damaged stream) is dropped with
   `EINVAL` instead of unwrapping `None` (a panic, which aborts the process on wasm32 and in release builds); `on_error` no
   longer unwraps the frame header either. Found by the corrupt-file test in `rvp-host-headless/tests/hardening.rs`.
8. `src/lib.rs`: `dav1d_picture_unref` leaves the picture all-zero (it used to convert an empty Rust picture back, which allocated an
   `itut_t35` `Arc` that no caller released: one small leak per picture), and `dav1d_get_picture` returns an all-zero
   picture when there is none (`EAGAIN`) for the same reason. Found by the AV1 fuzz target (LeakSanitizer, even on valid input).

## Speed of the portable (no assembly) code, for single-threaded wasm (post M10)

The decoder runs the Rust fallbacks of the DSP functions (there is no assembly on wasm32). Profiling a 1080p30 stream in the browser
showed time going to things that have nothing to do with the arithmetic. None of these changes the decoded pictures (the 1800
frames of the perf stream hash the same before and after, natively and in wasm, and `cargo xtask wasm-smoke` still agrees); each
only does the same integer sums with less overhead:

9. `src/mc.rs`: `put_8tap_rust`, `prep_8tap_rust`. The two-pass filters kept their intermediate rows in a `[[i16; 128]; 135]` that was
   zeroed for every block (34 KB, for an 8 x 8 block that needs 15 x 8 values). They use `MidRows`, which is not cleared and is only
   read where it has been written (`unsafe`, with the reason beside it). The filters themselves work on whole rows: the taps are summed
   eight outputs at a time over row slices (`acc_h8`, `acc_v8_pix`, `acc_v8_mid`), which the compiler vectorises, instead of one pixel
   at a time through the bounds-checked picture accessor.
10. `src/mc.rs`: `avg_rust`, `w_avg_rust`, `mask_rust` loop over zipped slices (no index arithmetic and checks per pixel).
11. `src/msac.rs`: `EcWin` is `u64` instead of `usize`: on wasm32 the arithmetic decoder's bit window was 32 bits and refilled from the
    stream twice as often. Any width decodes the same symbols.
12. `src/cdef.rs`: `cdef_filter_block_rust` is generic over the block size (`W`, `H`), so the loops over a block's pixels have known
    lengths. `src/itx.rs`: `inv_txfm_add` clears only the `w * h` intermediate values it uses, not all 64 x 64.
