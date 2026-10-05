# rusty-video-player — rules

- Commit & push after each substantial milestone.
- Use at most TWO sub-agents at a time (user, 2026-10-05; may change, so follow the latest instruction): the `coder` agent (`.claude/agents/coder.md`, Sonnet 5.5, effort high). It does all coding, docs and tests; the main session only orchestrates.
- Be efficient: minimal reporting, no superfluous output. Ask the user only when genuinely blocked.
- Never use `rm -rf` or other recursive deletes (user rule). Put scratch/build output in fresh `/tmp` dirs (`mktemp -d /tmp/rvp-XXXX`) and overwrite in place.
- Never run artificial CPU load generators (`yes`, busy loops, stress) on this machine (user rule).
- Run all tests headless (user rule): Playwright headless, desktop/Wine under `xvfb-run -a` with DISPLAY/WAYLAND_DISPLAY overridden. Never open windows or steal focus on the user's session.
- Tests: thorough but proportionate (user rule). Cover edge cases and error paths, sized to realistic use with a few times headroom, never absurd scales. Keep the everyday gate (`cargo test`, e2e) fast and deterministic. Fuzz, stress and long benchmarks are occasional or nightly jobs, run on this shared host only with the user's OK (Rusty Bucket CI shares it). See ../rust-os/docs/testing/testing-guide.md.

## Context
- Goal: a VLC-derived media player in Rust, compiled to WebAssembly, as an app for Rusty Bucket (`../rust-os`, plan index: `docs/planning/README.md`).
- Reference source: `../vlc` (shallow clone of upstream VLC).
- Drop legacy/obscure features (optical discs, rare protocols/codecs, skins, etc.).
- Look & feel: follow `../u-studio-video-editor` and `/home/jj/projects/unicorn-tears/claude-design-system` — not VLC's UI.

## Decisions (2026-10-05)
- **License: MIT OR Apache-2.0, clean-room.** `../vlc` is an architecture/behavior reference only. Never copy or line-by-line translate VLC code (GPL/LGPL). Track third-party licenses in `THIRD_PARTY_LICENSES.md`; deps must be MIT/Apache-compatible (MPL-2.0 crates like symphonia are OK as unmodified deps).
- **Codecs v1:** containers MP4/MKV/WebM; video H.264 (our own pure-Rust decoder), AV1 (rav1d), VP9; audio AAC, MP3, FLAC, Opus, Vorbis.
- **Target:** portable `no_std + alloc` player core behind a small host trait. Dev/test hosts: browser (wasm32 page) and headless native. Rusty Bucket adapter comes once its app ABI exists.
- **UI:** drawn by us (Unicorn Tears tokens), so it renders the same in the browser and in Rusty Bucket's Canvas surface.
- Repo: private `iDoMeteor/rusty-video-player`.
- Rust toolchain lives in `~/.cargo/bin` (not on PATH): run `source ~/.cargo/env` first.
- **Standalone (user, 2026-10-05):** RVP is Rusty Bucket's built-in Media app (audio + video), but it must remain a shippable independent app that never requires Rusty Bucket. Targets: an installable web app (PWA) and a native desktop app (Linux first). Rusty Bucket is just one more host; nothing in core/ui/app may depend on it. Separate builds/editions per target are fine (user OK); prefer one shared codebase with per-host crates.
- **Packaging (user, 2026-10-05), at the end (M11):** Flatpak, AppImage, .deb, .rpm, and a Windows .exe with an Inno Setup installer. Full app polish: logo, icon set at every size (.ico/.icns/hicolor PNG+SVG), .desktop file, AppStream metainfo, MIME/file associations, signing hooks. Reference: `../unicorn-viz` packaging. The desktop host must therefore support Windows as well as Linux.
- **Name (user, 2026-10-05): the official product name is "Rusty Wave".** Official icon master: `../rust-os/assets/images/icons/Rusty Wave app icon.png` (1254px RGBA; copy it into `assets/brand/`, never edit ../rust-os). Internal crate prefix `rvp-` stays as the codename; everything user-facing (app name, binary, app ID, packages, installer, PWA, docs) says Rusty Wave.
- **Distribution (user, 2026-10-05):** every time new installers build and verify, copy them (with SHA256SUMS and .asc signatures) to `/home/jj/projects/_software-dist/rusty-wave/` and to `s3://ut-software-dist/` (bucket root, as Unicorn Viz does). Name the files with the version (`rusty-wave-<ver>-...`) and never overwrite an existing version.
