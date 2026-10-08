# Developing Rusty Wave

[Documentation index](../README.md) · [Plan and architecture](../planning/PLAN.md) · [Host interfaces](host-api.md) · [Keys](keys.md) · [Packaging](../release/packaging.md)

Rust toolchain: it lives in `~/.cargo/bin`, so `source ~/.cargo/env` first. The project rules (clean-room, licences, testing, scratch space) are in
[`CLAUDE.md`](../../CLAUDE.md).

## Layout

| Path | What |
| --- | --- |
| `crates/rvp-core` | types, master clock, ring buffer, decoder traits |
| `crates/rvp-host` | host trait set (source, audio, video, surface, input, storage, clock) |
| `crates/rvp-demux` | MP4, Matroska/WebM and raw audio (MP3, FLAC, Ogg, WAV, ADTS) demuxers, tags and cover art |
| `crates/rvp-par` | worker pool, decoder-on-a-thread, pipelined H.264 (native threads or Web Workers) |
| `fuzz/` | cargo-fuzz targets and seed corpora (`cargo xtask fuzz`) |
| `crates/rvp-codec-*` | audio, H.264 (own decoder; its bitstream, parameter-set, transform, CAVLC and CABAC modules are reusable by an encoder), AV1, VP9 decoders |
| `crates/rvp-subs`, `crates/rvp-viz` | text subtitles; audio analysis and the effects of the visualizer |
| `crates/rvp-library` | the music library: index, scan driver, cover thumbnails, saved playlists (M3U/M3U8/PLS), persistence |
| `crates/rvp-player` | the engine: scheduler, pipeline, A/V sync, playlist |
| `crates/rvp-ui`, `crates/theme` | the UI (the Player and the Library faces, drawn into a pixel surface) and the generated design tokens |
| `crates/rvp-app` | session + UI + input glue behind the host trait |
| `crates/rvp-host-{headless,web,desktop,rb}` | hosts: native test harness, browser, desktop (Linux and Windows, binary `rusty-wave`), Rusty Bucket |
| `packaging/`, `assets/brand/`, `.github/workflows/` | metadata, icons, Flatpak, Windows installer; the logo; release workflows (tag or manual only) |
| `web/`, `tests/e2e/` | the page (canvas, audio worklet, glue) and its Playwright tests |
| `xtask`, `tools/` | `cargo xtask theme`, `check`, `fixtures` (also the 200-track test library, `tools/gen-library.py`), `web`, `serve`, `e2e`, `perf-web`, `fuzz`, `dist` (packages) |

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
cargo xtask web --threads       # also build the shared-memory (worker threads) variant; needs nightly + rust-src
cargo xtask e2e [--threads]     # build, make fixtures, run the Playwright tests (tests/e2e); --threads runs them on both builds
cargo xtask perf-fixtures       # one-minute 1080p30 streams; then `cargo xtask perf-web [--both]` plays them in the browser
cargo xtask fuzz [target] [secs]  # cargo-fuzz targets (nightly + cargo-fuzz)
cargo xtask dist <target>       # deb, rpm, appimage, flatpak, windows, installer, pwa, ... (../release/packaging.md)
cargo run --release -p rvp-host-desktop -- <files>   # the desktop app
```


Tests run headless (Playwright headless; the desktop tests under `xvfb-run`). `cargo xtask e2e` also needs Node and Chromium (`npm install` in
`tests/e2e` is done for you). `python3 tools/check-links.py` checks every relative link in the docs and every `docs/...` path named in code.
