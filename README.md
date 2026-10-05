# rusty-video-player

A VLC-inspired media player in Rust that compiles to WebAssembly. It runs in a browser page, headless on
native (for tests), and, once its app ABI exists, as an app inside [Rusty Bucket](../rust-os).

- Containers: MP4, MKV, WebM. Video: H.264 (our own decoder), AV1, VP9. Audio: AAC, MP3, FLAC, Opus, Vorbis.
- A portable `no_std + alloc` core behind a small host trait; no threads required.
- Its own UI, drawn with the Unicorn Tears design-system tokens (`crates/theme`).
- Clean-room: VLC is an architecture reference only; no VLC code is used.

Status: Milestones 0-2 done (plan, scaffold, host/clock/executor, MP4 and Matroska/WebM demuxers). Read [`docs/PLAN.md`](docs/PLAN.md) for the architecture and the
milestone list, and [`CLAUDE.md`](CLAUDE.md) for project rules.

## Layout

| Path | What |
| --- | --- |
| `crates/rvp-core` | types, master clock, ring buffer, decoder traits |
| `crates/rvp-host` | host trait set (source, audio, video, surface, input, storage, clock) |
| `crates/rvp-demux` | MP4 and Matroska/WebM demuxers |
| `crates/rvp-codec-*` | audio, H.264, AV1, VP9 decoders |
| `crates/rvp-subs` | SRT and WebVTT |
| `crates/rvp-player` | the engine: scheduler, pipeline, A/V sync, playlist |
| `crates/rvp-ui`, `crates/theme` | the UI and the generated design tokens |
| `crates/rvp-host-{headless,web,rb}` | hosts: native test harness, browser, Rusty Bucket |
| `xtask`, `tools/` | `cargo xtask theme`, `check`, `fixtures` |

## Build and test

The Rust toolchain lives in `~/.cargo/bin`: `source ~/.cargo/env` first.

```sh
cargo test --workspace          # native tests
cargo xtask check               # also checks wasm32-unknown-unknown and no_std (x86_64-unknown-none)
cargo xtask fixtures            # ffmpeg-generated test media into target/fixtures (made on demand by tests)
cargo xtask theme               # regenerate crates/theme/src/tokens.rs from crates/theme/tokens/*.css
cargo xtask theme --sync        # first refresh the CSS snapshot from the design system
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your
option. Third-party components and their licenses are listed in
[`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).
