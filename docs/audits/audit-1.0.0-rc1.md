# Audit before 1.0.0-rc1

Auditor: RW Strategist. Date: 2026-10-06. Tree: `4833374` (A, A2 and the rc1 prep done; the Cargo version was still 0.0.4).

**Method:**
- Code review of the riskiest and newest code: tag writing, theming fetch and parse, the updater, default-player registration, library and video persistence, posters, the decode thread pool, and the Flatpak manifest.
- A scan for panics on untrusted input (`unwrap`/`expect`/`unsafe` outside tests).
- Measured performance, light and niced: `cargo xtask perf-web --both --secs 30`, plus 20 s headless desktop runs under Xvfb per codec with `/usr/bin/time`.

Every finding below is to be fixed before tagging (user decision).

## Findings

| # | Severity | Area | Finding | Fix |
|---|---|---|---|---|
| B1 | High | Flatpak / default player | "Set as default media player" writes `mimeapps.list` under the sandbox's `XDG_CONFIG_HOME` (`~/.var/app/…/config`). It reports success but the host never sees it. | Grant `--filesystem=xdg-config/mimeapps.list` and write the host file (`$HOME/.config/mimeapps.list` resolved outside the sandbox), or if that's unavailable, say so honestly instead of "Set". Test the Flatpak path. |
| B2 | Medium | Flatpak / theming | No `--share=network`, so fetching a Claude Design link always fails inside the Flatpak. | Add `--share=network`. The update check stays off for Flatpak. |
| B3 | Medium | Flatpak / tag editor | Music and Videos are mounted `:ro`, so every tag save fails late with "read-only". | Grant `xdg-music` and `xdg-videos` read-write. `can_write` must detect read-only roots up front and explain before the form opens. |
| B4 | Medium | Tag editor (desktop) | `replace_file` renames over a symlinked track, so the symlink becomes a regular file (breaks symlinked libraries). | Canonicalize the target first and write beside the real file. Test with a symlinked track. |
| B5 | Low | Default player (Linux) | `set_linux_defaults` renames over `mimeapps.list`, replacing a symlink (dotfile managers). | Resolve symlinks before the temp file and rename. |
| B6 | Medium | Video library perf | Poster generation keeps decoding while a video plays (loudness analysis pauses; posters don't). This risks dropped frames, especially in the single-threaded web build. | Pause posters (and any other background decode) while video plays, the same way loudness pauses. |
| B7 | Low | Tag editor memory | `prepare` holds old + new + `new.clone()` (for `MemSource`): about 3× the file size, up to 768 MB at the 256 MB cap. That's an OOM risk in wasm. | Verify without cloning (borrow/`Rc`), and use a lower cap in the web host (e.g. 64 MB) with a clear message. |
| B8 | Low | Desktop audio | `ring.lock().unwrap()` in the audio paths: a poisoned mutex would panic the app (or the audio thread). | Recover from poisoning (`lock().unwrap_or_else(PoisonError::into_inner)`) and never panic in the cpal callback. |
| B9 | Low | Test harness | `cargo xtask perf-web --both`: the threaded pass's second and later specs fail with `ERR_CONNECTION_REFUSED` (the server stopped between passes), and Playwright trace artifacts hit `ENOENT`. | Keep the server alive across passes (or restart it per pass) and give each pass its own output dir. |
| P1 | High | Desktop CPU | The decode pool polls instead of blocking: `pool.rs` spin-waits for job completion, `relax()` parks for 60 µs, and the decoder thread wakes every 300 µs while busy. Desktop 1080p30, 20 s, no library: H.264 typical **1.13 cores**, H.264 stress 2.1, VP9 0.70, AV1 1.05. The decode itself needs roughly 0.3 core for H.264 typical, so polling burns about 0.5–0.8 core (battery, heat). Idle is fine (≈0). | Blocking waits (condvar or park with explicit unpark and completion notification; no spin longer than a few µs). Target: H.264 typical ≤0.5 core, with no drop-rate regression. Record before/after in docs/PLAN.md. |
| P2 | Medium | Web, single-thread | 1080p30 30 s: H.264 stress **6.44 %** dropped (M9 4.96 %), and the clock stalled (26.4 s played of 30, max tick 160 ms). AV1 10.0 % (M9 9.57 %). H.264 typical and VP9 0 %. The threads build is 0 % for everything measured. | Profile the stress regression (per-tick render is 3.3 ms; check the new tooltip/heart/viz paths on the player face, and whether posters or the library run during playback) and get back to M9 or better. AV1 single-thread is a known limit; no regression allowed. |
| P3 | Medium | First-run scan memory | Desktop RSS while playing: 105–158 MB with no library. With the first-run scan of the user's real Music folder (1,188 tracks), it's 337–444 MB, about 230 MB extra. | Find what the scan holds (whole-file reads, art decode buffers, the loudness queue, posters) and bound it. Target: under 100 MB above baseline for a 1–2k track first scan. |

## Measurements (before fixes)

| Run | Result |
|---|---|
| Web single-thread 1080p30 (30 s) | H.264 typical 0 %, H.264 stress 6.44 %, VP9 0 %, AV1 10.0 % dropped |
| Web threads 1080p30 (30 s) | AV1 0 %, H.264 stress 0 % (H.264 typical and VP9 not measured: B9) |
| Desktop CPU, 1080p30, 20 s (user time ÷ wall) | H.264 typical 1.13, stress 2.11, VP9 0.70, AV1 1.05 cores |
| Desktop RSS, 1080p30 | 105–158 MB, or 337–444 MB during a 1,188-track first scan |
| Desktop idle CPU | ≈0 (0.06 s user in 10 s) |

## Checked and fine

- The updater verifies size, SHA-256 and the OpenPGP detached signature before anything is moved or run. Manifest names are sanitised, and downgrades are refused.
- Desktop network fetch is HTTPS-only, with at most 4 redirects, a timeout and a byte cap. Theme imports are capped at 10 sheets and 400 KB of input.
- The video index decode bounds its counts against the blob size. Posters are bounded per video (600 packets).
- Web tag writes use `createWritable` (swap-file commit) with readwrite permission asked at write time.
- The macOS default-handler FFI has paired create/release calls. Windows opens Default Apps, since it can't be set silently.
- Rust's std opens files on Windows with `FILE_SHARE_DELETE`, so replacing the playing track's file works there.
- There are no `unwrap`/`expect` calls on untrusted input outside tests; the remaining ones are on fixed-size `try_into` slices or bundled assets.

## Needs the user's eyes

- The DJ Unicorn Tears paragraph on the About page (`crates/rvp-ui/src/lib_ui/about.rs`, `PARAGRAPHS`) uses facts from the arc.dev profile.
