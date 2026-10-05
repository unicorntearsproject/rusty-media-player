# rusty-video-player: Plan

> Status: Milestones 0 to 9 done; scope widened 2026-10-05 (standalone audio and video app, two editions, milestones
> M7 to M12). Decisions live in `CLAUDE.md`; this file is the architecture and the milestone list. Update it when
> a decision changes.

## 1. Goal and non-goals

RVP is a standalone media app for **audio and video**, written in Rust, with a portable core that compiles to
`wasm32-unknown-unknown` and runs natively. One codebase ships as two editions:

- **(a) Standalone:** an installable web app (PWA) and a native Linux desktop app (Flatpak, AppImage). Neither
  needs Rusty Bucket.
- **(b) Rusty Bucket's built-in Media app** (`../rust-os`), through the `rvp-host-rb` adapter
  (rust-os ADR-0023: RVP is the built-in player for video and audio; ADR-0026: RVP stays host-neutral and
  Rusty Bucket's App API media interfaces are modelled on RVP's `rvp-host` traits, not the reverse).

**Standalone rule:** `rvp-core`, `rvp-host`, `rvp-player`, `rvp-ui`, `rvp-app` and the codec, demux and library
crates never depend on Rusty Bucket. Rusty Bucket is one more host (`rvp-host-rb`), alongside the browser, the
desktop and headless hosts. Rusty Bucket-only extras (the Bucket Bar widget, streaming) are built on top of the
host traits, never by changing them. The headless and browser hosts are also the dev/test hosts.

- **In scope (v1):** containers MP4, MKV, WebM; video H.264 (our own decoder), AV1, VP9; audio AAC, MP3, FLAC,
  Opus, Vorbis; seek, pause, speed, volume, playlist, SRT/WebVTT subtitles; our own UI.
- **In scope (after v1, M8 to M11):** gapless playback; a host-neutral now-playing model and a visualizer tap;
  an audio-first view (library, playlists, queue, album/artist/track views, built-in visualizer); a native
  desktop host and packaging (PWA, Flatpak, AppImage, MPRIS).
- **Out of scope:** optical discs, network protocols other than "the host hands us bytes", streaming
  adaptive formats (HLS/DASH), skins, filters/effects, transcoding, DRM, interlaced H.264 (PAFF/MBAFF)
  until after v1, anything in VLC's `modules/` not listed above.
- **Clean-room:** `../vlc` is read only for architecture and behavior. No code is copied or translated
  (it is GPL/LGPL; this project is MIT OR Apache-2.0). Third-party crates must be MIT/Apache-compatible.

## 2. What we take from VLC (concepts only)

| VLC area | Concept we keep | Our shape |
| --- | --- | --- |
| `src/input` (access -> stream filters -> demux -> `es_out`) | A source of bytes feeds a demuxer; the demuxer emits timestamped packets per elementary stream (ES). An "ES out" layer picks which tracks are selected and routes packets to decoders. | `Source` (host) -> `Demuxer` -> `TrackRouter` -> per-track decoder queue |
| `modules/demux`, `modules/codec` | Demuxers and decoders are plug-ins behind small interfaces, selected by probing. | `Demuxer` and `VideoDecoder`/`AudioDecoder` traits, a static registry (no dynamic loading) |
| `src/clock` | One **main clock** maps stream time to system time; **audio is the master** when present, else the monotonic clock. Each ES clock converts a pts to a "when to render" using the main clock, rate and a few points of history. Discontinuities reset it. | `MasterClock` in `rvp-core` (section 6) |
| `src/audio_output` | A pull/push output with a software-visible latency; the stream is time-stretched or dropped/padded for small drift; volume and mute live in the output. | `AudioSink` (host) + `AudioPipeline` (resample, drift correction, volume) |
| `src/video_output` | A display thread waits until a frame's render date, drops late frames, shows the last frame when paused, and runs a small queue of decoded pictures. | `VideoQueue` + `present()` decision in the player tick |
| `src/player` | A **player** object: one input at a time, a command API (play, pause, seek, rate, track select), asynchronous **events** (state, position, tracks, errors), a **timer** API for UIs. The UI never touches the pipeline. | `Player` command/event API (section 7) |
| `src/playlist` | A list of items with next/prev, repeat, shuffle. | `Playlist` in `rvp-player` (small) |

VLC's threading model (an input thread, a decoder thread per ES, an audio thread, a vout thread) does not
transfer: wasm32-unknown-unknown has no threads by default. Our model is in section 5.

## 3. Crates

Workspace layout (`crates/*`, plus `xtask`). All crates are `MIT OR Apache-2.0`.

| Crate | `no_std`? | Role |
| --- | --- | --- |
| `rvp-core` | `no_std + alloc` | Shared types: `Rational`, `Timestamp` (microseconds, `i64`), `StreamInfo`, `Packet`, `VideoFrame` (planar YUV, strides, colour info), `AudioBuffer`, error types, `MasterClock`, `RingBuffer`. No I/O. |
| `rvp-host` | `no_std + alloc` | The **host trait set** (section 4): `Source`, `AudioSink`, `VideoSink`, `Surface`, `InputEvents`, `Storage`, `HostClock`, plus (M8) the host-neutral `NowPlaying` model and `VisualizerTap`, and (M10) the optional `Library` capability (directory listing). Mock implementations for tests. This is the source of truth for media interfaces; Rusty Bucket's App API follows it. |
| `rvp-demux` | `no_std + alloc` | Our own incremental demuxers: ISO BMFF (MP4/M4A), Matroska/WebM (EBML), and (M9) raw audio: MP3 (ID3v1/v2, Xing/Info/LAME gapless), native FLAC, Ogg (Vorbis, Opus, FLAC), WAV (PCM, RF64), ADTS AAC; tags and cover art for all of them; plus probing. Async over `Source`. |
| `rvp-par` | std (wasm32 with shared memory) | (M9) Worker pool behind `rvp-core::par::Parallel`, `ThreadedVideoDecoder` (a decoder on a thread of its own), the pipelined H.264 decoder (parse and reconstruction threads). Native threads, or Web Workers through a host-provided spawner. |
| `rvp-codec-audio` | std (wasm32 ok) | `AudioDecoder` impls: AAC, MP3, FLAC, Vorbis (symphonia codec crates, unmodified), Opus (`ropus`), PCM (own, M9). Resampler (rubato). |
| `rvp-codec-h264` | `no_std + alloc` | **Our own** H.264 decoder (M6), `forbid(unsafe_code)`. Public modules that an encoder can share (Rusty Bucket plans one): `bitstream` (NAL/AVCC/Annex B, RBSP escaping, `BitReader` and `BitWriter`, Exp-Golomb), `params` (SPS with VUI, PPS, scaling lists, slice header, pred weight table, MMCO: each has `parse` and `write`), `transform` (inverse and forward 4x4/8x8/DC transforms, quantisation, dequantisation, scans), `cavlc` (tables plus `read_residual_block` and `write_residual_block`), `cabac` (context init tables, arithmetic decoder, arithmetic encoder, binarisation offsets). The picture decoder is `decoder` (macroblock layer, intra/inter prediction, direct modes, deblocking, DPB, output order); `h264_decoder()` adapts it to `VideoDecoder`. |
| `rvp-codec-av1` | std (wasm32 ok) | rav1d wrapper (needs a wasm32 patch, see risk R1). |
| `rvp-codec-vp9` | std (wasm32 ok) | VP9 behind our `VideoDecoder` trait (M7): wraps `rusty_vp9` (Apache-2.0, pinned `=0.1.1`), adds the superframe pull loop, `VideoFrame` conversion with colour tags, size caps and key-frame gating. Both candidate crates are `std`-only, so this crate is not `no_std`. |
| `rvp-subs` | `no_std + alloc` | SRT and WebVTT parsers, MKV/MP4 subtitle payload decoders (`S_TEXT/UTF8`, `S_TEXT/WEBVTT`, `tx3g`, `wvtt`), `CueList` (what is on screen at t). |
| `rvp-viz` | `no_std + alloc` | Visualizer analysis (M8, done): own radix-2 FFT, 32 log bands, level, adaptive onset detection and autocorrelation tempo, as `VizSummary`s from the audio being heard. In M10 also the effect set drawn into an RGBA buffer by `rvp-ui` (Unicorn Viz spirit). |
| `rvp-library` | `no_std + alloc` | (M10) Library model: scan results, track/album/artist index, tags and cover art (via symphonia metadata, behind a std feature), queue, playlists with M3U/M3U8/PLS import and export. Persistence goes through `Storage`; directory walking through the `Library` host capability. |
| `rvp-player` | `no_std + alloc` | The engine: `Session` (cooperative tasks, pipeline, A/V sync, exact seek, frame step, A-B loop, subtitles, audio-track switching, WSOLA speed, gapless chaining, visualizer tap feed), `Playlist` (order, repeat, shuffle), events. Codecs come in through `CodecFactory`. |
| `theme` | `no_std` | Design-system tokens as `const`s (colour, spacing, radius, type scale, motion, glow). Generated by `cargo xtask theme` from `crates/theme/tokens/*.css`, a snapshot of `claude-design-system/tokens`. |
| `rvp-app` | `no_std + alloc` | The application glue (added in M5): owns the `Session` and the `Ui`, applies `Action`s to the player, builds the `UiModel`, composes picture + chrome into one RGBA surface and presents it. Hosts only forward input and call `App::tick`. Needs a `Host` whose video sink is `rvp_host::FrameSink`. |
| `rvp-ui` | `no_std + alloc` | Our UI: a small immediate-mode toolkit drawn into an RGBA framebuffer (rects, rounded rects, gradients, pre-rendered glow sprites, text via `fontdue`, Lucide icons as paths). Player view: video area, transport bar, seek bar, volume, playlist drawer, context menu, subtitle overlay. From M10 also the audio-first views (library, album/artist/track, queue, visualizer). Same pixels in browser, desktop and Rusty Bucket. |
| `rvp-host-headless` | std | Native host for tests: file `Source`, a virtual-time clock, a null/WAV audio sink, a frame-hash `VideoSink`, scripted `InputEvents`. Binary `rvp-headless`. |
| `rvp-host-web` | wasm32 only | `wasm-bindgen` cdylib: File API `Source`, WebAudio `AudioSink`, `<canvas>` `Surface`, DOM input, `requestAnimationFrame` clock; Media Session API (now-playing, M8); PWA manifest and service worker (M11). |
| `rvp-host-desktop` | std | (M11) Native Linux app: `winit` window, `softbuffer` `Surface`, `cpal` `AudioSink`, real files and drag-drop, MPRIS now-playing and media keys. Binary `rvp`. |
| `rvp-host-rb` | wasm32 only | Rusty Bucket adapter (M12): maps `rvp-host` traits (including now-playing and the visualizer tap) to the App API. Stub until its app ABI exists. |
| `xtask` | std | `cargo xtask theme | fixtures | web | serve | e2e | check | licenses`; from M11 also `desktop`, `flatpak`, `appimage`. |

