# Third-party licenses

This project is `MIT OR Apache-2.0`. Dependencies must be compatible with that. Update this file in the same
commit that adds, removes, or upgrades a dependency. Versions and licenses were checked on 2026-10-05
(`cargo info`, crate `Cargo.toml`). "Planned" entries are not yet in `Cargo.lock`.

## In use now

| Component | License | Notes |
| --- | --- | --- |
| `wasm-bindgen` (`rvp-host-web`, wasm32 only) | MIT OR Apache-2.0 | |
| `symphonia-core`, `-codec-aac`, `-codec-vorbis`, `-bundle-flac`, `-bundle-mp3`, plus its `-common`, `-metadata` 0.6.1 | MPL-2.0 | Used unmodified from crates.io. The license text and a pointer to the upstream source (https://github.com/pdeljanov/Symphonia) must accompany binary distributions. |
| `opus-decoder` 0.1.1 | MIT OR Apache-2.0 | |
| `libm` 0.2 (`rvp-core`, `no_std` sin/cos for the resampler) | MIT | |
| Transitive crates of the above (`bitflags`, `bytemuck`, `lazy_static`, `log`, `num-complex`, `num-traits`, `once_cell`, `smallvec`, `thiserror`, `autocfg`, `cfg-if`, ...) | MIT, Apache-2.0, Zlib, or `MIT OR Apache-2.0` | Checked with `cargo metadata` on 2026-10-05: nothing outside MIT/Apache-2.0/Zlib/MPL-2.0/Unlicense/Unicode-3.0. |
| `serde_json` (dev-dependency of `rvp-demux`, tests only) | MIT OR Apache-2.0 | |
| Unicorn Tears design-system tokens (`crates/theme/tokens/*.css`) | Project-owner material, included under this project's license | Vendored snapshot of `claude-design-system/tokens` (colours, type scale, spacing). Logos, mascot art, and stream thumbnails are **not** included and stay separately owned. |

## Planned (see `docs/PLAN.md` section 8)

| Component | License | Obligation |
| --- | --- | --- |
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
