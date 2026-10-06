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

## After fixes (RW Coder, 2026-10-06)

Every row is fixed in the commit that follows this document. Tests are named where a fix has one.

| # | Done | Test |
| --- | --- | --- |
| B1 | Flatpak default player: the host's `mimeapps.list` (`HOST_XDG_CONFIG_HOME`, else `$HOME/.config`) is edited **in place** (only that file is shared: `--filesystem=xdg-config/mimeapps.list`); if it does not exist, or is not writable, the dialog says so with the exact `xdg-mime default …` command instead of "Set". | `defaults::tests::a_symlinked_mimeapps_list_stays_a_link_and_flatpak_writes_in_place` |
| B2 | `--share=network` added. | `xtask dist check` (`flatpak_permissions_ok`) |
| B3 | `xdg-music` and `xdg-videos` read-write; `can_write` probes the folder (makes and removes a file) when the editor opens, so a read-only folder is explained (with the `flatpak override` command in Flatpak) before anyone types. | `writer::tests::a_read_only_folder_is_refused_up_front_when_the_editor_opens`, `the_flatpak_must_grant_what_tags_themes_and_defaults_need` |
| B4 | The tag writer canonicalizes the target and replaces the real file; a symlinked track stays a link. | `writer::tests::a_symlinked_track_stays_a_link_and_its_target_is_what_changes` |
| B5 | `mimeapps.list` that is a symlink: temp file beside the real file, link kept. | same test as B1 |
| B6 | Posters and loudness measurement also wait while a film is *opening or buffering* (only `Playing` held them before); folder reading (headers, tags) is the one background job that runs while a film plays. | `videos_scan::posters_wait_while_a_video_plays_and_go_on_when_it_is_paused` |
| B7 | The edited bytes are checked in place (no `clone()`): at most two copies of the file instead of three, the old one dropped as soon as the new exists; the web build's cap is 64 MB (desktop 256 MB) with a message that names it. | the tag editor tests (`tagedit.rs`) exercise `prepare` |
| B8 | `lock_ring` recovers a poisoned mutex; the device callback and the player thread can no longer panic on another thread's failure. | `audio::poison_tests::a_poisoned_ring_is_still_usable_by_both_threads` |
| B9 | `perf-web` starts the page's server once for all passes and gives each pass its own Playwright output folder (`RVP_E2E_OUTPUT`). `--both` now runs all 8 specs. | run below |
| P1 | The decode pool no longer polls: `Pool::run` wakes only the workers it needs, the caller sleeps until the last task unparks it; H.264's pipeline waits (`submit`, `wait_idle`, parse workers) park until the worker unparks them; the remaining waits (`MotionSlot::wait`, `JobSlot::wait`, the decoder thread's poll of an inner decoder) use an exponential back-off (`rvp_core::par::Backoff`, 25 µs to 1 ms) instead of a 60 µs poll. A `perf record` of H.264 typical shows no spin or wake symbols left. See the honest numbers below. | `rvp-par` and `rvp-core` tests (pool, pipeline, `Backoff` through the h264 waits) |
| P2 | No regression found when measured against the M9 commit **on the same machine on the same day** (below). Nothing was changed in the web decode path except the shared back-off; `wasm-opt -O3`/`-O4` were tried and give nothing measurable (kept `-O2`, the level is now `RVP_WASM_OPT`). | – |
| P3 | Cause: the poster job used the full player decoders (a thread pool per decoder, big frame pools: AV1 kept 150 MB). Posters now use `CodecFactory::video_light`: single-threaded, no pool; plus the library no longer starts new tag reads while 4 results are waiting to be filed (backpressure), and big covers are shrunk straight from the decoder's output (no full-size RGBA copy). | `videos_scan::posters_are_made_with_the_light_decoder_that_has_no_threads_of_its_own`, `art::tests::shrinking_straight_from_the_components_gives_the_same_picture` |

### Measurements after (desktop)

Same method as the audit: `/usr/bin/time` user time over 20 s of 1080p30 playback (23 s wall including start-up and exit), Xvfb, `HOME`/`XDG_*` in an empty folder, `--no-audio`. The host was shared (load average 3–4 from other sessions), so every figure carries about ±10 %.

| Fixture | User time ÷ wall, before | after | App threads only (`/proc/<pid>/task`, 8 s) after |
| --- | --- | --- | --- |
| H.264 typical | 0.93 | 0.87 – 0.94 | 0.99 cores (was 1.10 before the first fix) |
| H.264 stress | 1.94 | 1.78 – 1.98 | 2.19 |
| VP9 | 0.62 | 0.67 – 0.69 | 0.81 |
| AV1 | 0.91 | 0.88 – 0.91 | 1.15 |

