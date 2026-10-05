# rusty-video-player — rules

- Commit & push after each substantial milestone.
- Use at most ONE sub-agent at a time: the `coder` agent (`.claude/agents/coder.md`, Sonnet 5.5, effort high). It does all coding, docs and tests; the main session only orchestrates.
- Be efficient: minimal reporting, no superfluous output. Ask the user only when genuinely blocked.

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
