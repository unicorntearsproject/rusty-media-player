# rusty-video-player

A standalone media app for **audio and video**, written in Rust, with a portable core that compiles to WebAssembly.
One codebase, two editions:

- **Standalone:** an installable web app (PWA) and a native Linux desktop app (Flatpak, AppImage). It never
  requires Rusty Bucket.
- **Rusty Bucket's built-in Media app**, through the `rvp-host-rb` adapter, once its app ABI exists. Nothing in
  the core, UI or app crates depends on [Rusty Bucket](../rust-os); it is just one more host, next to the
  browser, the desktop and the headless test host.

- Containers: MP4, MKV, WebM. Video: H.264 (our own decoder), AV1, VP9. Audio: AAC, MP3, FLAC, Opus, Vorbis.
- A portable `no_std + alloc` core behind a small host trait; no threads required.
- Playlist, gapless playback, subtitles and a host-neutral now-playing model (Media Session in the browser) are in; planned: MPRIS on
  Linux, an audio-first library view with M3U/M3U8/PLS playlists and a visualizer view, and the desktop app and packaging.
- Its own UI, drawn with the Unicorn Tears design-system tokens (`crates/theme`).
- Clean-room: VLC is an architecture reference only; no VLC code is used.

Status: milestones 0 to 8 done.

- M0-M6: scaffold, host/clock/executor, demuxers, audio, AV1, A/V sync, the headless host, the browser host with the themed
  UI, and our own pure-Rust H.264 decoder (Baseline/Main/High, progressive 8-bit 4:2:0, bit-exact with ffmpeg).
- M7: VP9 through `rusty_vp9`, bit-exact with ffmpeg.
- M8: playlist with gapless playback, SRT/WebVTT and embedded subtitles, audio-track switching, pitch-preserving speed,
  exact seek, frame step, A-B loop, chapters, resume positions, the now-playing model (Media Session in the browser)
  and the visualizer tap.

Try it with `cargo xtask web && cargo xtask serve` and open http://127.0.0.1:8080/. Read [`docs/PLAN.md`](docs/PLAN.md)
for the architecture and the milestone list (M9 performance, M10 audio-first view, M11 desktop app and packaging, M12 Rusty
Bucket adapter), [`docs/host-api.md`](docs/host-api.md) for the host-neutral media interfaces, and [`CLAUDE.md`](CLAUDE.md)
for project rules.

## Layout

| Path | What |
| --- | --- |
| `crates/rvp-core` | types, master clock, ring buffer, decoder traits |
| `crates/rvp-host` | host trait set (source, audio, video, surface, input, storage, clock) |
| `crates/rvp-demux` | MP4 and Matroska/WebM demuxers |
| `crates/rvp-codec-*` | audio, H.264 (own decoder; its bitstream, parameter-set, transform, CAVLC and CABAC modules are reusable by an encoder), AV1, VP9 decoders |
| `crates/rvp-subs`, `crates/rvp-viz` | text subtitles; audio analysis for visualizers |
| `crates/rvp-player` | the engine: scheduler, pipeline, A/V sync, playlist |
| `crates/rvp-ui`, `crates/theme` | the UI (drawn into a pixel surface) and the generated design tokens |
| `crates/rvp-app` | session + UI + input glue behind the host trait |
| `crates/rvp-host-{headless,web,rb}` | hosts: native test harness, browser, Rusty Bucket (`rvp-host-desktop` arrives in M11) |
| `web/`, `tests/e2e/` | the page (canvas, audio worklet, glue) and its Playwright tests |
| `xtask`, `tools/` | `cargo xtask theme`, `check`, `fixtures`, `web`, `serve`, `e2e` |

## Build and test

The Rust toolchain lives in `~/.cargo/bin`: `source ~/.cargo/env` first.

```sh
cargo test --workspace          # native tests
cargo xtask check               # also checks wasm32-unknown-unknown and no_std (x86_64-unknown-none)
cargo xtask fixtures            # ffmpeg-generated test media (incl. the x264 H.264 matrix) into target/fixtures (made on demand by tests)
cargo xtask theme               # regenerate crates/theme/src/tokens.rs from crates/theme/tokens/*.css
cargo xtask theme --sync        # first refresh the CSS snapshot from the design system
cargo xtask web                 # build the browser player into target/web (needs wasm-bindgen-cli 0.2.129)
cargo xtask serve [--port N]    # serve target/web, default http://127.0.0.1:8080/
cargo xtask e2e                 # build, make fixtures, run the Playwright tests (tests/e2e)
```

Browser player: drop a file on the page or press `O`. Keys: `Space`/`K` play, arrows seek 5 s (Shift 30 s), `J`/`L` 10 s,
`Up`/`Down` volume, `M` mute, `F` fullscreen, `[`/`]` speed, `\` normal speed, `Home`/`End`, `S`/`A` subtitle and audio track, `.`/`,` frame step, `I` A-B loop, `N`/`P` next and previous, `R` repeat, `Z` shuffle, `Q` playlist, Page Up/Down chapters; right-click for a menu with all
of them. Open several files to make a playlist; they play gaplessly. See [`docs/host-api.md`](docs/host-api.md) for the now-playing and visualizer interfaces. Screenshots are in [`docs/screenshots`](docs/screenshots).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your
option. Third-party components and their licenses are listed in
[`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).
