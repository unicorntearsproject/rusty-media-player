# Rusty Wave

A standalone media app for **audio and video**, written in Rust, with a portable core that compiles to WebAssembly.
One codebase, two editions:

- **Standalone:** an installable web app (PWA) and a native desktop app for Linux (Flatpak, AppImage, .deb, .rpm) and Windows
  (installer). It never requires Rusty Bucket.
- **Rusty Bucket's built-in Media app**, through the `rvp-host-rb` adapter, once its app ABI exists. Nothing in
  the core, UI or app crates depends on [Rusty Bucket](../rust-os); it is just one more host, next to the
  browser, the desktop and the headless test host.

- Containers: MP4, MKV, WebM, and raw audio files (MP3, MP2, FLAC, Ogg incl. chained files, WAV, AAC/ADTS, with tags and cover art). Video:
  H.264 (our own decoder), AV1, VP9. Audio: AAC, MPEG layers I-III, FLAC, Opus, Vorbis, PCM (up to 8 channels, mixed down to stereo).
  Subtitles: SRT, WebVTT, ASS/SSA (styles, colours, position; no karaoke or effects) and PGS pictures, embedded or as files.
  Limits: AAC is LC, mono or stereo (a symphonia limit: 5.1 and HE-AAC with explicit SBR signalling play as video without sound);
  Opus is mono or stereo.
- 1080p30 in the browser: WebAssembly SIMD128 kernels, plus an opt-in threads build (`cargo xtask web --threads`).
- A portable `no_std + alloc` core behind a small host trait; no threads required.
- Playlist, gapless playback, subtitles and a host-neutral now-playing model (Media Session in the browser) are in, and so is the
  audio-first Library face: a scanned and indexed music library (albums, artists, tracks, search, cover art, saved playlists with
  M3U/M3U8/PLS import and export, a queue) and a full-window visualizer; the desktop app (`rusty-wave`) adds MPRIS and Windows media controls.
- Its own UI, drawn with the Unicorn Tears design-system tokens (`crates/theme`).
- Clean-room: VLC is an architecture reference only; no VLC code is used.

Status: milestones 0 to 11 done.

- M0-M6: scaffold, host/clock/executor, demuxers, audio, AV1, A/V sync, the headless host, the browser host with the themed
  UI, and our own pure-Rust H.264 decoder (Baseline/Main/High, progressive 8-bit 4:2:0, bit-exact with ffmpeg).
- M7: VP9 through `rusty_vp9`, bit-exact with ffmpeg.
- M8: playlist with gapless playback, SRT/WebVTT and embedded subtitles, audio-track switching, pitch-preserving speed,
  exact seek, frame step, A-B loop, chapters, resume positions, the now-playing model (Media Session in the browser)
  and the visualizer tap.

- M9: wasm SIMD128 kernels and an opt-in worker-thread build (1080p30 H.264, VP9 and AV1 at 0% dropped frames in headless Chromium),
  cargo-fuzz targets for the demuxers, decoders and parsers, truncated/growing/corrupt file handling, crash recovery, raw audio
  demuxers (MP3 with gapless info, FLAC, Ogg, WAV, ADTS) with tags and art.

- M10: the audio-first Library face next to the Player, one app with a switch (`B`): a library scanned from a folder (File System Access API or a
  folder input, incremental rescans, persisted in IndexedDB), albums, artists, tracks, search, queue and playlists (M3U, M3U8, PLS), gapless album
  playback, cover art, a now-playing screen and a visualizer view with five effects (reduced-motion safe), all with keyboard and pointer parity and
  context menus. See the [screenshots](docs/screenshots) and the M10 notes in [`docs/PLAN.md`](docs/PLAN.md).

- M11: the native desktop app `rusty-wave` for Linux and Windows (winit window, CPU-drawn pixels, cpal audio, native dialogs, drag and drop, full screen, HiDPI,
  MPRIS and the Windows media controls, system fonts for CJK, decoding on worker threads), the queue and playback position restored after a restart (all
  hosts), bounded cover memory, an original logo and icon set, and packaging: Flatpak, AppImage, .deb, .rpm, a Windows installer, the PWA, release
  workflows. See [`docs/packaging.md`](docs/packaging.md).

Try it with `cargo xtask web && cargo xtask serve` and open http://127.0.0.1:8080/. Run the desktop app with `cargo run --release -p rvp-host-desktop -- <files or folders>` (`rusty-wave --help`). Read [`docs/PLAN.md`](docs/PLAN.md)
for the architecture and the milestone list (M10 audio-first view, M11 desktop app and packaging, M12 Rusty
Bucket adapter), [`docs/host-api.md`](docs/host-api.md) for the host-neutral media interfaces, and [`CLAUDE.md`](CLAUDE.md)
for project rules.

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
cargo xtask dist <target>       # deb, rpm, appimage, flatpak, windows, installer, pwa, ... (docs/packaging.md)
cargo run --release -p rvp-host-desktop -- <files>   # the desktop app
```

Browser player: drop a file or a folder on the page or press `O`. Two faces, switched with `B` (or the button in the bar, or the switch in the
rail): the **Player** for video and the **Library** for music. A song opened from outside goes to the Library, a video to the Player.

Player keys: `Space`/`K` play, arrows seek 5 s (Shift 30 s), `J`/`L` 10 s, `Up`/`Down` volume, `M` mute, `F` fullscreen, `[`/`]` speed, `\` normal
speed, `Home`/`End`, `S`/`A` subtitle and audio track, `.`/`,` frame step, `I` A-B loop, `N`/`P` next and previous, `R` repeat, `Z` shuffle, `Q`
playlist, Page Up/Down chapters; right-click for a menu with all of them. Open several files to make a playlist; they play gaplessly.

Library keys: `1` Albums, `2` Artists, `3` Tracks, `4` Playlists, `5` Queue, `6` Now playing, `7` or `V` the visualizer, `/` or `Ctrl+F` search (type;
`Esc` clears and goes back). Plain arrows, `Home`/`End`, `PageUp`/`PageDown` move through the list or grid, `Enter` plays from the selected row (or
opens an album, artist or playlist), `Shift+Enter` adds to the queue, `Ctrl+Enter` plays next, `Delete` removes from the queue or a playlist,
`Alt+Up`/`Alt+Down` move an item, `Backspace` or `Esc` go back, `Tab` walks rail, content and bar, the menu key or `Shift+F10` opens the context menu
of the selection. `Ctrl+Left`/`Ctrl+Right` seek and `Ctrl+Up`/`Ctrl+Down` change the volume (`J`/`L` and `M` still work). In the visualizer:
`Left`/`Right` change the effect, `C` the colours, `T` the title, `Enter` turns it on or off. With the pointer: click a card or row (double click plays),
the play button on a card, right-click anything for its menu, drag queue rows to reorder, the mouse's back button goes back, the wheel scrolls.
Add a folder with the button in the rail (`Add folder`), drop one on the page, or open playlist files (`.m3u`, `.m3u8`, `.pls`) to import them.
See [`docs/host-api.md`](docs/host-api.md) for the now-playing, visualizer and library interfaces. Screenshots are in [`docs/screenshots`](docs/screenshots).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your
option. Third-party components and their licenses are listed in
[`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).
