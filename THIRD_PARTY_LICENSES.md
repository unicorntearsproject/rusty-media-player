# Third-party licenses

This project is `MIT OR Apache-2.0`. Dependencies must be compatible with that. Update this file in the same
commit that adds, removes, or upgrades a dependency. Versions and licenses were checked on 2026-10-05
(`cargo info`, crate `Cargo.toml`). "Planned" entries are not yet in `Cargo.lock`.

## In use now

| Component | License | Notes |
| --- | --- | --- |
| `wasm-bindgen` (`rvp-host-web`, wasm32 only) | MIT OR Apache-2.0 | |
| Unicorn Tears design-system tokens (`crates/theme/tokens/*.css`) | Project-owner material, included under this project's license | Vendored snapshot of `claude-design-system/tokens` (colours, type scale, spacing). Logos, mascot art, and stream thumbnails are **not** included and stay separately owned. |

## Planned (see `docs/PLAN.md` section 8)

| Component | License | Obligation |
| --- | --- | --- |
| `symphonia` 0.6.x (AAC, MP3, FLAC, Vorbis decode) | MPL-2.0 | Unmodified dependency: keep the license text and source availability notice. Modified files would have to stay MPL-2.0. |
| `opus-decoder` 0.1.x | MIT OR Apache-2.0 | |
| `rubato` 5.x | MIT OR Apache-2.0 | |
| `rav1d` 1.1.x (patched for wasm32, vendored under `third_party/rav1d`) | BSD-2-Clause | Keep the copyright notice and conditions with source and binary distributions; record the patch. |
| `rusty_vp9` 0.1.x or `vp9dec` 0.1.x | Apache-2.0 / MIT | Whichever is adopted in M7. |
| `fontdue` 0.9.x | MIT OR Apache-2.0 OR Zlib | |
| `hashbrown`, `libm`, `spin`, `bitflags`, `thiserror` | MIT or MIT OR Apache-2.0 | As needed. |
| Space Grotesk, JetBrains Mono (bundled font subsets) | SIL OFL 1.1 | Ship the OFL text and copyright lines; do not sell the fonts alone. |
| Anton (display) | SIL OFL 1.1 | Same. |
| Lucide icons (as path data) | ISC | Keep the copyright notice. |

Dev/test only (not shipped): `matroska-demuxer` (Zlib OR MIT OR Apache-2.0), `mp4` (MIT),
`rusty_h264-decoder` (BSD-2-Clause), `h264-reader` (MIT/Apache-2.0), `ffmpeg`/`ffprobe` as external oracles
(never linked, never distributed).

## Rejected

| Component | Why |
| --- | --- |
| `rav1d-safe` | AGPL-3.0 OR commercial |
| VLC source | GPL/LGPL; used as an architecture reference only, never copied or translated |
| `x264`, libopus/`audiopus_sys`, `fdk-aac`, `dav1d` (C) | C code or GPL; does not fit wasm32-unknown-unknown without a C toolchain |
