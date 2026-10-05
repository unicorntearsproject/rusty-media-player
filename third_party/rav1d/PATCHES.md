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