Why our own demuxers: they must be incremental, seekable, `no_std`, and async over a host `Source`;
every candidate crate (section 8) is `std` and blocking `Read + Seek`. They use `matroska-demuxer`, `mp4` and
`symphonia` as **dev-dependency oracles** in tests only.

## 4. The host trait

One small trait per concern, bundled in `Host`. All I/O is `async` (stable async fn in traits); the player
polls these futures itself (section 5), so a browser `Promise` and a native blocking read look the same.

```rust
pub trait Host {
    type Source: Source;
    type Audio: AudioSink;
    type Video: VideoSink;
    fn clock(&self) -> &dyn HostClock;
    fn audio(&mut self) -> &mut Self::Audio;
    fn video(&mut self) -> &mut Self::Video;
    fn surface(&mut self) -> &mut dyn Surface;
    fn input(&mut self) -> &mut dyn InputEvents;
    fn storage(&mut self) -> &mut dyn Storage;
    fn now_playing(&mut self) -> Option<&mut dyn NowPlaying> { None }   // M8, optional
    fn visualizer(&mut self) -> Option<&mut dyn VisualizerTap> { None } // M8, optional (see docs/host-api.md)
    fn library(&mut self) -> Option<&mut dyn Library> { None }          // M10, optional (directory access)
    async fn open(&mut self, req: OpenRequest) -> Result<Self::Source, HostError>; // file picker, drop, path, URL id
}

pub trait HostClock {                 // monotonic, microseconds, never goes back
    fn now_us(&self) -> i64;
    fn request_wake(&self, at_us: i64); // "tick me no later than"; host may tick earlier
}
pub trait Source {                    // random access, so seeking works without buffering the file
    async fn size(&self) -> Option<u64>;
    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, HostError>;
    fn name(&self) -> &str;
}
pub trait AudioSink {                 // push model with a pull-friendly ring on the host side
    fn open(&mut self, want: AudioParams) -> Result<AudioParams, HostError>; // host may pick its own rate/channels
    fn queued_frames(&self) -> usize;        // frames written but not yet played
    fn output_latency_us(&self) -> i64;      // device + host buffering
    fn write(&mut self, interleaved_f32: &[f32]) -> usize; // returns frames accepted
    fn set_paused(&mut self, paused: bool);
    fn set_volume(&mut self, v: f32);
}
pub trait VideoSink {                 // video area of the window
    fn present(&mut self, frame: &VideoFrame); // YUV in; the host or core converts to RGBA for the surface
}
pub trait Surface {                   // the UI canvas
    fn size(&self) -> (u32, u32, f32 /*device pixel ratio*/);
    fn present_rgba(&mut self, rgba: &[u8], dirty: Rect);
    fn set_fullscreen(&mut self, on: bool);
}
pub trait InputEvents {               // keyboard/mouse parity with Rusty Bucket's model
    fn poll(&mut self) -> Option<InputEvent>; // Key, Pointer{move,down,up,wheel,buttons}, Resize, Drop, Focus
}
pub trait Storage {                   // settings, recents, resume positions
    async fn load(&mut self, key: &str) -> Option<Vec<u8>>;
    async fn store(&mut self, key: &str, value: &[u8]);
}
```

Added in M8 (host-neutral, no Rusty Bucket types; these are the source of truth that Rusty Bucket's App API
media interfaces follow, rust-os ADR-0026). The shapes, rules and mapping hints are in [`host-api.md`](host-api.md):

```rust
pub trait NowPlaying {                // MPRIS-like; the host mirrors it to its OS or shell
    fn set_metadata(&mut self, m: &NowPlayingMeta);   // title, artist, album, art, duration
    fn set_playback(&mut self, p: &Playback);         // state, position, rate, can_next/prev/seek
    fn poll_command(&mut self) -> Option<TransportCommand>; // Play, Pause, Toggle, Stop, Next, Prev, SeekTo, SeekBy, SetRate, SetVolume
}
pub trait VisualizerTap {             // fed with what is heard (device latency removed), pre-volume
    fn push_block(&mut self, b: &VizBlock<'_>);       // interleaved f32 PCM plus stream time
    fn push_summary(&mut self, s: &VizSummary);       // 32 log bands, level, onset flag, tempo (about 94 a second)
}
```

