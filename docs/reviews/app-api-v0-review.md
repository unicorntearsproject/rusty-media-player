# Review of Rusty Bucket's App API v0 draft (`bucket_v0`)

Reviewed 2026-10-05 against rvp commit `746da34`. Draft read: `../rust-os/docs/developer/app-api.md`, `app-api-reference.md`,
`bucket-format.md`, ADR-0039 (and ADR-0023, ADR-0026). Compared with `rvp-host` (traits, `media.rs`, `library.rs`, `input.rs`),
`rvp_core` (`VideoFrame`, `AudioParams`, `Source` use), `rvp-player` (`TICK_US`, executor, `request_wake`), `rvp-app` (effects,
`FrameSink` compositing), `rvp-library` persistence, `rvp-par` (threads) and the web and desktop hosts. Nothing in `../rust-os`
was edited. Per ADR-0026 our traits stay as they are: every item below is a request to change the App API (or to document it),
except the short "Our side" list at the end.

Severity: **blocker** = we cannot build a correct adapter without it; **should** = we can work around it badly, or it will bite
within the first release; **nice** = polish.

## Corrections and requests

### Blockers

1. **[blocker] `video_present` layout is inconsistent and under-specified.** Draft: reference, Video, "frame layout (88 bytes)".
   Counted as written (six `u32`, then three `[i32;3]` each followed by a `u32` pad, then `i64`, then `[i32;4]`) the struct is
   96 bytes with `pts_us` at 72, not 88; and whether each pad follows its array or not is ambiguous (a plain `repr(C)` has none).
   Also unstated: stride unit, 10-bit packing, plane geometry. Change: publish an explicit offset table. Proposal: `struct_size u32`
   at 0, then `width, height, format, matrix, range, flags` (all `u32`, offsets 4 to 28), `planes [u32;4]` (32), `plane_lens [u32;4]`
   (48), `strides [u32;4]` (64), `pts_us i64` (80), `dest [i32;4]` (88) = 104 bytes; arrays of four (the fourth is for alpha or
   later 4:4:4 and is 0 now) so no padding is needed. State: strides are **bytes**; `Yuv420p10` is little-endian `u16` words
   with the value in the **low** 10 bits (0..1023, not P010's high bits; our `rvp_core::color` reads it that way); chroma planes
   are `(w+1)/2 x (h+1)/2` for odd sizes; `width/height` are the visible size, not coded size; `plane_len >= (rows-1)*stride + row_bytes`;
   no pointer alignment is guaranteed, so the host must use unaligned loads; the host reads the planes during the call only
   and may not hold them. The "mirrors `rvp_core::VideoFrame` field for field" sentence is not true as written (we have no `dest`
   or `plane_lens`; our planes are three separate buffers, our strides `usize`): reword it as "carries the same information".

2. **[blocker] A synchronous `file_read_at` can stall our single-threaded executor and starve audio.** Draft: app-api.md, last
   paragraph under "Mapping" ("the calls are synchronous and return quickly, so the adapter returns ready futures"); reference,
   Settings and files. Our `Source::read_at` is `async` on purpose: the demuxer, the library scanner (one open plus a few reads per
   file, thousands of files) and the tag reader all run as cooperative tasks inside `Player::tick`, which also feeds the audio
   ring (about 1 s deep) and presents video. "Quickly" is not a contract: a cold disk, virtio-blk, a FAT image on USB, or a
   future network stream (ADR-0015) will take tens of milliseconds to seconds. Change: make `file_read_at` **non-blocking by
   contract**: it returns the bytes that are available now, or `-BUSY` after starting a fill, and the host posts an event
   `IO_READY { file: i32 }` (new kind, e.g. 36) when a retry will succeed. Add `file_prefetch(h, offset: i64, len: i32) -> i32`
   (a hint, never blocks). A host whose files are always instantly readable (ramdisk, page cache hit) simply never returns `-BUSY`;
   the adapter turns `-BUSY` into `Pending`, and the event wakes `events_wait` so we do not wait for the next 10 ms tick. Do not
   use "completion writes into the app's buffer" (async DMA into a buffer that our dropped future already freed is a hazard):
   a retry-style read is cancel-safe. Also state the short-read rule (a short read only at end of file, or we must loop) and a
   per-call bound (say <= 1 MiB per call). The same applies to `kv_load`/`kv_store` (item 12) and `file_open_id` on a slow volume.

3. **[blocker] Structs cannot grow, and a missing import breaks instantiation.** Draft: app-api.md, Versioning ("additions
   inside a version are new functions and new capability bits only"; "`api_version()` returns the minor revision"). Every
   struct (`meta`, `pb`, `viz_summary`, `video frame`, `canvas_info`, `audio_open` out, the 64-byte event payloads) has a fixed
   size with no length field, so a minor revision can never add a field (for example a track number, or `transfer` on a
   frame). And a core wasm module that imports a `bucket_v0` function the host does not have fails to link, so an app built against
   minor 3 cannot run on a host at minor 2, which makes `api_version()` useless. Change: (a) every struct passed by pointer starts
   with `struct_size: u32` (and `flags: u32` where useful); the host reads `min(struct_size, known)` bytes and treats the rest
   as zero; for host-written structs the app passes its size and the host fills at most that much. (b) Every function of the
   major version always exists; on a host that predates it, it returns `-UNSUPPORTED`. (c) Unknown event kinds and unknown
   flag bits are ignored. (d) The manifest gets `api_min = "0.3"` so the OS can refuse early. Freeze these rules before freezing `bucket_v1`.

4. **[blocker] A music player must keep playing when it is not focused, and the draft can freeze it.** Draft: app-api.md,
   Lifecycle (`SUSPEND` "Frozen anyway (the resource guard)"), bucket-format.md `class = "foreground"`. A Media app's whole
   point is background audio. If the app is frozen or its main thread is starved, the ring drains in 1 s and the sound stops, and
   the tick that refills it never runs. Change: define a scheduling class `media` (manifest `class = "media"`) that the OS
   grants only while the app holds an open, playing `audio_open` stream: such an app gets `SUSPEND`/`RESUME` only for its video and
   canvas (a hidden canvas needs no frames), is never frozen while audio is playing, and keeps its timers and worker threads
   scheduled. State what `SUSPEND` does to an open audio stream (we ask: nothing; it keeps playing), and that `time_now_us` is
   monotonic across a freeze (we re-anchor our clock on a jump either way, but we need to know it can happen; a `RESUME` payload
   with the slept duration would help).

5. **[blocker] The library calls cannot express what our `Effect`s and `Library` trait need.** Draft: reference, Media shell,
   `library_*` rows and events 34/35. Gaps against `rvp_host::Library`/`Listing`/`FileEntry` and `rvp_app::Effect`:
   - `Effect::Rescan(root)` and `Effect::Forget(root)` have no calls. Add `library_rescan(root, len) -> i32` and
     `library_forget(root, len) -> i32` (forgetting also revokes the app's access).
   - `Listing.name` (the folder's display name) is not returned anywhere. Add `library_root_info(root, len, buf, cap)` or put it in
     the listing header; also have `library_roots` return `root_id\tname` lines (or a record list) and say whether
     a remembered but not currently readable root is listed (our `connected_roots` returns only readable ones; the browser has
     remembered-but-locked folders, and a removable disk can have the same state). Add a `library_reconnect(root)` that re-prompts.
   - **Root ids must be stable across sessions** (we key the saved index, playlists and queue by root id and path); say so, and
     that `file_open_id` ids from a listing only live until the next listing of that root (our contract).
   - The listing record layout (`id_len u32, path_len u32, size u64, mtime_ms i64` then bytes) is 24 bytes of header followed by
     unaligned data, so the next record's `u64`s are unaligned; either pad each record to 8 bytes or say "packed, read with
     `from_le_bytes`". State `path` is `/`-separated and relative to the root, `mtime_ms` is Unix epoch milliseconds, 0 when unknown.
   - A listing can be 100k files x ~150 B = 15 MB; "call once small, again with the right size" means the host must keep that
     blob between calls and we need a 15 MB buffer in a 32-bit linear memory. Add an offset/cursor (`library_listing(root, len,
     offset: i64, buf, cap)`, returning the total) or a `library_listing_next` iterator, and say how long the host keeps it
     (until the next `LIBRARY_LISTING` for that root or until `library_listing_release(root)`).
   - The `FOLDER_ADDED` and `LIBRARY_LISTING` payloads use text handles that die at the next `events_wait`: fine, but say so for
     the app (we copy them out immediately).
   - A walk of a big tree takes seconds: allow `LIBRARY_LISTING` with a `flags` bit "partial, more follows", or at least a
     `LIBRARY_PROGRESS`. (Nice.) A `LIBRARY_CHANGED { root }` event for file-system changes would let us avoid polling (nice).

### Should

6. **[should] Build selection: SIMD and threads are separate build axes, and `caps()` cannot choose a build.** Draft:
   bucket-format.md ("Which build runs": only `app.wasm` and `app.threads.wasm`, "threaded (+ SIMD)"), app-api.md
   Capabilities. A module that contains `simd128` or atomics instructions fails *validation* on an engine without them, before
   any `caps()` call. Our browser build is always `+simd128` and the threads build is `+atomics,+bulk-memory,+mutable-globals`
   with a shared, imported memory (max 2 GiB). A portable build with no SIMD runs 1080p video far too slowly on an interpreter, but
   it is the only thing wasmi can be sure to load. Change: a manifest table `[[builds]] file = "app.simd.wasm", requires =
   ["simd128"]`, `app.threads.wasm`, `requires = ["simd128","threads"]`, `app.wasm` (baseline); the OS picks the best one it can
   instantiate and says which in `caps()`. Also document the baseline wasm feature set every engine accepts: Wasm 2.0 (sign-ext,
   mutable-globals, multi-value, bulk-memory, reference-types, non-trapping float-to-int); current stable Rust emits those.

7. **[should] Threads: the start-up contract is missing.** Draft: reference, Threads. We spawn through
   `rvp_par::set_spawner` with a boxed closure pointer, exactly like the browser's `rvp_worker_entry(ptr, spawn)`. Specify:
   (a) the module imports memory as `env.memory` (LLD's default) with shared flag, initial and maximum from the module; the
   engine must honour `maximum` and make `memory.grow` return -1 (not trap) at the quota; (b) each thread is a **new instance** of
   the module on the same memory (as web workers are), so the stack pointer and TLS base are per instance: the app allocates the stack and
   TLS block before `thread_spawn` and passes the pointer in `arg`; the host promises nothing about `__wasm_init_tls`
   or the stack (state this, and the minimum stack the host gives, if any); (c) `thread_spawn` may be called from **any** thread
   (the browser allows it only from the main thread and we have a workaround; we would rather not need one), and
   `bucket_thread_start` runs on the new thread and returns when the thread is done; (d) `memory.atomic.wait32/64` and `notify`
   work on **every** thread, main included (our code avoids waiting on the main thread because a browser forbids it; if RB's
   main thread may wait we can drop our spin-lock allocator there), with the timeout honoured; (e) `cpu_count()` can change
   at run time (resource guard): say when, and add a `CPU_COUNT_CHANGED` event or document that we poll it; (f) whether a thread
   may exceed `cpu_count` (we clamp the pool to 2..=8, so say what happens if the guard grants 1 core); (g) `exit` ends every thread of the app at once (see item 8); no `thread_sleep` is needed (atomics with a timeout do it).

8. **[should] Panic, trap and restart.** Draft: Lifecycle ("The OS can always kill or freeze an app"); no mention of traps. Rust on
   `wasm32-unknown-unknown` is `panic=abort`: every panic is a trap, and `catch_unwind` does not exist. Our VP9 decoder
   (`rusty_vp9`) panics on malformed input in rare cases, and a trap on a decoder thread leaves shared state (spin locks held, a
   half-written ring) inconsistent. The browser host handles this by throwing away the whole wasm instance and starting a new
   one (`web/main.js`, `recover()`), limited to 3 restarts a minute, with all state coming back from storage (resume position every
   5 s, queue, library). We need the same in RB: (a) a trap on **any** thread terminates the whole app and releases the shared
   memory (no half-dead app); (b) a manifest policy `[run] restart = "on-trap"`, `restart_limit = "3/60s"` so the OS relaunches
   the app, with the OS owning the restart loop and the toast; (c) `launch_reason() -> i32` (0 normal, 1 hot reload, 2 after a
   trap, 3 after a kill) so we can show "The player crashed and was restarted"; (d) a `restart()` import for a clean
   re-instantiation we trigger ourselves, since we can detect a failed worker only from the main thread (e.g. a heartbeat) and
   want a restart without a trap; (e) `log` lines written before the trap are kept (our panic hook logs the message first) and
   the trap kind is added to the system log. Also confirm `bucket_save_state` is not required for any of this: our state is
   in `kv` already; we will return length 0 and flush the keys.

9. **[should] Video: `canvas_present` is our baseline, so it must be cheap; `video_present` is an optional accelerator.** Draft:
   reference, Canvas and Video; the "video layer under the canvas" model. Today `rvp_app::App` takes a `FrameSink` and
   composites the converted video into its own RGBA frame (one UI, one pixel surface), and our `VideoSink::present(&VideoFrame)`
   has no destination rectangle, so `dest` has nothing to come from. Per our M12 rule we do not change the app for Rusty
   Bucket's sake: adapter plan A is `FrameSink` plus `canvas_present` of the dirty rectangle. Requests: (a) `canvas_present`
   copies the dirty rectangle (or composites without a copy) and **returns without waiting for vsync** (a blocking present would
   stall the tick and the audio feed); say this; (b) a present with a stale size (a `RESIZE` is pending) returns `-BUSY` and draws
   nothing instead of trapping or `-INVALID`, as our desktop host does; (c) the buffer needs no alignment and `len` must equal
   exactly `w*h*4`; (d) `VIDEO_YUV` stays optional in the manifest (it is already in `optional`); (e) when we do adopt a video
   layer (an RVP-side change useful on desktop GPUs too: a `VideoSink` with a rectangle, a UI that leaves a transparent hole),
   we need `video_present` with `dest` = `{0,0,0,0}` meaning "hide the layer" (audio-only item) and a documented scaling filter
   (bilinear; a `flags` bit for nearest). Do not wait for this to freeze v0.

10. **[should] Frame pacing: there is no display-refresh signal.** Draft: Time and events; Canvas. Our tick runs every
    `TICK_US` = 10 ms (`session.rs`) and presents the newest due frame. On a 60 Hz display a 60 fps video is shown with
    ticks of 10 ms: judder (frames show for 20/20/10/20 ms patterns). The browser has `requestAnimationFrame`; the desktop host
    ticks off winit. Change: add `canvas_info.refresh_mhz: u32` and a `FRAME` event (kind 11) `{ vblank_us: i64 }` that is delivered
    once per display refresh while the app has armed it with `frame_request(on: i32)` (so a paused or hidden app costs nothing);
    `time_us` is the vblank we should target. Plus a `VISIBILITY` event (visible/occluded/minimised) so we can stop drawing and
    run the audio-only cadence.

11. **[should] `events_wait` needs a precise contract, and a way for other threads to wake the main one.** Draft: reference, Time and
    events. Our model is: `tick()`; then wait until `take_wake()` (the minimum of every `request_wake`, always at most now + 10 ms)
    or an event; `events_wait(buf, max, timeout = wake - now)` is the right mapping (this is exactly our desktop loop), and
    it covers audio-clock presentation too, because the audio clock is read inside the tick, not in the wait. It needs:
    (a) timer resolution and slack stated (we need <= 1 ms late, never early beyond a tick; spurious early returns allowed);
    (b) coalescing and overflow: `POINTER_MOVE`, `WHEEL` and `RESIZE` are coalesced, key, button, drop, `TRANSPORT`, lifecycle events
    are **never** dropped, and a bounded queue (say 1024) with a documented overflow policy; (c) event `time_us` is in the
    `time_now_us` clock domain (say so); (d) `events_wake()` callable from **any** thread, making the current or next `events_wait`
    return (an event kind 40 `WAKE`, or zero events): our threaded decoder finishes a frame on a worker and today the main thread
    notices at the next tick (up to 10 ms later); with `events_wake` it can tick at once; (e) `time_now_us` and the `log`/`audio_*`
    calls are callable from any thread (we have worker-side timing and want audio write on a thread later), and `time_now_us` is cheap
    (the executor calls it many times per tick; on wasmi a host call is not free).

12. **[should] `kv` limits, durability and eviction (persistence quotas).** Draft: reference, Settings and files ("Small persistent
    key-value store"). What we store: `library/index` (100 to 200 B per track: 15 to 30 MB for 100k tracks), `library/playlists`,
    `library/queue`, `session/position` (every 5 s), and `library/art/<16 hex>` thumbnails (about 62 KB each: 5,000 albums is
    300 MB). That is not "small". Change: (a) publish the limits and a way to read them: `limit_get(which: i32) -> i64`
    (max key length, max value size, per-app quota, max open handles, max events per wait, max art bytes); we need keys up to 255
    bytes of UTF-8 with `/` in them, values of at least 32 MiB, quota of at least 512 MiB; (b) a second **cache class** the OS may evict
    under space pressure without telling the app (`kv_store_cache(key, ..)` or a `flags` argument on `kv_store`): thumbnails are
    recomputable from tags, the index is not; `kv_load` then returns `-NOT_FOUND` and we rebuild; (c) `kv_load` with `cap = 0` returns the
    length in O(1) (we call it twice to size a buffer); (d) `kv_store` returns after the value is queued and is atomic (never a
    torn value after a crash: write-then-rename or journalled), with a `kv_flush()` we call on `TERMINATE` and before a restart;
    a store of 15 MB must not block the tick for a disk flush; (e) `-NO_SPACE` leaves the old value in place; (f) enumeration is
    not needed.

13. **[should] Audio: precise semantics for the clock inputs, and failure and device changes.** Draft: reference, Audio output.
    Our master clock is `heard = written - queued_frames / rate - output_latency`, so correctness depends on definitions:
    (a) `audio_queued(h)` counts only frames **not yet handed to the device** and `audio_latency_us(h)` is the delay from hand-off
    to audible, with no overlap (a driver DMA buffer counted in both double-counts and the video runs early); say it, and say
    `audio_queued` falls smoothly (sample-accurate), not in device-period steps; (b) a coherent snapshot is better than three calls:
    `audio_clock(h, out)` writing `{ struct_size, frames_played: u64, host_time_us: i64, latency_us: i64, underruns: u32 }` read
    atomically (the browser adapter extrapolates `played()` from `currentTime`; a stamped sample is what that code wants);
    (c) ring capacity: tell us via the `audio_open` out struct (`capacity_frames`) and guarantee at least 1 s (our web and desktop
    sinks hold 1 s; our `audio` task writes up to about 0.5 s ahead); `audio_write` accepts a partial write and never blocks;
    (d) `audio_open` returns channels in {1, 2} only (we mix to stereo ourselves, including gapless and surround downmix; we do
    not want multichannel from the host) and the rate may differ (we resample); say the legal set (44.1, 48, 88.2, 96 kHz at
    least); (e) paused: `audio_queued` is frozen, nothing is lost, resume is click-free; (f) `audio_flush` also resets
    `frames_played` and `underruns`; (g) events `AUDIO_DEVICE_CHANGED { rate, channels }` (default output switched, headphones
    unplugged: we reopen) and `AUDIO_ERROR` (`-IO`/device lost); today our desktop host shows "The audio device went away" and
    reopens on the next play, and wants the same signal; (h) samples outside -1..1 are clamped by the host, never wrapped
    (our limiter normally prevents it, but a codec bug must not blast); (i) per-stream volume stays 0..1 and a system volume
    change (hardware keys) is the host's business and not an app event unless it is `TRANSPORT SetVolume` (item 15).

14. **[should] Wake-ups while idle.** Draft: reference, `events_wait`. Today our session asks for a wake at `now + 10 ms` on every tick, even
    when paused with nothing to do (we will lengthen that: item 28). Please make an `events_wait` return with no events cheap
    (no allocation, no scheduler churn), and let an app that waits with a long timeout and an armed `frame_request(off)` cost
    nothing, so the Resource Guard does not count Rusty Wave's idle polling against it.

15. **[should] Media keys, transport routing and focus.** Draft: reference, `TRANSPORT`, key table. We need the OS to say: (a)
    media keys (Play/Pause, Next, Prev, Stop, volume up/down/mute) reach the **active media session** as `TRANSPORT` events even when the
    app is not focused, and are **not** also delivered as `KEY_DOWN`; (b) the rule for which app is the active session
    (last one to report `now_playing_playback` with state Playing; a second media app must not steal it silently) and a way
    to give it up (`now_playing_playback` state Stopped for 30 s, or an explicit `now_playing_clear()`); (c) named keys for
    when the app is focused and the shell does not handle them: `MediaPlayPause`, `MediaNext`, `MediaPrev`, `MediaStop`,
    `VolumeUp`, `VolumeDown`, `VolumeMute`, `Back`, `Forward` (kind `0x0011_0000 + 60..`); (d) `TRANSPORT` payload layout:
    `command: u32` at payload offset 0, `value` (an `i64` microsecond count, or an `f32` in the low four bytes, little-endian)
    at payload offset 8 so it is 8-aligned; unknown commands are ignored; (e) `SeekTo`/`SeekBy` are in microseconds and
    `SetVolume` is 0..=1 (it matches our `TransportCommand`); the shell must not send `Next` when `can_next` was false,
    or we ignore it (we do; say it is allowed to be sent).

16. **[should] Input gaps: cursor, fullscreen state, pointer capture and leave, wheel units, Space.** Draft: reference, events 1 to 10;
    Canvas. (a) **Cursor**: no way to set the cursor or hide it. A video player hides it after 3 s in fullscreen, and uses
    pointer, text, grab, resize cursors (our web host and desktop host both do this). Add `cursor_set(kind: i32) -> i32`
    (default, pointer, text, grab, grabbing, ew-resize, ns-resize, none). (b) **Fullscreen**: `canvas_fullscreen(on)` has no
    state back; the user leaves with Esc or the shell; add `FULLSCREEN` event or a `flags` bit on `RESIZE` and `canvas_info`.
    (c) **Pointer capture and leave**: a slider drag must keep receiving `POINTER_MOVE` and `POINTER_UP` outside the window; add
    `pointer_capture(on)` or define implicit capture while a button is down; add `POINTER_LEAVE` (we clear hover states). Touch is not
    needed in v0. (d) **`WHEEL`**: say the sign (positive `dy` = scroll down, as ours) and the unit (lines vs pixels: define one line as
    N pixels at scale 1, or deliver pixels with a flag; we scroll lists and set volume by the wheel). (e) **`KEY`**: the table lists
    `Space` as named key 14 *and* says keys that type a character use the scalar value, which makes U+0020 ambiguous; pick one
    (our `Key::Space` and `Key::Char(' ')` are different variants, we accept either). Named keys also need Page Up/Down (have),
    `CapsLock`/`NumLock` (nice), and the F-keys (have). (f) **Text input/IME**: our library search takes characters; add a `TEXT`
    event with a text handle for composed input (nice, v1 can do without). (g) Pointer coordinates are **physical pixels** as `f32`
    (our `InputEvent` uses physical pixels, `Resize { w, h, dpr }`); the draft says "canvas pixels": confirm they are the same thing.
    `RESIZE` must also fire when only `scale` changes (the window moved to another monitor), and the fractional scale (1.25, 1.5) is fine.

17. **[should] Idle and sleep inhibit.** Missing from the draft. A video must not blank the screen or suspend the machine while it
    plays; audio must not either (sleep, not the screen). Add `power_inhibit(kind: i32, on: i32) -> i32` (1 = keep display on,
    2 = keep system awake), released automatically when the app exits or traps. (Our browser host does not use the Wake Lock API
    and our desktop host does not call the screensaver inhibit yet; both are on our side to add: this is an App API gap and a to-do for us.)

18. **[should] Files: multi-select pick, save, launch arguments, sibling files, drop details.** Draft: reference, Settings and files;
    events 9, 33. (a) `file_pick(kinds, kinds_len)` has no `flags`: add `multi` (our `PickFile` and `AddFiles` pick several
    files; `FILE_PICKED` then repeats with a `flags` "more follows" bit, or add a `file_pick_count`); add a `folder` mode that is
    not the library (nice). Say a cancelled pick delivers `-CANCELLED` (item 19), not `-DENIED`. (b) **Save**:
    `Effect::Download { name, mime, data }` (an exported playlist) needs `file_save(name, name_len, mime, mime_len, data, len)
    -> request` with a `FILE_SAVED { request, status }` event (save-as dialog, or a Downloads folder: say which). (c) **Launch
    arguments and "Open with"**: `[[opens]]` is in the manifest but the app has no way to learn which file it was started with, or
    that a second file was opened while it runs (single instance). Add an `OPEN` event (kind 36+, `file: i32` handle, `flags`
    bit 0 = "while running") delivered before the first `events_wait` returns, one event per file. (d) **Sibling files**:
    subtitles next to the video and `cover.jpg` next to a track are normal; today we only get files the user handed over. Add
    `file_open_sibling(h, name, name_len) -> i32` (same folder, read-only, same grant). (e) **Drop**: `DROP.file` is an *open
    handle*: say who closes it, that the app owns it like any handle, how many handles an app may hold (we need at least 64: the
    library scanner and gapless each hold some), and that a multi-file or folder drop is **one event per file** (a drag
    of 200 tracks must not overflow the queue: item 11b). Our adapter will turn each into `InputEvent::Drop { id }` with
    `file_id(h)` and close the handle if the id reopens it. (f) `file_size` of a stream of unknown length: pick a sentinel
    other than `-1` (which is `-INVALID`); we propose `-UNSUPPORTED`. (g) `file_id` for a bundle file or a file with no stable
    id returns `-UNSUPPORTED`, and `Host::stable_ids()` becomes per-file at the adapter (we need only the "all or nothing" bit, so
    say whether *every* id from `file_id` is stable).

19. **[should] Error model.** Draft: app-api.md, Error codes. (a) A cancelled picker is reported as `-DENIED`, which also means
    "no permission": add `-10 CANCELLED`. (b) Add `-11 NO_DEVICE` (audio device lost or none) and `-12 TIMEOUT`;
    keep the numbering append-only. (c) Several results are `i32` counts and lengths: say that lengths are limited to
    `2^31 - 1` and that functions returning a length never return a negative that is not an error. (d) `-INVALID` for an
    out-of-range pointer is right (never trap); please also say what happens for an unknown handle (`-NOT_FOUND`, as listed) vs a handle
    of the wrong kind (`-INVALID`). (e) `file_read_at` returning `0` at end of file and `0` for "nothing yet" must not be
    confused: `-BUSY` is the latter (item 2).

20. **[should] Capabilities change at run time, and the draft says "right now".** Draft: app-api.md, Capabilities. `VISUALIZER`
    is "the shell wants the visualizer feed": that flips whenever the shell shows or hides its visualizer. Our core computes the
    analysis only while `Host::visualizer()` is `Some`, so the adapter must learn the change. Add a `CAPS_CHANGED` event (and
    say which bits can change: `VISUALIZER`, `AUDIO_OUT` after a device appears, `NOW_PLAYING`, `LIBRARY` after a permission
    change, `THREADS` never). Also: `caps()` bits for `THREADS` and `ATOMICS` are redundant with the chosen build (item 6) but fine.

21. **[should] `now_playing_*` layouts: field by field.** Draft: reference, Media shell.
    - `now_playing_metadata` 56 bytes: `title (0,8) artist (8,8) album (16,8) art_mime (24,8) art (32,8) duration_us i64 (40)
      has_video u32 (48)` + 4 pad = 56: the size is right and matches our `NowPlayingMeta` field for field (`Art { mime, data }`
      becomes two pairs; `Option` maps to `(0,0)`; `duration_us: Option<i64>` maps to -1 for `None`). Please write the offsets in the doc
      as above. Add `struct_size` (item 3). State: strings are valid UTF-8 (the host rejects with `-INVALID` or replaces; say
      which), an empty string means unknown, a maximum string length (we send up to a few KB of tag text), a maximum `art` size
      (covers are 100 KB to 5 MB; give a limit of at least 8 MiB and `-TOO_LARGE` beyond), the host copies during the call, and sending
      the same item again (tags arrived, so we resend with art) replaces it. The shell must decode PNG and JPEG (progressive too).
    - `now_playing_playback` 24 bytes: `state u32 (0), rate f32 (4), position_us i64 (8), can_next u8 (16), can_prev u8 (17),
      can_seek u8 (18)` + 5 pad = 24: size right, fields match our `Playback` (`state` numbering 0 stopped, 1 playing, 2 paused is what our web
      adapter already uses). The extrapolation rule (position + elapsed x rate while playing, resent only on state change, rate change,
      a jump over 0.5 s) is correct and must be on the shell side. Add the clock the shell extrapolates with: `time_us` in
      `time_now_us` terms (a `host_time_us: i64` field) so a delayed delivery does not skew the position.
    - Nice: MPRIS and Media Session also show shuffle, repeat, volume, track number, album artist, year, genre. Our
      `NowPlayingMeta` and `Playback` do not carry them yet (our side: item 28), so `struct_size` (item 3) is the only thing needed now.

22. **[should] Event payload layout for `DROP`, `RESIZE` and others is only listed as field names.** Draft: reference, event table.
    Write byte offsets for every kind, in the same way as the structs (payload starts at 16; `i64` and `f32` pairs are
    naturally aligned): e.g. `RESIZE`: `w u32 (16), h u32 (20), scale f32 (24)`, `flags` bit 0 = fullscreen; `KEY_DOWN`: `key u32 (16), mods u32
    (20)`; `POINTER_*`: `x f32 (16), y f32 (20), button u32 (24)`. Say text handles are valid until the next `events_wait`, are
    copied with `event_text`, and that `event_text` for a bad handle returns `-NOT_FOUND`.

23. **[should] Manifest: app ID, memory, optional AUDIO_OUT, file types.** Draft: bucket-format.md, manifest example. (a) The example
    ID is `com.unicorntears.rustywave`; our published ID is `io.github.idometeor.RustyWave` (desktop file, AppStream,
    MPRIS name, Flatpak, icons). The ID "never changes": pick one now. We prefer to keep ours for every host; if RB needs a
    reverse-DNS under its own domain, say so and we will map it in `rvp-host-rb` only (the manifest is RB's file). (b) `AUDIO_OUT`
    should be **optional**, not required: the app plays video without a device (our desktop host has a silent mode driven by the
    wall clock); `CANVAS` stays required. (c) `[[opens]]` extensions: our list is `mp4 m4v mkv webm mka mp3 mp2 flac ogg oga
    opus wav m4a m4b aac` plus `srt vtt` (subtitles) and `m3u m3u8 pls` (playlists), with roles: media = `view`, playlists and subtitles =
    `view` too. (d) `memory_max_mb`: the example 512 is too low for threads plus 4K AV1 plus the library; our web threads build
    declares 2 GiB; say what the OS does when the declared maximum is more than it grants (instantiate with a smaller
    maximum and let `memory.grow` fail, or refuse).

### Nice

24. **[nice] `viz_*` timing and the layout.** Draft: reference, `viz_block` and `viz_summary`. Layout check, field by field against
    `VizSummary`: `pts_us i64 (0), level f32 (8), peak f32 (12), bands [f32;32] (16..144), bass (144), mid (148), treble (152),
    onset u32 (156), onset_strength f32 (160), tempo_bpm f32 (164)` = 168 bytes exactly (the word "padding" in the draft is wrong:
    there is none); `onset` is 0 or 1. `viz_block` matches `VizBlock` (`rate: i32` vs our `u32` is fine; `frames` is the
    count of frames, not samples; interleaved `f32`). State: both calls are delivered at the moment the audio is **heard**
    (we have already removed device latency), so the shell should draw them on receipt; the cadence is one `viz_block` per
    tick (10 to 20 ms, so 50 to 100 calls a second with 1 to 4 KB each) and about 94 `viz_summary` a second; they stop while paused
    and the timestamps jump back on a seek (the shell must not assume monotonic `pts_us`); the shell must not call back into
    the app from inside them. If the shell cannot afford 100 cross-process calls a second, offer a batched `viz_summary_n`.

25. **[nice] Reserve HDR fields in the video frame.** Draft: Video. Our `VideoFrame` has `matrix` and `range` but no transfer
    function or primaries; BT.2020 content is treated as SDR today. Reserve `transfer: u32` (0 unspecified, 1 sRGB/BT.709, 2 PQ,
    3 HLG) and `primaries` in the `flags` word or a reserved field now (item 1), and a `DISPLAY_HDR` bit in `canvas_info.flags`,
    so adding tone mapping later does not need a new major.

26. **[nice] Output device selection and list.** Missing: `audio_devices(buf, cap)` and `audio_set_device(id)`; the browser does
    not have it either (Chromium has `setSinkId`), so it can wait.

27. **[nice] Clipboard and theme.** Clipboard is listed under "planned"; we need only "copy track info or path" (text). We
    do not consume `theme_get` in v1 (the UI is drawn with Unicorn Tears tokens on purpose, a project decision in `CLAUDE.md`) and we bundle
    our fonts (Space Grotesk, JetBrains Mono, OFL), so `/system/fonts` is not needed for us.

28. **[nice] Our side (RVP), not requests.** We will: lengthen the idle wake when paused (item 14); add idle/sleep inhibit to the
    browser and desktop hosts (item 17); add shuffle, repeat, volume and track fields to `NowPlayingMeta`/`Playback` in an
    additive way (item 21); keep `TICK_US` as the upper bound of the wake, and let the adapter call `file_read_at` retry-style (item 2).
    None of this changes what we ask of RB except as written above.

## Confirmed: no API need

- **Crossfade and auto-level (loudness normalisation).** We mix, crossfade and level-adjust inside the core before `audio_write`, on a
  single continuous stream: the same stream is kept across gapless item changes (our `AudioSink::open` is called again only
  after an item without sound), the rate and channel count are fixed for the life of the stream (we resample and
  downmix), and nothing is pre-scheduled in the host. So no new call is needed. Two prerequisites already listed: ring capacity
  of at least 1 s (item 13c) and clamping (item 13h). Crossfade needs *no* extra look-ahead in the host: our own buffer holds the
  overlap.
- **Mix of 10 ms ticks and the audio clock.** The clock is read inside the tick from `audio_queued` and `audio_latency_us`; no
  extra callback is needed from the device (item 13 only tightens the definitions).
- **Mapping table.** The one-to-one mapping in app-api.md is right for `HostClock`, `AudioSink`, `Surface`, `InputEvents`,
  `Storage` and `NowPlaying`. Corrections: `Library` has no `rescan`/`forget` call (item 5); `Source` is not ready-future-only
  (item 2); `VideoSink::present` has no `dest` (item 9); `Host::stable_ids()` is per host, not per file (item 18g).

## What is good

- Core wasm with plain imports and no WASI: right for both engines and for a cross-compile of our `no_std + alloc` core.
- The conventions (full length returned, `min(len, cap)` written, host-checked ranges that never trap, a handle owner per app,
  host never keeps a pointer) are the right ones and make the adapter a thin layer.
- `events_wait` with a relative timeout is the same loop our desktop host already runs, so `request_wake` maps without change.
- Microseconds as `i64` everywhere, f32 interleaved audio, RGBA8 straight alpha, physical pixels and a `scale`: all the same units as ours.
- `now_playing` and `viz` shapes are lifted straight from `rvp-host`, and the sizes of `meta` (56), `pb` (24) and `viz_summary` (168) are right.
- Capability bits for threads, SIMD and each shell feature, a single-threaded fallback with `-UNSUPPORTED`, and the Simulator running the same imports.
- `MEMORY_PRESSURE` maps onto our thumbnail budget (`App::set_thumb_budget`), `TERMINATE { grace_ms }` onto `App::save_state`.