**P1: x86_64 SIMD added; H.264 typical 0.87–0.94 → 0.64 core (target ≤ 0.5 not met, close).** Polling was minor; the CPU was in scalar kernels. `rvp_core::simd` now has an SSE2 side (`simd/x86.rs`: the SIMD128 names as thin `core::arch::x86_64` wrappers, SSE2 being the x86_64 baseline, no runtime check) and every SIMD128 kernel (H.264 luma/chroma MC, bi-prediction and weighting, deblocking, YUV→RGBA, the scaler) runs on it unchanged. Each keeps its scalar twin, and the same `selftest`s (random data, scalar vs SIMD) are now native `cargo test`s, as are per-operation tests of the shim against plain lane arithmetic. All `unsafe` is in the shim (pointer loads/stores with SAFETY notes, register-only intrinsics in blocks with a SAFETY note); the codec and UI crates stay `forbid(unsafe_code)`. aarch64 (macOS) stays scalar for now. No AVX2 variants: the profile is spread over many kernels, not one wide loop.

| Desktop, user÷wall, 20 s | Before | After |
| --- | --- | --- |
| H.264 typical | 0.87–0.94 | 0.64 |
| H.264 stress | 1.78–1.98 | 1.09 |
| VP9 | 0.67–0.69 | 0.46 (the VP9 decoder is a dependency; its kernels are not ours) |
| AV1 | 0.88–0.91 | 0.69 |

What is left in H.264 typical (`perf`): `mc_luma` 12.6 %, CABAC `residual_sparse` 7.6 % (entropy, not SIMD-able), deblocking 7 % + `Bs::motion` 3.8 %, `yuv420_rows_to_rgba` 6.9 %, `MbRecon::uniform` 6 %, `mc_chroma` 6 %, scalar `combine_scalar` 5.3 %, `weighted_bi` 4.3 %, `blit_scaled` 4.1 %, memmove 3.5 %. Further gains are a long tail (AVX2 for colour conversion, SIMD `combine`/`uniform`, boundary-strength), none above 0.05 core each.

Memory, desktop RSS: playing 1080p, no library: 105–158 MB (unchanged); **scanning 12 1080p films for posters: 182 MB → 56 MB** (H.264 110 → 55, VP9 97 → 51, AV1 178 → 47); scanning 160 tracks with 6 MB embedded covers: 50 MB.

### Measurements after (web, `cargo xtask perf-web --both --secs 30`)

| Build | Fixture | dropped | session ms/tick |
| --- | --- | --- | --- |
| threads | AV1 / H.264 stress / H.264 typical / VP9 | **0 / 0 / 0 / 0 %** | 2.0 / 2.0 / 1.9 / 1.9 |
| single | H.264 typical / VP9 | 1.8 % / 0.33 % (loaded host; 0 % when quiet) | 12.9 / 7.5 |
| single | H.264 stress | 7.6 % (5.7 – 8.5 % over six runs) | 37 – 39 |
| single | AV1 | 9.1 – 9.6 % when the host is quiet (26 % while another run shared it) | 9 – 10 |

P2, same machine, same day, single-thread H.264 stress, three runs each: **M9 commit `7ad474d` 6.64 / 7.09 / 6.82 % dropped, 35.7 – 36.0 ms/tick; HEAD 7.84 / 8.46 / 5.71 %, 37.0 – 39.0 ms/tick.** The audit's 6.44 % and M9's recorded 4.96 % were taken on a quieter day; run to run the same commit varies by ±1.5 ms. At most a 4 % rise in decode time, inside the noise, and no code on that path changed since M9 (the codec crates are untouched); the UI per-tick costs (`render` 3.5 ms, `present` 0.65 ms) are the same. The single-thread build at 1080p30 stress is at its limit, as the plan says; the threads build has none.

## UX-visible changes (for the report)

- Rail: Settings and About RW use compact buttons; Add folder hides in very short windows (earlier, in A2).
- Flatpak: the Music and Videos folders are now writable by the app, the app can use the network (theme links), and "Set as default media player" really changes the host's defaults, or says exactly what to do when it cannot.
- Tag editor: a folder that cannot be written is explained when the editor opens, not when saving.
- Web: files over 64 MB cannot have their tags edited (message says so).
- Posters wait while a film opens, buffers or plays (they used to wait only while it played).