The player raises metadata and state changes, the host forwards them (browser: Media Session API; desktop: MPRIS;
Rusty Bucket: the shell's media interface). The analysis is computed in core (`rvp-viz`) so every host gets the same
numbers, and only while the host provides a tap.

Rules: the core never blocks, never spawns, never reads a global clock, and never allocates in a hot path
without reason. Everything time-dependent goes through `HostClock`, so the headless host can run a 2-hour
movie in seconds with a virtual clock and get deterministic output.

Mapping to Rusty Bucket (`../rust-os/docs/planning/README.md`; our page: `docs/planning/rusty-video-player.md`; App API draft: `docs/developer/app-api.md`): its apps get a Canvas surface (our `Surface`), input events,
timers (`HostClock`), fs (`Source`/`Storage`), and audio later (HDA; our `AudioSink`). Its D8 rule
(full keyboard *and* full pointer control, right-click menus, scroll, middle-click, back/forward buttons)
is a UI requirement here (section 9). Its draft host API v0 has no audio or file-pick yet; M12 tracks that.

## 5. Threading model (works with no threads)

One **cooperative, single-threaded executor** inside `rvp-player`; hosts call `Player::tick(&mut host)`
from whatever drives them (rAF + `setInterval` in a browser, a loop in headless, the app main loop in
Rusty Bucket). A tick:

1. Drain `InputEvents`, run UI logic, apply commands.
2. Poll the task set once (no-op waker; tasks re-poll every tick, they are cheap when blocked):
   - `demux` task: reads packets (async `Source`), routes them to per-track bounded queues. Back-pressure
     when queues are full (target ~1-2 s of packets).
   - `audio` task: decodes while `queued_frames + decoded_ahead < target` (~200-500 ms), resamples, writes
     to the sink.
   - `video` task: decodes under a **time budget** (e.g. 6 ms or one frame of work, whichever is
     smaller; large frames are decoded in slices where the codec allows it) so UI and audio never starve.
3. Video presentation: look at the head of the decoded-frame queue; compute `render_at = clock.to_system(pts)`;
   present if due, drop if later than a threshold and a newer one is also due, hold otherwise.
4. Draw the UI if dirty, `present_rgba`, then `request_wake(next_deadline)`.

Audio is the realtime-critical path and is **not** in the tick's hands once written: the host owns a ring
(WebAudio: `AudioBufferSourceNode` scheduling with ~100-200 ms lookahead, or an AudioWorklet reading a ring;
native: the device callback). Underruns are reported through `queued_frames`/`output_latency_us` and handled
by the clock (rebuffer, resync), not by blocking.

If a codec cannot meet realtime in one thread (1080p AV1 and the 25 Mbit/s H.264 stress stream in wasm), the escalation
path is, in order: (a) frame skipping up to the next reference-safe frame (VLC-style "late frame" handling),
(b) wasm SIMD128 kernels (M9, always on in the browser build), (c) an **optional** worker pool (M9, built with
`cargo xtask web --threads`): the `rvp-core::par::Parallel` capability plus `rvp-par` (a pool, and a video decoder that runs on a
thread of its own). The page uses it when it is cross-origin isolated (`cargo xtask serve` sends COOP/COEP) and falls back to
the single-threaded build otherwise (`?threads=0` forces that); everything works without it. The browser's main thread never
blocks: hand-over uses spin locks and `unpark`, only workers `park`, and the pool runs jobs serially on the UI thread. Only the
main thread can create Web Workers, so a worker that wants another thread posts a message and the page spawns it.

For Rusty Bucket the same applies, with one extra caveat from its plan: `wasmi` is an interpreter, so
HD decoding will not be realtime there. M12 must either use a JIT/AOT wasm runtime or a native
"codec service" imported through the app ABI; our `VideoDecoder` trait is the seam for that.

## 6. Clock and A/V sync

Clean-room design from the concept above:

- `MasterClock { mode: Audio | Monotonic, base_stream_us, base_system_us, rate, paused, latency }`.
- **Audio master:** after the audio task writes `n` frames ending at stream time `t`, the position being
  heard now is `t - (queued_frames/rate) - output_latency`. The clock is re-anchored to that every tick
  and smoothed (a short moving average) so jitter in `queued_frames` does not cause video judder.
- **Monotonic master** (no audio track, audio disabled, or sink unavailable): anchored at play start,
  scaled by `rate`, frozen on pause.
- `to_system(pts)` / `to_stream(system_now)` for the video side. Pause/resume, rate change and seek
  re-anchor. A **discontinuity** (timestamp jump > threshold, seek, track switch, loop) resets the clock.
- **Drift:** audio never stretches for small errors in v1; if video is late by more than a frame it drops
  frames, if early it waits. For rate != 1 audio is time-scaled (WSOLA, M8) or muted above 2x.
- Subtitles use the same clock (`to_stream(now)`).
- All of it is plain integer/`f64` math over `HostClock` values, so it unit-tests with a fake clock.

## 7. Player API (sketch)

```rust
let mut player = Player::new(host_config);
player.command(Command::Open(item));  // Play, Pause, Seek(Position|Fraction|Relative), SetRate,
                                      // SelectTrack(kind, id), SetVolume, Mute, Next, Prev, Stop, ...
while let Some(ev) = player.poll_event() { /* StateChanged, Position, Duration, Tracks, Buffering,
                                              Ended, Error, Metadata, SubtitleCue */ }
player.tick(&mut host);
```

UI code only sees commands/events and a read-only `Snapshot` (state, position, duration, tracks, volume,
rate, playlist). The same API is exposed to JS by `rvp-host-web` for page integration and testing.

## 8. Candidate crates (checked 2026-10-05)

"wasm32" = `cargo check --target wasm32-unknown-unknown` result in a scratch crate on this machine.
"no_std" = declared in the crate.

| Crate | Version | License | no_std | wasm32 | Verdict |
| --- | --- | --- | --- | --- | --- |
| `symphonia` (+ `-codec-aac`, `-bundle-mp3`, `-bundle-flac`, `-codec-vorbis`, `-core`) | 0.6.1 | MPL-2.0 | no (std) | builds | **Use** as unmodified dep for AAC/MP3/FLAC/Vorbis decode. MPL-2.0 is file-level copyleft: fine unmodified; keep notice in `THIRD_PARTY_LICENSES.md`. Its demuxers are only used as test oracles. |
| `opus-decoder` | 0.1.1 | MIT OR Apache-2.0 | no | builds | **Rejected in M3**: its CELT inverse MDCT uses an O(N^2) DFT with a `sin_cos` per term (`celt/kiss_fft.rs`), so decoding 6 s of audio cost 5 s of CPU even optimised. |
| `ropus` | 0.12.18 | BSD-3-Clause | no | builds | **Used for Opus** (bit-exact port of libopus, float output via `decode_float`; runs ~100x faster). `audiopus_sys` stays **rejected** (C code, bad for wasm). |
| `nanomp3` | 0.2.0 | MIT OR Apache-2.0 | **yes** (`alloc`) | n/a | Alternative MP3 decoder if we want a `no_std` audio path. |
| `lewton` / `claxon` | 0.10.2 / 0.4.3 | MIT OR Apache-2.0 / Apache-2.0 | no | builds | Alternatives for Vorbis / FLAC. |
| `rubato` | 5.0.1 | MIT OR Apache-2.0 | no (alloc-heavy, std) | expected | Resampler (device rate vs stream rate). Verify at M3; else a small own linear/sinc resampler. |
| **`rav1d`** | 1.1.0 | BSD-2-Clause | no | **fails** (38 errors: `libc::{ptrdiff_t, intptr_t, off_t, ENOENT, ...}` missing on wasm32-unknown-unknown, plus ambiguous `.abs()`) | **Use, with a patch.** The failures are libc type/errno imports; the fix is small (replace with `core::ffi`/local consts) and BSD-2 allows a vendored patched copy in `third_party/rav1d` (and an upstream PR). Build with `default-features = false, features = ["bitdepth_8","bitdepth_16"]` (no `asm`), 1 thread. This is risk R1; resolved in M4. |
| `rav1d-safe` | 0.6.0 | **AGPL-3.0 OR commercial** | no | n/a | **Rejected** (licence). |
| `dav1d` | 0.11 | MIT (bindings to C dav1d) | no | n/a | Rejected: C dependency does not fit wasm32-unknown-unknown without a C toolchain target. |
| **`rusty_vp9`** | 0.1.1 | Apache-2.0 | no (`std`: `Vec`, `Arc`, `OnceLock`, `catch_unwind`) | builds and runs in Node | **Used for VP9** (M7): bit-exact on every profile 0 fixture (33 files in all), 3 to 4 times faster than `vp9dec`. See the M7 notes. |
| `vp9dec` | 0.1.1 | MIT | no (`std`, uses `std::thread` for tiles and the loop filter, which fall back to one thread on wasm32) | builds and runs in Node | **Not used** (M7): also bit-exact, but 3 to 4 times slower. Kept as a possible second oracle. |
| `rvp9-decoder` | 0.2.0-alpha.3 | BSD-3-Clause | no | untested | Third option; alpha, needs Rust 1.95. |
| `rusty_h264-decoder` | 0.16.0 | BSD-2-Clause | optional (`std` feature) | builds | **Not shipped and not used**: our own decoder (M6) is checked against ffmpeg alone, which proved enough. |
| `h264-reader` | 0.9.0 | MIT/Apache-2.0 | no | builds | NAL/SPS/PPS parsing only; we may use it for bitstream-level cross-checks, not required. |
| `fontdue` | 0.9.4 | MIT OR Apache-2.0 OR Zlib | **yes** (default `hashbrown`, `simd`) | builds | **Use** for UI text (the same choice as `../rust-os`). |
| `tiny-skia` | 0.12.0 | BSD-3-Clause | `no-std-float` feature | expected | Optional for vector icons/paths in `rvp-ui` (decide in M5; own rect/gradient/glow code is enough for chrome). |
| `hashbrown`, `libm`, `spin`, `bitflags`, `heapless`, `embedded-io` | 0.17.1 / 0.2.16 / 0.12.3 / 2.13 / 0.9 / 0.7 | MIT or MIT OR Apache-2.0 | yes | yes | General `no_std` building blocks, use as needed. |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | yes (v2, `default-features = false`) | yes | Error derive for `std` crates. |
| `wasm-bindgen`, `web-sys`, `js-sys` | 0.2.129 / 0.3.106 / 0.3.106 | MIT OR Apache-2.0 | n/a | target | Browser host. Build with the `wasm-bindgen` CLI (no `trunk`) driven by `xtask`. |
| `pollster` / `futures-lite` | 1.0.1 / 2.6.1 | Apache-2.0 OR MIT | n/a | yes | Native test hosts only; the player has its own hand-rolled poller. |
| `symphonia-metadata` | 0.6.1 | MPL-2.0 | no | builds | **Not needed** (decided in M9): tags and cover art come from our own readers in `rvp-demux` (ID3v1, ID3v2.2 to 2.4, Vorbis comments, FLAC pictures, WAV `LIST/INFO`, MP4 `ilst`, Matroska tags), so M10's library has no tag dependency. `lofty` (0.25.4, MIT OR Apache-2.0) stays the fallback if we ever edit tags. |
| `microfft` | 0.6.0 | MIT | **yes** | expected | Candidate for the `no_std` visualizer FFT (fixed power-of-two sizes); otherwise a small own radix-2 FFT in `rvp-viz`. `rustfft` (6.4.1, MIT OR Apache-2.0) and `realfft` (3.5.0, MIT) are std alternatives, not needed. |
| `winit` | 0.30.13 stable (0.31 is beta) | Apache-2.0 | no | n/a | Desktop window, input, drag-drop, fullscreen (M11). Pin the 0.30 line. |
| `softbuffer` | 0.4.8 | MIT OR Apache-2.0 | no | n/a | **Preferred desktop presenter** (M11): we already produce a finished RGBA frame, so a CPU-to-window blit is enough and avoids wgpu. |
| `pixels` | 0.17.2 | MIT | no | n/a | Alternative presenter (wgpu based, GPU scaling); heavier. Only if softbuffer is too slow at 4K. |
| `cpal` | 0.18.2 | Apache-2.0 | no | n/a | Native audio output (M11); PipeWire, PulseAudio and ALSA on Linux. |
| `rfd` | 0.17.2 | MIT | no | n/a | Native open/save dialogs (portal-aware on Linux), for open file/folder and playlist export (M11). |
| `souvlaki` | 0.8.3 | MIT | no | n/a | Media controls and MPRIS on Linux (M11, via `zbus`, MIT). `mpris-server` (0.10.0, **MPL-2.0**) is the alternative; fine unmodified. |
| `directories` | 6.0.0 | MIT OR Apache-2.0 | no | n/a | XDG paths for the desktop library database and settings (M11). |
| `walkdir` | 2.5.0 | Unlicense OR MIT | no | n/a | Library scan on desktop (M10). |
| `ashpd` | 0.12.3 | MIT | no | n/a | Optional: XDG portals when sandboxed (Flatpak), if `rfd` does not cover it. |
| `matroska-demuxer`, `mp4` | 0.8.1 / 0.14.0 | Zlib OR MIT OR Apache-2.0 / MIT | no | builds | Test oracles only (dev-dependencies). |
| `mp4parse` | 0.17.0 | MPL-2.0 | no | n/a | Not used. |
| `appimagetool`, `flatpak-builder` | n/a | MIT / LGPL-2.1 | n/a | n/a | Packaging **tools** run by `cargo xtask` (M11); not linked and not shipped, so their licences do not touch ours. Bundled runtime libraries inside the packages (for example `libasound`, `libxkbcommon`) keep their own licences and are listed in `THIRD_PARTY_LICENSES.md` per package. |
| `libopus`/`audiopus_sys`, `fdk-aac`, `vorbis_rs`, `minimp3`, `x264`-family | various | C code | no | no | Rejected (C, and GPL for x264). |

Tools (not shipped): `ffmpeg`/`ffprobe` (present on this machine, with libx264, libaom, libsvtav1,
libvpx-vp9, libopus, libvorbis, libfdk_aac, flac, libmp3lame) generate all test fixtures and reference
`framemd5`/PCM output. `wasm-bindgen` CLI 0.2.129 is installed with `cargo install wasm-bindgen-cli --version 0.2.129` (it must match the crate version in `Cargo.lock`); `wasm-opt` is optional (`cargo install wasm-opt` failed to link here, the prebuilt binaryen release binary in `~/.cargo/bin` works).

## 9. UI and look

Source of truth: the Unicorn Tears design system, mirrored into the `theme` crate.

- **Palette (tokens):** ink `#07060d` .. `#342357` surfaces, magenta `#ff2bd6` primary, cyan `#19e3ff`
  secondary, violet `#9d4eff`, lime sparingly; semantic success/warning/danger; the magenta -> violet -> cyan
  "tears" gradient; text `#ffeffb`/muted/dim/disabled; focus ring cyan 2 px.
- **Type:** Space Grotesk (UI), JetBrains Mono (timecodes, stats), Anton for display only. Not bundled
  yet; M5 embeds OFL subsets and records their licences in `THIRD_PARTY_LICENSES.md` (fontdue needs font
  bytes and there are no system fonts in wasm, so bundling is mandatory).
- **Layout (from `../u-studio-video-editor`, "Preview and transport" docs):** the picture is the hero,
  filling the window on `--ink-900` letterboxing; a single slim transport bar below (play/pause, step,
  time, seek bar, volume, speed, tracks, fullscreen), with the **cyan playhead line** on the seek bar and
  tooltips that name their key. The bar auto-hides in fullscreen. Playlist is a drawer, not a permanent pane.
- **Glow only for selection/focus** (u Studio rule: "no glowing static chrome"): the focused control,
  the hovered seek handle, the primary Play button. Glows are pre-rendered 9-slice sprites, never blurred
  per frame. `reduce-motion` honoured. No sparkle in the player chrome (it is looked at for hours); Rusty
  Bucket's shell supplies its own boot sparkle.
- **Input (u Studio + Rusty Bucket D8):** `Space` play/pause; `J/K/L` shuttle (2x, 4x, 8x on repeat);
  `Left/Right` seek back/forward (step sizes decided in M5, Q2), `,`/`.` frame step,
  `Home/End`, `I/O` A-B loop, `M` mute, `Up/Down` volume, `F` fullscreen, `S` cycle subtitles, `A` cycle audio,
  `N/P` next/prev, vim `h/j/k/l` aliases. Pointer: click picture = play/pause, double-click = fullscreen,
  wheel = volume (over bar: seek), middle-click = play/pause, **right-click = context menu**
  (everything the keyboard can do), back/forward buttons = prev/next. Everything is reachable with only
  the keyboard and with only the pointer.
- **Voice:** playful, short, technical (`"Can't decode this one. HEVC isn't on the guest list."`).
- Icons: Lucide (ISC), compiled to path data at build time.
- **Audio-first view (M10):** when the open item is audio (or the user picks Library), the picture area becomes
  a cover-art-forward now-playing screen with the visualizer behind it; a left rail switches Library, Albums,
  Artists, Tracks, Playlists, Queue. Same tokens, same input rules (full keyboard and full pointer, right-click
  menus). The visualizer view is full window and borrows the Unicorn Viz spirit (`../unicorn-viz`, MIT: audio
  reactive demoscene effects driven by FFT bands, beat and tempo tracking): a small set of effects written
  from scratch in Rust (spectrum bars, scope, tunnel/plasma, beat-pulsed particle field), audio-reactive via the
  `VisualizerTap` summary, `reduce-motion` and photosensitivity safe (flash rate capped, off by default for
  reduced motion). Ideas and algorithms may be studied; no code is copied without keeping its MIT notice.

## 10. Test strategy

- **Fixtures** are generated by `cargo xtask fixtures` with ffmpeg into `target/fixtures` (small synthetic
  sources: `testsrc2`, `sine`, `smptebars`, 2-10 s). A handful of tiny files (< 100 KB) may be committed
  under `tests/fixtures` for CI without ffmpeg. No third-party footage is committed.
- **Oracles:** `ffprobe` (container facts), `ffmpeg -f framemd5` / `-f f32le` (decoded output),
  and in-process cross-checks against the dev-dependency crates in section 8.
- **Conformance:** H.264 JVT conformance bitstreams (or ffmpeg-generated equivalents) for each stage;
  VP9/AV1 vectors via the chosen crates' own suites and ffmpeg-made streams.
- **Headless end-to-end:** `rvp-headless play file --frames-hash --audio-wav out.wav --virtual-time`
  reproduces the full pipeline deterministically and is the main regression harness.
- **Browser:** a Playwright script (the machine has the Playwright MCP) loads the page, opens a fixture,
  and asserts on canvas pixels + the JS-visible `Snapshot`.
- Every crate keeps `cargo test` green on native; `no_std` crates also run
  `cargo check --target wasm32-unknown-unknown` (and `x86_64-unknown-none` for `no_std` core crates) in
  `cargo xtask check`.

## 11. Milestones

Each milestone ends with a commit and push. "Done when" is a concrete, automatable test unless noted.

**M0 Plan and scaffold** (this one). *Done when:* `docs/PLAN.md` exists, the workspace builds, `cargo test`
passes on native, `cargo check --target wasm32-unknown-unknown` passes for the wasm-capable crates, the
`theme` crate is generated from the tokens and a test proves the generated file is in sync.

**M1 Core types, clock, host traits.** *(done 2026-10-05)* `rvp-core` types, `MasterClock`, `RingBuffer`, `rvp-host` traits and
mock host, the cooperative task poller, headless host skeleton with virtual clock. *Done when:* clock
unit tests pass (audio-master re-anchoring with jittery `queued_frames` stays within 1 ms/s of ideal; pause,
rate change, seek, discontinuity all re-anchor correctly), and a mock-host test runs the poller for 10
virtual seconds with tasks that wake in the expected order with no real sleeping (< 100 ms wall time).

**M2 Demux: MP4, MKV, WebM.** *(done 2026-10-05; see the notes below)* `rvp-demux` probing + incremental demuxers (moov/fragmented MP4, sample
tables, edit lists; EBML, SimpleBlock/BlockGroup, lacing, Cues, Tracks, codec private data), seeking to the
nearest keyframe, AVCC/HVCC-less H.264 (AVCC) and AV1/VP9 config extraction. *Done when:* for a generated
fixture set (`h264+aac mp4`, `fragmented mp4`, `av1+opus webm`, `vp9+vorbis webm`, `h264+flac mkv`), the demuxer's
stream list, durations, and the per-packet (track, pts, dts, size, keyframe) sequence equal `ffprobe -show_packets`
exactly; random-seek tests land on a keyframe <= target for 1000 random targets; a `Source` that returns
1-byte reads and one that returns `Pending` for 3 polls first still produce identical results.

*M2 notes (deviations from the text above, all intentional):* packets are emitted in file order and compared
with ffprobe **per stream** (ffprobe interleaves differently); `dts` is compared for MP4 only (Matroska has no
decode timestamps, so `dts == pts` there); the container duration matches ffprobe within 1 ms (100 ms for
fragmented MP4, where ffprobe estimates it); Matroska seeking uses a cluster index built at open instead of
`Cues`; Opus `CodecDelay` is subtracted from audio timestamps (as ffprobe does); fixtures come from
`cargo xtask fixtures` (ffmpeg) and are not committed; a truncation/corruption test checks for panics. The
test suite also needs `ffmpeg` and `ffprobe` (set `RVP_SKIP_FIXTURES=1` to skip).

**M3 Audio decode and output.** *(done 2026-10-05; see the notes below)* `AudioDecoder` trait; AAC/MP3/FLAC/Vorbis via symphonia, Opus via
`opus-decoder`; channel mapping to stereo; resampler; `AudioPipeline` (queue target, volume, mute);
headless WAV sink; optional native `cpal` sink. *Done when:* audio-only playback of every codec fixture
through `rvp-headless` matches `ffmpeg -f f32le` output (lossless FLAC bit-exact; others within the codec's
tolerance: RMS error < -60 dBFS, length within 1 frame), and a seek in the middle resumes within 1
packet of the target with no gap > 20 ms in the WAV timeline.

*M3 notes:* the resampler is our own (`rvp-core::resample`, 32-tap windowed sinc) instead of `rubato`, so the
player core stays `no_std`; codecs are injected through `rvp_core::CodecFactory` instead of cargo features,
so `rvp-player` depends on no codec crate; no `cpal` sink yet (optional). Opus pre-skip, MP3/Vorbis decoder
delay and AAC priming all come from container timestamps (Matroska `CodecDelay`, MP4 edit lists) and the
player trims audio before stream time zero. End padding is not trimmed (Opus +648, Vorbis +320 samples,
MP3 +47 against ffmpeg), all under one codec frame. AAC is LC only with at most 2 channels (a symphonia
limit). Verified: FLAC bit-exact; AAC -159 dBFS, Vorbis -164, Opus -101, MP3 -150 RMS error versus ffmpeg;
seek to 4.0 s resumes at 4.0 s with no gap over 20 ms (`crates/rvp-host-headless/tests/audio.rs`).

**M4 AV1 and the video path.** *(done 2026-10-05; see the notes below)* Patch rav1d for wasm32 (R1), `rvp-codec-av1`, YUV (4:2:0 8/10-bit) ->
RGBA conversion with colour matrix/range, `VideoQueue`, present/drop/hold policy, first full A/V sync in
headless. *Done when:* `cargo check -p rvp-codec-av1 --target wasm32-unknown-unknown` passes; decoding
the AV1 fixture matches `ffmpeg -f framemd5` frame-for-frame (decoded planes bit-exact); end-to-end headless
play of an AV1+Opus WebM with virtual time reports audio/video drift < 1 frame (max |video_pts - clock| <= 1/fps)
over 60 s, with an injected 200 ms decode stall causing frame drops and recovery, not a freeze.

*M4 notes:* rav1d needed only a ~20-line `libc` shim to build for wasm32 (the 38 compile errors were all missing
`libc` items); it is vendored in `third_party/rav1d` and runs in Node at the same output as native
(`cargo xtask wasm-smoke`, 8-bit and 10-bit). It runs single threaded and without assembly. AV1 4:2:2/4:4:4 is rejected
and monochrome is expanded to grey chroma. Verified: decoded planes bit-exact against ffmpeg for 8-bit (150 frames)
and 10-bit (50 frames); a 60 s AV1+Opus file at 13 ms ticks shows 1500 of 1500 frames presented, max |clock - pts|
12 ms (one frame is 40 ms) and max on-screen age 52 ms; with a 200 ms stall injected into one decode, 4 frames
are dropped, drift stays at most 10 ms, the picture freezes for 210 ms and then steps frame by frame again, and
audio is untouched (`crates/rvp-host-headless/tests/video.rs`). `VideoDecoder` gained `drain()`;
`CodecFactory` supplies decoders; unsupported streams (for example H.264 until M6) are skipped with a warning and
the file still plays its audio. Dev and test builds compile third-party crates at `opt-level = 3` without
debug assertions (rav1d's checked mode is 15x slower).

**M5 Browser host and UI shell.** `rvp-host-web`, `xtask web/serve`, `theme`-driven `rvp-ui` (transport,
seek bar, volume, context menu, drag-and-drop/open dialog, fullscreen), bundled fonts, keyboard/pointer model
from section 9, JS test hook. *Done when:* a Playwright test opens the page, loads the AV1 fixture
from a `File`, and verifies (a) canvas pixels at the video rect change over 2 s, (b) `Snapshot.position`
advances at 1x +- 5%, (c) `Space` pauses (position frozen, last frame stays), (d) a seek click on the seek bar
lands within 1 s, (e) every shortcut in section 9 has a context-menu entry (checked by a unit test over the
action table), (f) a screenshot matches the committed golden within a tolerance.

*M5 notes:* done 2026-10-05. `cargo xtask web` builds `rvp-host-web` with wasm SIMD128 (`-C target-feature=+simd128` in
its own target dir) so the pixel loops vectorise, runs `wasm-bindgen --target web` and `wasm-opt` if present, and copies
`web/` to `target/web`; `cargo xtask serve [--port N]` serves it; `cargo xtask e2e [--update-golden|--screenshots]`
builds, makes fixtures and runs the Playwright suite in `tests/e2e` (Chromium; 10 tests). The page is a single `<canvas>`:
the Rust `rvp-ui` draws picture and chrome into one RGBA surface (video is converted to RGBA and bilinear-scaled in wasm,
so Rusty Bucket's Canvas gets identical pixels) and `web/main.js` only forwards input and presents. Audio is an
AudioWorklet reading a ring buffer (`web/audio-worklet.js`; ScriptProcessor fallback in `web/audio.js`) that reports frames
played, so the audio clock is the master clock; the browser opens an `AudioContext` at 48 kHz up front, resumed on the first
gesture. The browser `Source` reads 1 MiB chunks with next-chunk prefetch. Decisions: **Q2** arrows seek 5 s, Shift+arrows
30 s, `J`/`L` 10 s (reverse shuttle needs reverse decode, so J/K/L follow web-player convention: back 10 s, play/pause,
forward 10 s; deviation from section 9's shuttle), volume 5% per step, `[`/`]` speed step, `\` normal speed, `O` open,
`A`/`S` cycle tracks (stubs until M8: a toast says so), Tab cycles buttons with a cyan focus ring, Menu key or Shift+F10
opens the context menu, and the menu is fully keyboard driven. Not done in M5 (kept for later milestones): frame step,
A-B loop, playlist keys (`N`/`P`, back/forward buttons), chapter and subtitle rendering, audio/subtitle track switching.
Speed 0.25x to 4x works (`Session::set_rate`), but audio is resampled (pitch follows speed) until M8's WSOLA; changing speed
restarts the pipeline at the current position (a short rebuffer). The controls auto-hide after 2.5 s without pointer or key
activity while playing and always show while paused or with a menu open; `prefers-reduced-motion` makes every fade
instant (checked by a test). `rvp-ui` unit tests check the keymap against the context menu (every shortcut has an entry),
click/drag/menu/keyboard behaviour and the layout; `rvp-host-headless/tests/app.rs` drives the whole `App` with scripted
input in virtual time; the golden is `tests/e2e/golden/player-paused.png` (a seek to 15.0 s makes the picture
deterministic; tolerance: mean error < 1.5, under 1% of pixels off by more than 8). Performance (Chromium headless on this
machine, 320x240 AV1 into a 1280x720 canvas, wasm SIMD build): about 20 ms per tick on average, picture composition 9 ms
per new frame, chrome 5 ms, upload 4 ms; AV1 decode in wasm is the bulk of the session time. Playback keeps real time and
drops a few frames while starting; real-time HD is M9. Screenshots: `docs/screenshots/` (regenerate with
`cargo xtask e2e --screenshots`).

**M6 H.264 (our own decoder), staged.** Each stage has its own conformance gate against ffmpeg `framemd5`
(and the optional `rusty_h264-decoder` oracle); decoded pictures must be **bit-exact**.
- **6a** NAL/SPS/PPS/slice-header parsing, bit reader, Exp-Golomb, CAVLC, intra 4x4/16x16/PCM, integer
  transforms, deblocking, I-frame-only streams. *Done when:* the I-only fixtures (CIF and 720p) are bit-exact and decode
  720p in < 40 ms/frame native release.
- **6b** P slices: inter prediction, multiple reference frames, list init + reordering, MMCO and sliding window,
  quarter-pel luma/chroma interpolation, long-term refs. *Done when:* x264 `--profile baseline --bframes 0 --ref 4`
  fixtures are bit-exact.
- **6c** B slices: direct spatial/temporal, weighted prediction (explicit + implicit), DPB with output ordering
  (POC types 0/1/2). *Done when:* x264 `--profile main --no-cabac --bframes 3 --weightp 2` fixtures are bit-exact with
  correct display order and pts.
- **6d** CABAC: context init tables, binarization, all syntax elements, slice data. *Done when:* x264 default
  `--profile main` fixtures (CABAC) are bit-exact, including 1080p.
- **6e** High profile: 8x8 transform, 8x8 intra prediction, scaling matrices. *Done when:* x264 `--profile high`
  fixtures (8x8dct, scaling lists) are bit-exact; then wired into the hosts (MP4/MKV H.264+AAC/FLAC plays in the
  browser, Playwright test with an H.264 fixture).
- 6f (post-v1) interlaced PAFF/MBAFF, 4:2:2/4:4:4, high bit depth; FMO/ASO never.

*M6 notes (done 2026-10-05, all stages bit-exact against ffmpeg).* Clean-room: written from ITU-T H.264 (03/2010);
the VLC, CABAC-init, deblocking and scan/default-matrix tables are generated from the spec's text by
`tools/gen-h264-tables.py` (the spec PDF is not committed); no FFmpeg/openh264/VLC/JM source was read, ffmpeg and
ffprobe are binary oracles only, and `rusty_h264-decoder` was not needed.

*Conformance.* Fixtures come from `tools/gen-fixtures.sh` (50 x264 MP4s in `target/fixtures/h264`, never committed):
Baseline/Main/High, CAVLC and CABAC, intra-only, P with 1/4/16 refs, B with spatial/temporal/auto direct, B-pyramid,
weightp and weightb, 8x8dct, JVT scaling matrices, multiple slices, constrained intra, deblock offsets, no-deblock,
odd (cropped) sizes, QP 5 to 48, 720p and 1080p. Every decoded plane of every frame equals `ffmpeg -f rawvideo`
(`crates/rvp-codec-h264/tests/stage_{a..e}.rs`, plus an end-to-end test through the player in
`rvp-host-headless/tests/video.rs` and two Playwright tests). x264 never emits many syntax features, so
`tests/synth.rs` builds streams with this crate's own writers (I_PCM pictures plus zero-motion P/B macroblocks, so
each output macroblock shows exactly which reference was used; random residual macroblocks with CAVLC) and compares
with ffmpeg: about 5000 random streams cover POC types 0/1/2, frame_num wrap, sliding window and MMCO 1-6, long-term
references, reference list modification with long-term entries, explicit and implicit weights, spatial and temporal
direct, up to 3 slices per picture with every deblocking mode, CABAC I_PCM/skip, I slices inside P pictures,
frame pictures of interlace-capable streams, and 4x4/8x8 transforms with SPS- and PPS-level scaling matrices (fall-back
rules A and B). Truncation, bit-flip, zero/0xFF-run, shuffle and random-NAL fuzzing (`tests/robust.rs`, also run in
release with overflow checks on) finds no panic. Two places where ffmpeg differs from the specification and the
generators therefore steer clear of: it identifies long-term pictures by array position, so with sparse
`LongTermFrameIdx` (>= number of long-term pictures) the deblocking filter treats distinct long-term pictures as one
(long-term indices stay below 2 in the tests); and for CAVLC it appears to use the coded block pattern rather than the
coefficients for bS 2 of 8x8-transform blocks that are coded but all zero (generators never produce those; encoders do
not either). This decoder follows the specification in both.

*Deviations and limits.* Progressive 8-bit 4:2:0 only. Rejected cleanly with `Unsupported` (tests in
`tests/robust.rs`): field pictures and MBAFF (x264 `--interlaced`), High 10/4:2:2/4:4:4/monochrome, lossless
(transform bypass), FMO/ASO/slice groups, data partitioning, SP/SI slices. Frame pictures of a stream with
`frame_mbs_only_flag` 0 (no MBAFF) decode as progressive. Redundant slices are ignored. Lost data is concealed (missing
macroblocks copy the newest reference, missing references fall back to an existing or grey picture, `frame_num` gaps
insert "non-existing" frames). A new picture is detected by 7.4.1.2.4 or by a slice that restarts at macroblock 0.
Output order uses the DPB size and `num_reorder_frames` from the VUI (level-derived DPB size without one, so streams
without a VUI show up to a DPB's worth of frames late). Pictures above 36 864 macroblocks (4096x2304) are refused
(`Decoder::set_max_mbs`).

*Performance (this machine, release, single thread, one frame at a time; `cargo run --release -p rvp-codec-h264 --example bench -- file.mp4`).* Native and wasm
(`cargo xtask wasm-smoke`; `node tools/wasm-smoke.mjs <module> <file> --bench N`, Node 22 / V8, plain wasm32 release
build; a +simd128 build is within 5%) decode output identical to each other:

| Stream | Bitrate | Native | Wasm (Node) |
| --- | --- | --- | --- |
| 720p High, CABAC, B-frames, typical content (`h_high_720p_typ`) | 3.4 Mbit/s | 4.5 ms/frame, 220 fps | 8.0 ms/frame, 125 fps |
| 1080p High, typical (`h_high_1080p_typ`) | 4.7 Mbit/s | 9.7 ms/frame, 103 fps | 17.6 ms/frame, 57 fps |
| 720p High, stress (noise added, `h_high_720p`) | 28 Mbit/s | 21 ms/frame, 47 fps | 29 ms/frame, 35 fps |
| 1080p High, stress (`h_high_1080p`) | 25 Mbit/s | 31 ms/frame, 32 fps | 44 ms/frame, 23 fps |
| 720p Baseline P, CAVLC (`p_base_720p`) | 3.4 Mbit/s | 9.0 ms/frame, 111 fps | 14 ms/frame, 70 fps |
| 720p Baseline intra-only, CAVLC (`i_base_720p`) | 11 Mbit/s | 14 ms/frame, 70 fps | 20 ms/frame, 49 fps |

720p30 High therefore decodes faster than real time on native release, and in wasm too, even for the 28 Mbit/s
stress stream. Optimisation so far is only restructuring (slice-based interpolation kernels, uniform-motion shortcuts in
the deblocking filter, table masking in the CABAC engine); no SIMD or threads. The stress streams are dominated by CABAC
(about 25% in `decision`), typical ones by prediction and deblocking. 1080p stress in a browser tab sharing a thread
with the UI is the case M9 still has to win.

**M7 VP9.** *(done 2026-10-05; see the notes below)* Benchmark `rusty_vp9` and `vp9dec` on the VP9 fixtures against: licence, correctness
(ffmpeg `framemd5`), speed, wasm32 build, `no_std` support and memory; wrap the winner as `rvp-codec-vp9` behind
`CodecFactory`. Fixtures: profile 0, 8-bit, several sizes and encoder settings, odd sizes, and resolution change
mid-stream if the crate supports it. If neither is acceptable, report why and propose options before porting our
own (stage like M6). *Done when:* VP9 (profile 0, 8-bit) fixtures decode bit-exact to `framemd5`, 1080p30 decodes
at >= 30 fps native release single thread, a headless test plays a VP9+Opus WebM end to end, and the browser E2E
passes with it.

*M7 notes (done 2026-10-05).* **Choice: `rusty_vp9` 0.1.1.** Both candidates were benchmarked in a scratch crate (native, one pinned core, AVX2 paths on; wasm32 in Node/V8, plain build) on the libvpx-made fixtures
(`tools/gen-fixtures.sh`, set `vp9`, never committed) against ffmpeg's `-f rawvideo` output (equivalent to `framemd5`, stricter on message):

| | `rusty_vp9` | `vp9dec` |
| --- | --- | --- |
| Licence | Apache-2.0 (compatible; one of our two options) | MIT |
| Correctness | bit-exact on every fixture | bit-exact on every fixture |
| Native 1080p typical, ms/frame | 8.6 | 34.5 |
| Native 720p typical, ms/frame | 3.5 | 15.7 |
| Native 720p stress (noise), ms/frame | 18.7 | 39.1 |
| wasm 1080p typical, ms/frame (Node) | 33.6 | 121.6 |
| wasm 720p stress, ms/frame (Node) | 30.4 | 75.8 |
| wasm32 build | yes, same output hashes as native | yes, same hashes (its threads fall back to one) |
| `no_std` | no | no |
| Size | about 31k lines incl. an encoder we do not use | about 14k lines |
| Malformed input | `catch_unwind` safety net inside (useless on wasm, where panics abort) | returns errors |

Both crates claim and (on our streams) show conformance with the 315 libvpx vectors; `rusty_vp9` won on speed alone, by 3 to 4 times. Through our wrapper (`rvp-codec-vp9`), native single thread: 1080p typical 7.1 ms/frame (140 fps), 720p
typical 2.9 ms, 720p stress 17 ms; wasm (Node, including demux): 1080p typical 34.6 ms/frame (29 fps), 720p typical 14.4 ms, so
1080p30 VP9 in wasm is at the edge and is an M9 (SIMD128) target.
Fixtures: sizes 8x8 to 1080p, odd sizes (327x245, 130x66, 17x9), good/best/realtime/CBR, hidden alt-ref frames (superframes), tiles (columns and rows),
error-resilient, frame-parallel, cyclic/variance/complexity AQ (segmentation), lossless, q 4 and 63, sharpness, static threshold, screen content, all-intra, BT.709/BT.2020 and full-range tags, profile 2
(10-bit 4:2:0, decoded to `Yuv420p10`), profile 1 (4:4:4, rejected with `Unsupported`), VP9+Opus and VP9+Vorbis, and a resolution change at key frames (320x240, 480x270,
200x120). Reference scaling inside a stream (inter frames of a new size) cannot be produced by ffmpeg's libvpx wrapper and is not covered by our fixtures; `rusty_vp9` claims it through the libvpx `resize_*` vectors.
Wrapper behaviour: profiles 1 and 3, 4:2:2/4:4:0/4:4:4 and 12-bit are rejected; the decoder is reset after any error and ignores inter frames until the next key frame; pictures above 8192x4352 are refused; the colour matrix and range
come from the key frame header (peeked by the wrapper, as `rusty_vp9` drops the range bit). Fuzzing (`tests/robust.rs`: truncation, bit flips, header flips, garbage, dropped and swapped packets, mid-stream starts; 9000 streams run once) never made `rusty_vp9` panic, so the abort-on-panic wasm behaviour is not a practical
problem so far; the test asserts zero contained panics. Tests: `crates/rvp-codec-vp9/tests/{conformance,robust}.rs`, `crates/rvp-host-headless/tests/vp9.rs` (bit-exact frames through the session, A/V sync, seek, resize, damaged file), two Playwright tests
(VP9 with Opus and with Vorbis, plus a resize test that watches the picture size change), and `cargo xtask wasm-smoke` (native and wasm hashes agree). `window.rvp.snapshot()` gained `frame_size`.
Bench: `cargo run --release -p rvp-codec-vp9 --example vp9_bench -- file.webm`. Done criteria met (bit-exact profile 0, 1080p30 at 140 fps native, browser E2E).

**M8 Playlist, seek, subtitles, gapless, now-playing, visualizer tap.** *(done 2026-10-05; see the notes below)* `Playlist` (add/remove/reorder, repeat,
shuffle, drag to add), resume positions in `Storage`, accurate seek (decode forward to the exact frame), frame
step, speed 0.25x-4x with WSOLA audio, track selection (audio/subtitle), A-B loop, chapters (MKV/MP4), `rvp-subs`
(SRT, WebVTT, MKV `S_TEXT/UTF8` and `S_TEXT/WEBVTT`) with rendering in `rvp-ui`. Additions:
- **Gapless playback:** the next playlist item is opened and its decoder primed while the current one plays; the
  audio pipeline joins the two streams sample-accurately (container/encoder delay and padding trimmed using
  MP4 edit lists, Matroska `CodecDelay`, LAME/Xing gapless tags where present), with the sample rate converted
  once at the join and no clock discontinuity. Crossfade is out of scope.
- **Now-playing model** in `rvp-host` (`NowPlaying`, `NowPlayingMeta`, `TransportCommand`; section 4): title, artist,
  album, art, position, duration, state, plus transport commands, MPRIS-like and host-neutral. The player updates
  it from container metadata (and, from M10, library tags). In the browser `rvp-host-web` wires it to the **Media
  Session API** (metadata, playback state, position state, action handlers for play, pause, seek, previous/next).
- **Visualizer tap** (`VisualizerTap`, `rvp-viz`): the audio pipeline hands every output block (interleaved f32
  PCM plus stream time) to a tap in `rvp-host`, and core computes an FFT/beat summary (log-spaced bands, level,
  onset flag, tempo estimate) that rides along. The browser host exposes both to JS for tests.
*Done when:* unit tests parse a corpus of SRT/WebVTT edge cases (BOM, CRLF, overlapping cues, styling tags
stripped, cue settings ignored) into the expected cue list; headless play with a subtitle file emits `SubtitleCue`
events at the right virtual times (+-1 frame); exact-seek test shows frame N's decoded hash equals the hash from
sequential decode for 50 random N; a playlist E2E plays 3 items back to back with no gap > 100 ms and resumes the
saved position after reload; **gapless:** a headless test of consecutive FLAC (or Opus, MP3 with gapless tags)
tracks cut from one continuous sine is bit-continuous across the join (no gap, no extra samples; for lossy
codecs the join error stays under -60 dBFS RMS) and reports a gap of 0 frames; **now-playing:** a headless test
sees metadata, state and position updates and applies each `TransportCommand`, and a Playwright test checks
`navigator.mediaSession.metadata`/`playbackState` and that a Media Session action pauses playback; **visualizer:**
a 1 kHz sine puts its energy in the expected band and an impulse train at 120 bpm gives onsets at 500 ms +- 1 block
and a tempo estimate within 2 bpm.

*M8 notes (done 2026-10-05, in four commits: tracks/seek, playlist/gapless, then now-playing, tap and chapters).*

- **Subtitles.** `rvp-subs` parses SRT and WebVTT (BOM, CRLF, index lines, overlapping cues, `NOTE`/`STYLE`/`REGION` blocks,
  cue ids and settings ignored, `<i>`, `<c.x>`, `<v>`, timestamps tags and `{\an8}` stripped, entities decoded) and the packet
  payloads of Matroska `S_TEXT/UTF8` and `S_TEXT/WEBVTT` (also the older `D_WEBVTT/*` ids) and MP4 `tx3g` (`mov_text`) and `wvtt`.
  The demux task decodes every subtitle packet it passes into a per-track `CueList`, so switching tracks needs no re-read; cues that
  were skipped over by a forward seek are not recovered (a cue that began before the landing cluster is missed). Sidecar `.srt`/`.vtt`
  files (dropped or picked together with the video, or on their own onto a playing one) load through a session task and are selected
  at once. `SessionEvent::Subtitle` fires with the stream position when the text changes (tests: within one frame of the cue time).
  The UI draws wrapped lines on dark pills above the bar. ASS/SSA is listed as unsupported; the bundled font is a Latin subset.
- **Audio tracks.** `Session::select_audio` rebuilds the audio decoder, reroutes the demuxer and re-seeks to the current position
  (a short rebuffer). Labels use the language tag (`English`, `Spanish`, ...). `A` and `S` cycle, the Tracks button and the menu pick.
- **Exact seek, frame step, A-B loop.** Seeking already decoded forward from the keyframe and showed the frame at the target; M8 adds
  the test (50 random targets on a B-frame GOP-12 stream are bit-identical to the sequential decode, forward and backward). `.` and `,`
  step one frame (backward is an exact seek to just before the frame on screen); resuming after a step re-seeks so audio restarts at the
  picture. `I` marks A, then B (the loop runs), then clears; menu entries set each mark. Playback past B seeks back to A.
- **Speed.** `rvp-core::stretch::TimeStretcher` is a streaming WSOLA (40 ms Hann windows, 50% overlap, cross-correlation search of
  plus or minus 10 ms on a mono mix, coarse then fine). 0.25x to 4x keep the pitch (tests: 440 Hz stays within 8 Hz from 0.5x to 3x). It
  replaces the varispeed resampler; cost is about 0.1 to 0.3% of a core (`cargo run --release -p rvp-viz --example viz_bench`).
- **Playlist and next/previous.** `Playlist` keeps ids, order, current, repeat (off/all/one) and shuffle (the current item stays first;
  adds join the remaining order). `N`/`P`, mouse back/forward, `R` repeat, `Z` shuffle, `Q` the playlist menu (up to 12 rows around the
  current item, plus add, move, remove, clear); `P` restarts an item that has played more than 3 s. Several files picked or dropped
  become the playlist; Shift+drop and "Add files" append. Items are host ids resolved with `host.open(Id)` (the web host now keeps dropped
  files, so Previous and Repeat can reopen them). Unopenable files are skipped with a toast.
- **Resume.** The position of the current file is stored under `resume:<name>` every 5 s, when pausing or changing item, and on
  `pagehide`; opening the same name again seeks there (only past 5 s and with 10 s left; finishing a file forgets it).
- **Gapless.** `Session::queue_next` opens the next item in the background; the app queues it when under 12 s remain or the demuxer has
  finished. When the current item has handed over all its audio, the feed moves to the next one **without a flush**: `AudioOut` keeps
  timeline segments (a segment per item, seek or timestamp jump) so `heard_pts` and the item being heard stay exact, and the next item
  becomes current when its first sample is heard (`ItemStarted`). The end padding is cut: Opus by Matroska `DiscardPadding`
  (`Packet::discard_end_us`), AAC and others by the MP4 edit list (the stream duration is now the edit list's playable length) applied as
  an end limit on the audio track. Tests: FLAC pieces cut from one sine reproduce it bit for bit (288000 frames, no gap, nothing doubled);
  Opus pieces match within -64 dBFS (exact frame count), AAC within -50 dBFS (its own coding noise); no click at the joins; also through
  the application with titles changing as each item is heard. A next item with no audio, or one that is not ready in time, starts
  right after the current one ends (not gapless). Crossfade is out of scope.
- **Now-playing.** `rvp-host::media` (`NowPlaying`, `NowPlayingMeta`, `Playback`, `TransportCommand`) and `docs/host-api.md`. Tags and cover
  art come from the containers: MP4 `ilst` (title, artist, album artist, album, `covr`), Matroska `Info/Title`, `Tags` and cover attachments.
  The app sends metadata when the item or its tags change and playback only on a change or a jump of more than 0.5 s (the host
  extrapolates); commands from the sink drive the player. The browser maps it to the **Media Session API** (`web/mediasession.js`): metadata with artwork
  from a Blob, `playbackState`, `setPositionState`, handlers for play, pause, stop, seekto, seekforward/backward, next and previous.
- **Visualizer tap.** `rvp-viz::Analyzer` (hop 512 frames: 2048-point Hann spectrum folded into 32 log bands, level, a 512-point spectral
  flux (two windows per hop so a hit is seen in the middle of one) with a median/MAD threshold for onsets, tempo from the autocorrelation
  of the flux with a half-lag check). `AudioOut` keeps a copy of what the sink accepted and hands over what has been **heard**
  (latency removed). Tests: sine in its band; clicks at 90, 120 and 150 bpm give exactly one onset per click within one hop and the
  tempo within 2 bpm, through the whole pipeline as well; the browser exposes the tap's counters and the latest summary as
  `window.rvp.vizState()`.
- **Chapters** (not on the original M8 list, but in the plan): Matroska `Chapters` and MP4 `chpl`; Page Down/Page Up, a menu with the
  chapters near the current one, marks on the seek bar and the chapter title in the hover time.
- **Fixtures** (`tools/gen-fixtures.sh`, set `m8`, never committed, regenerated when its version number changes): sidecar and embedded
  subtitles, two audio tracks, gapless pieces in FLAC, Opus and AAC, chapters, tagged audio with cover art, 120 bpm clicks. Tests:
  `crates/rvp-subs`, `rvp-core::stretch`, `rvp-viz`, `rvp-player::{playlist,audio}` unit tests; `rvp-demux/tests/m8_meta.rs`;
  `rvp-host-headless/tests/m8_{tracks,seek,gapless,playlist,nowplaying,viz}.rs`; `tests/e2e/m8.spec.js` (subtitles, a dropped .srt,
  track switching, frame step and loop, a three-file playlist with N/P and the menu, resume after a reload, Media Session, the visualizer tap,
  chapters).

**M9 Performance and robustness.** *(done 2026-10-05; see the notes below)* wasm SIMD128 kernels for YUV->RGBA, deblocking, inter
prediction, loop filters; optional worker pool (`Parallel` capability, threads build); fuzzing the demuxers and bitstream parsers
(`cargo fuzz`, no panics, no OOM on 1 GB claims); corrupt/truncated stream handling; memory caps; raw audio demuxers.
*Done when:* 1080p30 H.264 High and VP9 play at 1x in headless **wasm** (run with Node or the Playwright browser) with < 2% dropped
frames over 60 s; fuzz targets run 10 min each with no findings; a truncated-file test plays to the end of the available data and
emits `Ended`/`Error` instead of panicking.

- **Result against "done when".** Headless Chromium (16 threads, 1280x720 page, 60 s at 1x): H.264 typical, VP9, AV1 and H.264 at 25 Mbit/s
  **0.00% dropped** with the threads build; with the single-threaded build H.264 typical and VP9 0.00%. Nine fuzz targets ran 10
  minutes each without a finding (after the findings below were fixed; `demux`, which covers the newest code, was run again after each
  finding, the last time for 15 minutes). `hardening.rs` truncates and corrupts every fixture and checks that the session ends with `Ended` or an error.
- **Browser speed, before and after** (`cargo xtask perf-fixtures`, then `cargo xtask perf-web --both`; 1080p30, one minute each; the
  "before" column is the build at the start of M9, single-threaded, 30 s runs; this machine runs the `powersave` governor, so
  absolute numbers move by 10-20% with load: compare rows, not runs):

  | Stream (1080p30, 60 s) | Before: dropped | Single-threaded + SIMD128: dropped | Threads build (8 workers): dropped | Session ms per tick, single / threads |
  | --- | --- | --- | --- | --- |
  | H.264 High, typical (about 6 Mbit/s) | 15.2% | 0.00% | 0.00% | 6.8 / 2.0 |
  | H.264 High, 25 Mbit/s stress | 42.3% | 4.96% | 0.00% | 29.9 / 1.9 |
  | VP9 (rusty_vp9) | 27.8% | 0.00% | 0.00% | 5.9 / 1.8 |
  | AV1 (rav1d) | 49.7% | 9.57% | 0.00% | 10.0 / 2.2 |

  The single-threaded build keeps up with typical H.264 and VP9; the 25 Mbit/s stream and AV1 need the threads build on this CPU.
  "Session ms per tick" is the average time of the player's tick on the UI thread (the decoders run on workers in the threads build).
- **Native, same streams** (`taskset -c 3` for one core; `cargo run --release -p rvp-codec-h264 --example bench`, `rvp-par --example
  h264_bench`, `rvp-codec-vp9 --example vp9_bench`; `RVP_POOL` sets the pool size):

  | Stream | One thread, scalar decoder | Pipelined: 1 / 2 / 4 parse threads |
  | --- | --- | --- |
  | H.264 typical | 10.2 ms/frame (98 fps) | 7.7 / 4.5 / 3.7 ms (131 / 223 / 272 fps) |
  | H.264 25 Mbit/s | 29.4 ms/frame (34 fps) | 20.3 / 12.6 / 11.2 ms (49 / 80 / 89 fps) |
  | VP9 | 7.2 ms/frame (139 fps) | n/a (one decoder thread) |

  The pipelined decoder is bit-exact with the inline one (same checksum; the ffmpeg-oracle conformance, synthetic and robustness tests
  run on both).
- **How it was found: profile first** (`perf record` natively, the decoder's `stages` timing in `rvp-codec-h264 --example bench`, and the tick breakdown in
  `window.rvp.snapshot().perf` in the browser). The work followed what showed up: colour conversion and scaling, H.264 inter
  prediction and deblocking, CABAC and residual parsing, then threads for what was left.
- **SIMD128** (`core::arch::wasm32`, behind `cfg(target_feature = "simd128")`; every kernel has a scalar twin that defines the result
  and a `selftest` that compares them on random data *inside WebAssembly*, run in Node by `cargo xtask wasm-smoke`
  (`tools/wasm-selftest.mjs`); the same fixtures decode to the same hashes natively, in scalar wasm and in SIMD wasm): YUV420 to RGBA
  (`rvp-core::color`, banded on the pool), the picture scaler (`rvp-ui::gfx`, vertical-first resampler), H.264 luma six-tap and chroma
  bilinear prediction, averaging and weighted bi-prediction, luma and chroma deblocking (normal and strong filters; vertical edges
  by 8x8 transposes), a branch-free CABAC decision, and in the vendored `rusty_vp9` (`third_party/rusty_vp9`, `PATCHES.md`) the
  eight-tap motion compensation and the loop-filter edge kernel. The 4x4 and 8x8 inverse transforms stayed scalar with the existing
  DC-only shortcut: they were not a hot spot. The page needs SIMD128 anyway (every current browser has it); the plain wasm build
  exists for the smoke test.
- **Threads** (opt-in: `cargo xtask web --threads`, nightly with `rust-src`, `-Z build-std`, atomics, bulk memory, shared memory up to
  2 GiB; `serve` sends COOP/COEP): `rvp-par` pool (lazy start, spin locks, `mark_ui_thread`), `ThreadedVideoDecoder` (the decoder
  lives on a worker, results come back in an epoch-tagged outbox so a seek discards stale frames; `VideoDecoder::pending()` lets the
  session count frames in flight), the H.264 decoder split into a **parsing side** (pictures parsed in parallel on workers; B-slice
  direct prediction waits for the co-located picture's motion data) and a **reconstruction side** (an event stream consumed in order;
  inter macroblocks in row bands on the pool, then intra and PCM macroblocks in order, then deblocking one plane per job), AV1 through
  rav1d's own threads (`n_threads` from the pool, a spawn hook for Web Workers, `PATCHES.md` items 5 and 6), colour conversion and
  scaling in bands. With no executor the H.264 decoder runs inline and is bit-exact with the threaded path. VP9 stays on one decoder
  thread (rusty_vp9 has no frame or tile threading). The single-threaded build stays the baseline and is what `cargo xtask e2e` runs
  first; `cargo xtask e2e --threads` runs the whole suite a second time on the threads build.
- **Crash recovery and `catch_unwind` on wasm** (`panic = abort` there, and `rusty_vp9` relied on `catch_unwind` inside its decoder):
  a panic cannot be caught in wasm, so the answer is to make one unlikely and to survive it. Unlikely: the VP9 fuzz target found no
  panic in about 40k mutated streams, and frame sizes are capped (`MAX_PIXELS`, 8192x4352) before the decoder sees them. Survive: a
  panic on the main thread or on a worker (reported to the page by the worker's rejection handler) makes the page **discard the whole
  wasm instance** (re-import of the glue, fresh shared memory, workers terminated), build a new player, reopen the files and resume
  at the saved position. Tests: `tests/e2e/player.spec.js` crash recovery (a panic on the main thread and one in
  the decoder thread, both builds) via `debug_panic` and `debug_crash_decoder`.
- **Fuzzing** (`fuzz/`, cargo-fuzz with libFuzzer and AddressSanitizer on nightly; not part of the workspace; `cargo xtask fuzz
  [target|all] [secs]`, `fuzz/run.sh`; seeds are committed in `fuzz/corpus/<target>` (under 200 KB in all), new corpus goes to the
  untracked `fuzz/work`). Third-party codec crates are built without overflow checks, as in release builds (profile `rustflags`,
  because cargo-fuzz forces `-Cdebug-assertions`); our own crates keep every check.

  | Target | Input | Runs in 10 min | Findings, all fixed with a regression input or test |
  | --- | --- | --- | --- |
  | `demux` | any bytes through `open`, every packet, seeks | 0.9M (the newest run 2.7M in 15 min) | MP4 data position overflow, a slow Matroska scan (timeout), FLAC seek-table offset overflow, Ogg granule overflow |
  | `h264`, `h264_pipelined` | AVCC stream, inline and threaded | 92k, 33k | negative slice QP; threaded and inline disagreeing after a slice error (`parse_error`) |
  | `vp9` | frames and superframes | 10k | none |
  | `av1` | packets into rav1d | 16k | zero-length data aborted the process (`send_packet` now ignores empty packets); panic on a frame without headers; a leak of one `Arc` per picture (rav1d patches 7 and 8) |
  | `audio` | codec config and packets for AAC, MP3, FLAC, Vorbis, Opus, PCM | 130-180k | none in the decoders (an overflow in symphonia's FLAC predictor appeared only because cargo-fuzz forces overflow checks: it wraps in release) |
  | `subs` | SRT, WebVTT, MKV and MP4 subtitle payloads | 1.4M | unbounded cue counts and timestamps (caps below) |
  | `playlist`, `playlist_files` | M3U, M3U8, PLS text | 0.5M, 1.6M | none |

  Regression inputs live next to the code (`crates/*/tests/fuzz_regressions/*.bin` and `fuzz_regressions.rs`).
- **Memory caps and limits.** MP4: 4M samples (`MAX_SAMPLES`); Matroska: 1M clusters indexed, 10k chapters, tag and chapter depth 8, 8 MiB of
  header metadata; one read allocation at most 256 MiB; subtitles: 200k cues, 4096 bytes of text per cue, 8 MiB per subtitle file;
  playlists: 100k entries of at most 4096 bytes; tags: art at most 16 MiB, ID3v2 tag at most 32 MiB; video: 8192x4352 pixels for AV1 and VP9
  (`frame_size_limit`, `MAX_PIXELS`) and 36864 macroblocks for H.264 (configurable); the session holds at most 96 MiB of queued
  packets. Positions and sizes use saturating and forward-only arithmetic so a hostile offset cannot loop or wrap.
- **Truncated, growing and corrupt files.** Demuxers re-ask `Source::size()` when a read reaches the known end, so a file that is still
  being written (a recording, a download) plays on as it grows (`tests/growing.rs`: a prefix, then more, gives exactly the packets of the
  whole file). The session treats `Error::Truncated` as "wait": it retries every 250 ms for 1.5 s, then plays out what it has and ends
  (`Ended`, with a warning). A packet that fails to decode is dropped and playback goes on. Fixed on the way: the audio clock stopped
  advancing once the audio had been handed over, so a longer video would hang at the end of a shorter audio track.
- **Raw audio demuxing** (done here rather than in M10; own code, `no_std`, in `rvp-demux`; `AnyDemuxer` sniffs the file, looking past
  ID3 tags). **MP3**: ID3v2.2/2.3/2.4 (all four text encodings, unsynchronisation, extended headers, `APIC` front cover preferred),
  ID3v1, Xing/Info/VBRI and the LAME extension (encoder delay and padding become a negative first timestamp, `-(delay + 529)` samples,
  and `discard_end_us` on the last frames, so a gapless MP3 is sample-exact: -133 dBFS against ffmpeg); frames are found by sync and chain
  checking, and a full walk of files up to 24 MiB gives an exact duration and a seek index. **FLAC**: STREAMINFO, seek table, Vorbis
  comments, pictures; frames are delimited by CRC-16 plus a valid next header (CRC-8); seeks use the table or a byte bisection.
  **Ogg**: Vorbis (block sizes read from the mode table at the end of the setup header give packet durations), Opus (TOC durations,
  pre-skip, 80 ms pre-roll after a seek) and FLAC-in-Ogg; other multiplexed streams are skipped, a chained stream ends the file; seeks
  bisect on page granule positions. **WAV**: RIFF and RF64, PCM 8/16/24/32 and float 32/64 (also `WAVE_FORMAT_EXTENSIBLE`), `LIST/INFO` and
  `id3 ` tags; a truncated or open-ended data chunk is clamped to the file. **ADTS AAC**: header to AudioSpecificConfig, exact duration
  by a walk. `rvp-codec-audio` has a PCM decoder (mono and stereo) for WAV. `Metadata` gained album artist, track and disc numbers and
  totals, year and genre. Tests (`rvp-demux/tests/audio.rs`, set `audio` of `tools/gen-fixtures.sh`): stream parameters, durations, tags,
  cover art and **every packet's size and time against `ffprobe -show_packets`** for 14 files; ID3v1; gapless trimming; seeks on all
  formats; truncated and byte-flipped files; `rvp-host-headless/tests/m9_raw_audio.rs` plays each file through the whole pipeline and
  compares the output with ffmpeg's decode (lossless formats and WAV exact, lossy within -70 to -147 dBFS, lengths within a frame);
  `tests/e2e/m9.spec.js` plays nine of them in the browser. Not done: Layer I/II MPEG audio, APE and AIFF, ReplayGain, multi-channel
  PCM (the output path is mono or stereo, like the other decoders), chained Ogg playback past the first stream.
- **Deviations from the brief.** wasm IDCT SIMD was skipped (not a hot spot); no frame threading for VP9; the "before" numbers come from a
  30 s baseline run, not 60 s; the Node-wasm timing numbers are not tabulated (Node is only used for the bit-exactness smoke tests and
  kernel self-tests, the browser is the speed oracle).

**M10 The audio-first view.** Makes RVP a music player as well as a video player; all of it lives in host-neutral
crates and `rvp-ui`, so every edition gets it.
- **Library:** scan and index (`rvp-library`) a folder or a set of files handed over through the optional `Library`
  host capability (desktop: directory walk; browser: File System Access directory handle with an
  `<input webkitdirectory>` fallback; Rusty Bucket: its fs); incremental rescans keyed by path, size and mtime;
  tags (title, artist, album artist, album, track/disc, year, genre, duration) and **cover art** from `rvp-demux`
  (its own readers since M9: ID3v2, Vorbis comments, MP4 `ilst`, FLAC pictures) with a folder-art fallback (`cover.jpg`/`folder.png`);
  the index and a downscaled art cache persist through `Storage`. Art decoding is JPEG/PNG; a small decoder crate
  is chosen in this milestone (MIT/Apache only).
- **Playlists:** M3U, M3U8 (UTF-8, `#EXTINF`) and PLS import and export; relative paths resolved against the
  playlist's location; missing entries are kept and flagged, never silently dropped.
- **Queue:** play next, add to queue, reorder, remove, clear, repeat, shuffle (no immediate repeats), save queue
  as a playlist; the queue is the M8 `Playlist` seen through the library.
- **Views:** Albums (art grid), Artists, Tracks (sortable list), Playlists, Queue, search-as-you-type, all
  keyboard and pointer driven, with the now-playing screen from section 9.
- **Gapless album playback:** playing an album or a queue uses the M8 gapless path end to end (including the
  next-track prefetch across files in a folder and across containers).
- **Visualizer view** (Unicorn Viz spirit, `../unicorn-viz`, MIT, reference only): a full-window view with at
  least four effects (spectrum bars, oscilloscope, tunnel/plasma, beat-reactive particles), a preset switcher,
  an optional overlay (title, artist, time), driven by the M8 `VisualizerTap` summary and drawn in `rvp-viz`
  into the RGBA surface so it looks the same everywhere; effects run within the tick budget at 1080p in wasm.
*Done when:* a generated library (about 200 tracks across 20 albums, mixed MP3/FLAC/Opus/AAC/Vorbis, with and
without embedded art, odd tags, Unicode, missing tags) scans in a headless test into the exact expected
artist/album/track tree with art for the right albums, and a rescan after adding, changing and deleting files
updates only what changed; M3U, M3U8 and PLS fixtures round-trip (import, export, import gives an equal list) and
relative, absolute, missing and BOM/CRLF cases are covered; the queue operations pass unit tests; a headless
album playback test shows gapless joins (section M8 criterion) for the whole album and the right now-playing
metadata per track; a Playwright test scans a folder in the browser, opens an album and verifies view contents,
now-playing metadata, and that the visualizer canvas pixels move with the music and stay still when paused
(golden screenshots with a tolerance for the static views).

**M11 The standalone desktop app and packaging.** The "standalone" edition (a):
- **Native host** `rvp-host-desktop` (binary `rvp`): `winit` window (Wayland and X11), `softbuffer` presenting the
  RGBA surface (`pixels` only if needed), `cpal` audio with the device clock feeding `queued_frames` and
  `output_latency_us`, real files and folders (command line, `rfd` open dialogs, drag-drop of files and folders,
  "open with"), settings and library database under the XDG directories, fullscreen, window state, HiDPI.
- **MPRIS on Linux** (`org.mpris.MediaPlayer2`, via `souvlaki` or `mpris-server`) implementing `NowPlaying`:
  metadata, art, position, transport commands, media keys; checked with `playerctl`.
- **PWA:** web app manifest (name, icons from the theme, `display: standalone`, file handlers and a share/open target
  where the browser supports them), a service worker that precaches the wasm, JS and fonts so the app starts
  offline (versioned cache, update flow), install prompt handling, and the Media Session wiring from M8.
- **Packaging:** a Flatpak (manifest in `packaging/flatpak`, runtime `org.freedesktop.Platform`, permissions: Wayland,
  fallback X11, PulseAudio/PipeWire, `--filesystem=xdg-music:ro`, `xdg-videos:ro`, MPRIS own-name; portals for the
  rest) and an AppImage (`packaging/appimage`, built by `cargo xtask appimage`), plus a `.desktop` file, AppStream
  metainfo and icons. Release builds are static where possible and `cargo xtask licenses` lists every bundled
  component.
- **Licence check (done 2026-10-05, in section 8):** `winit` Apache-2.0, `softbuffer` MIT OR Apache-2.0, `cpal`
  Apache-2.0, `rfd` MIT, `souvlaki` MIT (`mpris-server` MPL-2.0 as the alternative), `directories` MIT OR Apache-2.0,
  all compatible with MIT OR Apache-2.0; the packages' bundled system libraries are audited per package before a
  release and recorded in `THIRD_PARTY_LICENSES.md`. No GPL/LGPL code is linked statically into the binary.
*Done when:* `cargo xtask desktop` builds `rvp`; a headless-display test (Xvfb or a Wayland compositor in CI, plus
the virtual-time host for logic) opens a fixture from the command line, shows decoded frames (screenshot or
surface hash) and plays audio through a null/PipeWire sink with the audio clock advancing at 1x +- 5%;
`playerctl` can read metadata and position and pause and seek it; `cargo xtask flatpak` and `cargo xtask appimage`
produce packages that start and play a fixture in a clean environment (`flatpak run`, the AppImage on a bare
container); a Playwright test installs the PWA (manifest valid, service worker active), reloads **offline** and
still plays a locally opened fixture; `THIRD_PARTY_LICENSES.md` lists everything bundled.

**M12 Rusty Bucket adapter** (was M10). `rvp-host-rb` against the app ABI (Canvas, input, timers, fs, audio) once it
exists, mapping `rvp-host` (playback, `NowPlaying`, `VisualizerTap`, `Library`) to the App API whose media
interfaces rust-os models on ours (ADR-0026); decide runtime vs codec service (wasmi is too slow; see section 5).
Nothing here may change core, `rvp-host`, `rvp-ui` or `rvp-app` for Rusty Bucket's sake. *Done when:* the player app
runs inside Rusty Bucket under QEMU, opens a video from the ramdisk/FAT image, and a headless QEMU screendump
shows decoded frames and the themed UI; **blocked** until `../rust-os` Phase 3 (host API v0) lands, plus
audio and file APIs.

## 12. Risks and open questions

| # | Risk | Plan |
| --- | --- | --- |
| R1 | `rav1d` 1.1.0 does not compile on `wasm32-unknown-unknown` (libc imports). | **Resolved in M4**: a private `libc` shim module (see `third_party/rav1d/PATCHES.md`) was the only change needed. The vendored copy decodes bit-exact in Node (`cargo xtask wasm-smoke`). Upstreaming the shim is still worthwhile. |
| R2 | Real-time 1080p in single-threaded wasm for H.264/AV1/VP9. | **Resolved in M9**: SIMD128 kernels (single-threaded build: H.264 typical and VP9 at 0% dropped, H.264 25 Mbit/s 5%, AV1 9.6%) and the opt-in threads build (0% dropped on all four, 60 s each). Numbers in the M9 notes. |
| R3 | Rusty Bucket's `wasmi` is an interpreter: video will not be realtime there. | M12; native codec service or a JIT/AOT runtime; tracked in `../rust-os/docs/planning/architecture.md` (D12 compile-ahead engine, D16 threads). |
| R4 | Bit-exact H.264 is long, detail-heavy work. | **Resolved in M6**: staged ffmpeg oracles, generated tables, synthetic streams for features x264 does not emit. |
| R5 | `opus-decoder` is a 0.1.x crate. | Test vectors in M3; fallback `ropus`. |
| R6 | Symphonia is MPL-2.0 and `std`. | Fine unmodified (file-level copyleft); audio crate is isolated, so it could be swapped for `nanomp3`/own decoders without touching the core. |
| R7 | Browser audio latency/clock accuracy differs by browser. | Measure `AudioContext.outputLatency/baseLatency`; fall back to monotonic master when unreliable; test on Chromium and Firefox. |
| R9 | Desktop and packaging dependencies (`winit`, `cpal`, `souvlaki`, bundled system libs) add native licences and a Linux-only surface. | All direct deps checked (section 8); the desktop host is its own crate so core stays `no_std`; package audit and `cargo xtask licenses` in M11; Flatpak/AppImage built in CI-like clean environments. |
| R10 | Gapless needs exact encoder delay/padding, which not every file carries (older MP3s without LAME tags). | Use container and tag data when present; otherwise trim only the decoder delay and report `gapless: approximate` in the now-playing metadata; tests use generated files with known cuts. |
| R11 | The browser cannot always read a whole music folder (File System Access is Chromium-only). | `<input webkitdirectory>` fallback; the library persists its index so a rescan is only needed after changes; document per-browser limits. |
| R8 | Fonts: OFL files must be bundled for `fontdue`. | M5: Space Grotesk and JetBrains Mono (OFL) subsets; licence text in `THIRD_PARTY_LICENSES.md`. |
| Q1 | Do we want a hardware-decode escape hatch (WebCodecs) in the browser host? | Decide after M4 numbers. It would be an optional `VideoDecoder` impl, never required. |
| Q2 | Keyboard seek step defaults (for example 5 s and 30 s with Shift). | **Decided in M5**: 5 s, 30 s with Shift, `J`/`L` 10 s. |
