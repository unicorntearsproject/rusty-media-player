# rusty-video-player: Plan

> Status: shipped as 1.0.0-rc9.1 (milestones M0 to M12 and the release-candidate rounds are done; the 1.0.0 final waits for tests on other
> systems). This file is the architecture and the history of how it got there: the sections up to the milestone list describe the design,
> the "rounds" at the end record what each release added. What comes next is in [v1.1-media-server.md](v1.1-media-server.md) and the
> README's Roadmap. Decisions live in [`CLAUDE.md`](../../CLAUDE.md). Back to the [documentation index](../README.md).

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

- **In scope (v1):** containers MP4, MKV, WebM; video H.264 and HEVC (both our own decoders), AV1, VP9; audio AAC, MP3, FLAC,
  Opus, Vorbis; seek, pause, speed, volume, playlist, SRT/WebVTT subtitles; our own UI.
- **In scope (after v1, M8 to M11):** gapless playback, optional crossfade and an automatic loudness level; a host-neutral now-playing model and a visualizer tap;
  an audio-first view (library, playlists, queue, album/artist/track views, built-in visualizer); a native
  desktop host and packaging (PWA, Flatpak, AppImage, MPRIS).
- **Out of scope:** optical discs, network protocols other than "the host hands us bytes", streaming
  adaptive formats (HLS/DASH), skins, filters/effects, DRM, interlaced H.264 (PAFF/MBAFF) until after v1,
  anything in VLC's `modules/` not listed above. (Transcoding was out of scope for v1; it is planned for 1.1 as a server feature.)
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
| `rvp-core` | `no_std + alloc` | Shared types: `Rational`, `Timestamp` (microseconds, `i64`), `StreamInfo`, `Packet`, `VideoFrame` (planar YUV, strides, colour info), `AudioBuffer`, error types, `MasterClock`, `RingBuffer`; the BS.1770 loudness meter (`loudness`), the gain stage, equal-power curve and true-peak limiter (`dynamics`), `AudioSettings` and their saved text (`settings`). No I/O. |
| `rvp-host` | `no_std + alloc` | The **host trait set** (section 4): `Source`, `AudioSink`, `VideoSink`, `Surface`, `InputEvents`, `Storage`, `HostClock`, plus (M8) the host-neutral `NowPlaying` model and `VisualizerTap`, and (M10) the optional `Library` capability (directory listing). Mock implementations for tests. This is the source of truth for media interfaces; Rusty Bucket's App API follows it. |
| `rvp-demux` | `no_std + alloc` | Our own incremental demuxers: ISO BMFF (MP4/M4A), Matroska/WebM (EBML), and (M9) raw audio: MP3 (ID3v1/v2, Xing/Info/LAME gapless), native FLAC, Ogg (Vorbis, Opus, FLAC), WAV (PCM, RF64), ADTS AAC; tags and cover art for all of them; plus probing. Async over `Source`. |
| `rvp-par` | std (wasm32 with shared memory) | (M9) Worker pool behind `rvp-core::par::Parallel`, `ThreadedVideoDecoder` (a decoder on a thread of its own), the pipelined H.264 decoder (parse and reconstruction threads). Native threads, or Web Workers through a host-provided spawner. |
| `rvp-codec-audio` | std (wasm32 ok) | `AudioDecoder` impls: AAC, MPEG layers I-III, FLAC, Vorbis (symphonia codec crates, unmodified), Opus (`ropus`), PCM (own, M9; up to 8 channels). Resampler (rubato). |
| `rvp-codec-h264` | `no_std + alloc` | **Our own** H.264 decoder (M6), `forbid(unsafe_code)`. Public modules that an encoder can share (Rusty Bucket plans one): `bitstream` (NAL/AVCC/Annex B, RBSP escaping, `BitReader` and `BitWriter`, Exp-Golomb), `params` (SPS with VUI, PPS, scaling lists, slice header, pred weight table, MMCO: each has `parse` and `write`), `transform` (inverse and forward 4x4/8x8/DC transforms, quantisation, dequantisation, scans), `cavlc` (tables plus `read_residual_block` and `write_residual_block`), `cabac` (context init tables, arithmetic decoder, arithmetic encoder, binarisation offsets). The picture decoder is `decoder` (macroblock layer, intra/inter prediction, direct modes, deblocking, DPB, output order); `h264_decoder()` adapts it to `VideoDecoder`. |
| `rvp-codec-av1` | std (wasm32 ok) | rav1d wrapper (needs a wasm32 patch, see risk R1). |
| `rvp-codec-vp9` | std (wasm32 ok) | VP9 behind our `VideoDecoder` trait (M7): wraps `rusty_vp9` (Apache-2.0, pinned `=0.1.1`), adds the superframe pull loop, `VideoFrame` conversion with colour tags, size caps and key-frame gating. Both candidate crates are `std`-only, so this crate is not `no_std`. |
| `rvp-subs` | `no_std + alloc` | SRT and WebVTT parsers, ASS/SSA (script, styles, override tags), PGS bitmap decoder, MKV/MP4 subtitle payload decoders (`S_TEXT/UTF8`, `S_TEXT/WEBVTT`, `S_TEXT/ASS`, `S_HDMV/PGS`, `tx3g`, `wvtt`), `CueList` (what is on screen at t). |
| `rvp-viz` | `no_std + alloc` | Visualizer analysis (M8): own radix-2 FFT, 32 log bands, level, adaptive onset detection and autocorrelation tempo, as `VizSummary`s from the audio being heard. M10: the effects (`effects.rs`: spectrum bars, oscilloscope, tunnel, starfield, plasma, three palettes) drawn on the CPU into a small RGBA picture that `rvp-ui` scales to the window; beat pulses capped at 3 a second and 12 percent, a calm mode for reduced motion. |
| `rvp-library` | `no_std + alloc` | (M10, done) The library: track/album/artist index with accent-folded sorting and search, the scan driver (`Scanner`: new, changed and gone files by path, size and mtime; tags, duration and cover art through `rvp-demux`; folder pictures), thumbnails (`zune-jpeg` and `zune-png`, 144 px), saved playlists with M3U/M3U8/PLS import and export (`rvp-player::listfile`), and the binary format the index, thumbnails and playlists are saved in through `Storage`. Directory walking comes from the `Library` host capability. |
| `rvp-player` | `no_std + alloc` | The engine: `Session` (cooperative tasks, pipeline, A/V sync, exact seek, frame step, A-B loop, subtitles, audio-track switching, WSOLA speed, gapless chaining and crossfade, loudness gain and limiter, visualizer tap feed), `measure_source` (the loudness of a whole file), `Playlist` (order, repeat, shuffle), events. Codecs come in through `CodecFactory`. |
| `theme` | `no_std` | Design-system tokens as `const`s (colour, spacing, radius, type scale, motion, glow). Generated by `cargo xtask theme` from `crates/theme/tokens/*.css`, a snapshot of `claude-design-system/tokens`. |
| `rvp-app` | `no_std + alloc` | The application glue (added in M5): owns the `Session` and the `Ui`, applies `Action`s to the player, builds the `UiModel`, composes picture + chrome into one RGBA surface and presents it. Hosts only forward input and call `App::tick`. Needs a `Host` whose video sink is `rvp_host::FrameSink`. |
| `rvp-ui` | `no_std + alloc` | Our UI: a small immediate-mode toolkit drawn into an RGBA framebuffer (rects, rounded rects, gradients, pre-rendered glow sprites, text via `fontdue`, Lucide icons as paths). Player view: video area, transport bar, seek bar, volume, playlist drawer, context menu, subtitle overlay. M10 added the Library face (`lib_ui/`: rail, header, virtual lists and grids, hero blocks, the now-playing screen, the visualizer view, the bar, prompts, context menus, keyboard and pointer), switched to with `B`. Same pixels in browser, desktop and Rusty Bucket. |
| `rvp-host-headless` | std | Native host for tests: file `Source`, a virtual-time clock, a null/WAV audio sink, a frame-hash `VideoSink`, scripted `InputEvents`. Binary `rvp-headless`. |
| `rvp-host-web` | wasm32 only | `wasm-bindgen` cdylib: File API `Source`, WebAudio `AudioSink`, `<canvas>` `Surface`, DOM input, `requestAnimationFrame` clock; Media Session API (now-playing, M8); PWA manifest and service worker (M11). |
| `rvp-host-desktop` | std | (M11) Native Linux app: `winit` window, `softbuffer` `Surface`, `cpal` `AudioSink`, real files and drag-drop, MPRIS now-playing and media keys. Binary `rvp`. |
| `rvp-host-rb` | std (wasm32 in the module) | Rusty Bucket adapter (M12): maps `rvp-host` traits (including now-playing, the visualizer tap and the library) to the App API (`bucket_v0` v0.3) through `bucket-v0-sys`; the loop, threads start-up, `video_present` layer. |
| `bucket-v0-sys`, `bucket-v0-mock` | `no_std` / std | (M12) Raw App API bindings with layout asserts, the documented function table and an import checker (Rusty Bucket may adopt it); a deterministic mock host for native tests. |
| `rvp-wave-bucket` | wasm32 only | (M12) The module of `Rusty Wave.bucket`: `bucket_main`, `bucket_save_state`, `bucket_thread_start`, codecs. `cargo xtask bucket` builds and packs it. |
| `xtask` | std | `cargo xtask theme | fixtures | web | serve | e2e | check | licenses`; from M11 also `desktop`, `flatpak`, `appimage`; M12 adds `bucket`, `bucket-smoke`, `bucket-e2e`. |

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
    fn visualizer(&mut self) -> Option<&mut dyn VisualizerTap> { None } // M8, optional (see docs/reference/host-api.md)
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
media interfaces follow, rust-os ADR-0026). The shapes, rules and mapping hints are in [`host-api.md`](../reference/host-api.md):

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
is a UI requirement here (section 9). Its App API (draft v0.3, after our review) has audio, files, now-playing, the visualizer feed and the library; M12 maps to it.

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

**M0 Plan and scaffold** (this one). *Done when:* `docs/planning/PLAN.md` exists, the workspace builds, `cargo test`
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
  once at the join and no clock discontinuity. Crossfade is an optional setting (see "Audio settings: crossfade and automatic level").
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
  were skipped over by a forward seek are recovered (see "Gap closing after M10"). Sidecar `.srt`/`.vtt`/`.ass`
  files (dropped or picked together with the video, or on their own onto a playing one) load through a session task and are selected
  at once. `SessionEvent::Subtitle` fires with the stream position when the text changes (tests: within one frame of the cue time).
  The UI draws wrapped lines on dark pills above the bar (ASS and PGS have their own drawing, below); the bundled font is a Latin subset.
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
  right after the current one ends (not gapless, but without a stall: see "Gap closing after M10"). Crossfade is an optional setting (see "Audio settings: crossfade and automatic level").
- **Now-playing.** `rvp-host::media` (`NowPlaying`, `NowPlayingMeta`, `Playback`, `TransportCommand`) and `docs/reference/host-api.md`. Tags and cover
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
- **x86_64 SIMD** (2026-10-06): the same kernels run natively through an SSE2 shim (`rvp_core::simd::x86`, baseline, no runtime detection), bit-exact against the scalar twins (selftests are native tests); desktop H.264 typical 0.9 → 0.64 core. aarch64 stays scalar; AVX2 and the long tail are optional follow-ups.
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
  pre-skip, 80 ms pre-roll after a seek) and FLAC-in-Ogg; other multiplexed streams are skipped, a chained stream plays on through its links
  (added after M10); seeks bisect on page granule positions. **WAV**: RIFF and RF64, PCM 8/16/24/32 and float 32/64 (also `WAVE_FORMAT_EXTENSIBLE`), `LIST/INFO` and
  `id3 ` tags; a truncated or open-ended data chunk is clamped to the file. **ADTS AAC**: header to AudioSpecificConfig, exact duration
  by a walk. `rvp-codec-audio` has a PCM decoder (mono and stereo) for WAV. `Metadata` gained album artist, track and disc numbers and
  totals, year and genre. Tests (`rvp-demux/tests/audio.rs`, set `audio` of `tools/gen-fixtures.sh`): stream parameters, durations, tags,
  cover art and **every packet's size and time against `ffprobe -show_packets`** for 14 files; ID3v1; gapless trimming; seeks on all
  formats; truncated and byte-flipped files; `rvp-host-headless/tests/m9_raw_audio.rs` plays each file through the whole pipeline and
  compares the output with ffmpeg's decode (lossless formats and WAV exact, lossy within -70 to -147 dBFS, lengths within a frame);
  `tests/e2e/m9.spec.js` plays nine of them in the browser. Not done: APE and AIFF (ReplayGain came with the audio settings; MPEG layers I/II, multi-channel PCM and
  chained Ogg came after M10).
- **Deviations from the brief.** wasm IDCT SIMD was skipped (not a hot spot); no frame threading for VP9; the "before" numbers come from a
  30 s baseline run, not 60 s; the Node-wasm timing numbers are not tabulated (Node is only used for the bit-exactness smoke tests and
  kernel self-tests, the browser is the speed oracle).

**M10 The audio-first view.** *(done 2026-10-05; see the notes below)* Makes RVP a music player as well as a video player; all of it lives in host-neutral
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

M10 notes (what was built, what it cost, what is not there):
- **Test library** (`tools/gen-library.py`, written by `tools/gen-fixtures.sh`, also `RVP_FIXTURE_SET=library`): 201 tracks in 20 albums
  made with ffmpeg: MP3 (ID3v2.3, v2.4, ID3v1 only), FLAC, Opus and Vorbis (the picture as a `METADATA_BLOCK_PICTURE`), AAC in M4A and WAV
  (`LIST/INFO`); embedded JPEG and PNG art on 11 albums, `cover.jpg`, `folder.png` and `Folder.JPG` beside the tracks of three, none on
  the rest; Latin, Turkish, Icelandic, Cyrillic and Japanese names; a compilation with and without an album-artist tag; two discs; a folder
  of untagged files; tracks without an album. The generator writes `expected.json` (the tree, from its own spec, not from a scan) and the
  tests compare against it: `rvp-host-headless/tests/m10_library.rs` (the exact artist/album/track tree, years, durations, codecs, the
  right colour of cover on the right albums, a rescan after one added, one changed and one deleted file reads exactly those three, and
  everything survives a restart) and `m10_app.rs` (see below). A second, small set (`--showcase`: 40 tracks of 2.5 to 3.5 minutes with
  generated covers and a beat) exists for the screenshots and the visualizer tests.
- **Scanning** (`rvp-library::Scanner`) runs inside the app's tick: up to four files in flight, at most about 4 ms of filing per tick, folder
  pictures only for albums that need them, the views rebuilt every 0.4 s. MP3 and ADTS are opened with `rvp_demux::open_quick` (no walk to
  the end: Xing frame count, else an estimate), so a library of big files is not read whole. 201 tracks scan in about 0.25 s natively and
  about 1 s in the browser; a JPEG cover decodes in tens of milliseconds in wasm, so the first scan has a few ticks of 40 to 250 ms. Bugs the
  fixtures found: MP4 `ilst` had no album artist, track, disc, year or genre (added), Vorbis `ALBUM_ARTIST` with an underscore was not read.
- **Index rules**: tracks group into albums by (album, album artist) or, with no album artist tagged, by (album, folder); a group whose
  artists differ is "Various Artists"; no album tag means "Unknown Album" under the artist; artists are album artists; sorting folds case and
  accents, ignores a leading "The" and compares numbers by value; search needs every word, in title, artist, album or genre.
- **Persistence**: `library/index` (a small versioned binary format, damaged blobs are refused), one `library/art/<hash>` per cover thumbnail
  (144 px, RGB: about 62 KB each, loaded a few per tick), `library/playlists`. Browser: `localStorage` is far too small, so keys under
  `library/` go to IndexedDB (`web/library.js`); the page loads them once before the player starts and the Rust `Storage` reads a copy, so it
  stays synchronous. A remembered folder is a File System Access handle in IndexedDB (read again without a prompt where the browser allows
  it); without that API a `webkitdirectory` input is the fallback and the folder must be picked again after a reload (its files cannot be
  reached until then: the app says so and the rail shows the folder as not connected). A dropped folder works too.
- **Playlists** (`rvp-library::plist`): saved playlists hold library paths; import resolves entries by the longest matching tail of the path
  (so absolute, relative, `file://`, Windows-style and bare names all land on the right file), keeps what matches nothing as a flagged missing
  entry, and re-matches when the files appear; export writes M3U8 (UTF-8, `#EXTINF`) or PLS. Import then export then import gives equal lists
  (tests for both formats, BOM, CRLF, lone CR, missing entries). Opening a `.m3u`, `.m3u8` or `.pls` (picker, drop, button) imports it.
- **Queue and gapless**: the queue is the M8 `Playlist` with library tracks as items (`Playlist` gained `track`, `insert_after`, `move_after`, a
  revision counter, and a shuffle that never plays an item twice in a row across the repeat-all wrap). Play, shuffle, play next and add to queue,
  a click on a row plays the list on screen from there. Headless tests (`m10_app.rs`) play a whole lossless album (matches the ten files played
  alone at both ends, nothing lost at the joins, no run of exact zeros near any join) and a mixed MP3/FLAC/Opus/AAC/Vorbis album (the lengths add
  up within 16 ms per join, no gap), with one now-playing update per track carrying the tags it should. 24 kHz sources come out at 48 kHz, so a
  join can differ from the files played alone by the resampler's edge (a few dozen frames), which is why the exact comparison is only at the ends.
- **Faces and views**: one app, two faces (`Mode::Library` / `Mode::Player`), `B` switches without touching playback; a song opened from
  outside goes to the Library face (Now playing), a video to the Player; `V` is the visualizer. Views: Albums (cards with a play button on hover),
  Artists, Tracks (sort by column head or menu), Playlists, Queue (Alt+Up/Down or drag to reorder), Search (artists, albums, tracks), album, artist and
  playlist pages, Now playing (cover, tags, up next, the video's picture for a video), the visualizer, an empty-library screen. Everything is reachable
  with the keyboard alone (arrows, Home/End, PageUp/Down, Enter, Shift+Enter add, Ctrl+Enter play next, Delete, Backspace and Esc back, Tab through
  rail and bar, digits for views, `/` or Ctrl+F search, the menu key) and with the pointer alone (every row and card has a context menu; the
  mouse's back button goes back). Plain arrows move in lists; Ctrl+arrows seek and change the volume, as do J/L, M and the bar.
- **Visualizer**: five effects, three palettes, `Left/Right` or the switcher, `C` palette, `T` title overlay, `Enter` on/off. It is driven by
  `Session::set_viz_capture` (the same analysis as the host tap, only while the view that shows it is up, so it costs nothing elsewhere). It is
  off by default with reduced motion (the view says how to start a calm version: no beat pulses or bursts, slow drift), a pause freezes the picture
  exactly, brightness changes on the beat are at most 12 percent and three a second. Tests: pixels change over 8 samples while playing, are identical
  over 6 while paused, all five effects draw, and with `prefers-reduced-motion` the picture rests and then moves gently (`m10.spec.js`).
- **Cost** (wasm, headless Chromium, per drawn frame; the picture is made at half or quarter size and scaled): at 1280x720 a redraw of a view is
  5 to 9 ms and a visualizer frame about 9 ms (30 a second); at 1920x1080 about 8 to 11 ms and 15 ms; on a 2560x1440 canvas 20 to 28 ms for a
  redraw (the visualizer then runs at 22 a second). Pointer moves only redraw when what is under the pointer changes. To get there `fill_rrect`
  fills the inside of a rounded rectangle by rows, `stroke_rrect` only looks near the edges, `shadow_rrect` evaluates on a coarse lattice and
  interpolates (all three have tests against the pixel-by-pixel versions), the now-playing screen dims the small visualizer picture instead of the
  big screen, and the cover shadow is a rim while the visualizer moves behind it.
- **Tests**: `rvp-library` 25 unit tests, `rvp-ui` 17 library-face tests (keyboard, pointer, menus, drag, drawing at several sizes and scales),
  `rvp-viz` effect tests (movement, stillness when paused, flash caps, calm mode), `rvp-player` playlist tests, headless `m10_library.rs` (4) and
  `m10_app.rs` (10), Playwright `m10.spec.js` (10 tests: a scan through the folder input, an album played with the media session following, the
  visualizer's pixels, search and menus, playlists made, exported, imported and kept after a reload, the library after a reload with covers, a
  File System Access folder (an origin-private directory behind a replaced `showDirectoryPicker`), the faces, goldens `library-albums.png`,
  `library-album.png`, `library-tracks.png` with a tolerance, and reduced motion), a `library` fuzz target (index, playlists, thumbnails, JPEG
  and PNG headers and bodies, playlist text: 700k runs without a finding), and 18 new screenshots in `docs/screenshots` (15 to 28).
- **Not done / limits**: the bundled fonts are Latin subsets, so Cyrillic and Japanese names are indexed, searched and sent to the Media Session
  correctly but drawn as boxes (a fallback font is a later decision: size); the Chromium headless shell crashes when it reads a File System
  Access handle for an origin-private directory back from IndexedDB, so the remembered-folder path is only tested up to storing it; a library of
  thousands of albums keeps its thumbnails in memory (62 KB each); no lyrics, no tag editing, no smart playlists; the queue and
  the current position are not restored after a restart (the library and playlists are); MPRIS and media keys on the desktop are M11.

**Gap closing after M10** *(done 2026-10-05; between M10 and M11, on its own branch)*. The known gaps of M8 to M10, closed or settled:

- **Test stability.** The browser specs no longer wait for amounts of time. `tests/e2e/helpers.js` has the waits every spec uses: a state
  of `window.rvp.snapshot()`, `frames(n)` (n animation frames: "the page has drawn what I just did", which is where the rectangles of the
  snapshot come from), `ticks(n)` (the page's own frame counter), `settled(read)` (a value that stops changing: the picture after a
  seek, the cards once their covers are in) and `playedFor(us)`. "Position runs at 1x" (and 2x) is now measured against the audio
  device's own clock (`AudioContext.currentTime`, shown in `window.rvp.audio().time`), not `performance.now()`, and a window in which
  the player stalled is measured again (up to four times): that is the one retry, and what it excludes is a stall on a busy machine,
  which says nothing about the rate. Other things that depended on how fast the machine was: "the subtitle appears within 300 ms" (it
  must not appear early and must be up while the cue lasts; the headless tests check the frame), A-B loop and playlist watchers (they
  run until the event happened, not for N seconds), the reduced-motion recording and the visualizer's "calm" check (the change is
  counted per 120 ms of real time between samples). Real elapsed time is still waited for where the claim is about time (controls
  hide after 2.5 s idle, a double click needs two clicks apart). One Rust test compared wall time (`exec.rs`: "10 s of virtual time must
  not sleep" now allows 5 s, not 100 ms). Proof: `cargo xtask e2e --threads` (both builds) three times in a row with CPU load; see the
  results next. Under 16 busy loops on a 16-core machine, three early runs failed 3, 3 and 1 tests (a subtitle checked after the cue
  was over, a frame step measured from the middle of a picture, a one-track queue that ended during a keyboard sequence; all
  fixed in the specs, none in the player); the next run, 46 of 46 on the single-threaded build and 46 of 46 on the threads build,
  passed. The series was stopped there at the user's request (the machine was saturated), so the proof is one clean loaded run of both
  builds plus several clean unloaded ones, not three consecutive loaded ones.
- **Subtitles.** *A cue already on screen when a forward seek lands now shows:* the demuxer lands on a video keyframe and used to
  read on from there, so the subtitle blocks that came before it (in the cluster, or in earlier ones) were never seen. After every
  seek the session asks the demuxer for the subtitle packets of the 20 s before the landing point (`Demuxer::side_packets`, Matroska
  reads the block headers of those clusters and only the subtitle blocks' payloads, without moving the read position; MP4 already
  seeks every track to the sample at or before the landing time); equal cues are not added twice. The test fails without the fix
  (`rvp-host-headless/tests/gaps_subs.rs`: a fresh session per seek, because the demuxer reads ahead and finds the cue by itself
  when it has read that far). *ASS/SSA:* `rvp_subs::ass` parses the script header (`PlayResX/Y`, `WrapStyle`, V4 and V4+ styles with
  the SSA alignment numbers), Matroska `S_TEXT/ASS` blocks (`ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text`) and
  whole `.ass`/`.ssa` files (sidecar). Override tags kept: `\b \i \u \s`, `\c`/`\1c` and `\alpha`/`\1a`, `\fs`, `\an` and `\a`, `\pos` and the
  start of `\move`, `\r`, `\N \n \h`; karaoke, `\t`, `\fad`, clips, fonts by name, rotation and drawings (`\p`: their text is dropped)
  are accepted and ignored. A cue carries `Rich` (spans with bold/italic/underline/strike/colour/size in thousandths of the picture
  height, alignment 1-9, position, margins, layer, outline colour); `Cue::text` stays the plain text, so events and the snapshot
  are as before. The UI draws styled text on the video's picture area (outline, italics by shearing the glyph rows, bold from the bold
  face, stacking per edge, word wrap, layers); the bundled fonts are Latin subsets, as before. *PGS (HDMV bitmap subtitles) in Matroska
  are done:* `rvp_subs::pgs::PgsDecoder` reads the segments of a display set (palette, run-length objects in fragments, composition
  state, cropping), keeps the epoch's palettes and objects between sets, and gives "show this picture" or "clear"; a picture stays up
  until the next display set. The UI blends the RGBA objects onto the picture area. The fixture is made by `tools/gen-pgs.py` (ffmpeg
  has no PGS encoder); the test compares our pictures **pixel by pixel with ffmpeg's rendering** (the same BT.601/BT.709-by-size
  colours). Fuzzing: the `subs` target covers ASS and PGS (1.0M runs; one overflow in `PlayRes` found and fixed with a regression input).
- **Audio.** *MPEG layers I and II:* symphonia's `mp1`/`mp2` are enabled; the raw demuxer reads layer I/II frames (all the bit-rate
  tables, 384 and 1152 samples per frame, MPEG 1 and 2); Matroska `A_MPEG/L1`/`L2`; the decoder follows the layer in each frame header,
  so a track that says "mp3" can carry any layer (MP4 does). `ffmpeg -c:a mp2` files match ffmpeg's decode to -98 dBFS (stereo and an
  MPEG 2 mono file at 24 kHz); there is no layer I encoder at hand, so layer I is wired and not tested with a file. *Multichannel:* PCM
  decodes up to 8 channels (WAVE_FORMAT_EXTENSIBLE too), FLAC and Vorbis already did; the stereo mix (centre and surrounds at -3 dB,
  the back centre at -6 dB, LFE dropped, scaled so the largest row sums to 1) equals ffmpeg's `aresample=rematrix_maxval=1.0` at
  -100 dBFS for 5.1 (FLAC, WAV 16 and 24 bit) and 7.1 (FLAC, float WAV): the 7.1 weights of the first version differed from ffmpeg's
  and were fixed by reading ffmpeg's matrix back from its output. *Chained Ogg (Vorbis, Opus, FLAC):* the demuxer plays on through the
  links, the timeline continues, the next link's headers go to the decoder as ordinary packets (`ChainDec` in `rvp-codec-audio` rebuilds
  the decoder and drops the Opus pre-skip, which in a chain is not at the start of the file), the duration is the sum of the links and
  seeks work across them when the file was scanned (a file whose end belongs to another stream than its start is scanned once; a
  chain that reuses one serial number and ends in a link longer than 128 KiB plays through but shows the length of the last link).
  The output equals the links decoded alone, one after the other (Vorbis -163 dBFS, FLAC exact, Opus -96), also after a seek into the
  second link. *AAC:* symphonia 0.6 has **no SBR and no AAC with more than two channels**. `HE-AAC` signalled backward compatibly (the
  usual way) decodes as its LC core at the core's sample rate, band-limited; with explicit hierarchical signalling (object type 5/29)
  and 5.1/7.1 the decoder is refused as `Unsupported` ("aac too complex"), the video plays without sound and the warning says why
  (`video_aac51.mp4` test; the container does not matter). FDK-AAC here has no SBR encoder either, so HE-AAC files could not be
  produced: the checks are on hand-made AudioSpecificConfigs (`rvp-codec-audio/tests/aac_limits.rs`). Not done: layer I
  test file, Opus multistream (more than two channels).
- **Gapless with items that have no sound, or arrive late.** Measured with a harness that records the host time of every picture
  (`rvp-host-headless/tests/gaps_gapless.rs`: audio and video items, video-only items and audio-only ones in every order, and a source
  whose reads wait for the host clock): a next item without audio joins the previous one with one tick of delay (no picture waits more
  than 1.5 frames), and a late item shows its first picture within two frames of its file arriving. The bug found on the way was in the
  test host and in the contract: `NullAudio::open` kept the count of what an earlier stream had left queued (its drain is lazy), so an
  item with sound after one without it started a second late. `AudioSink::open` now says it starts a new stream, the headless sink
  starts clean and the session flushes the sink when it opens it (the web host's `open` already did).
- **AV1 single-thread speed** (1080p30, `cargo xtask perf-web --single --only av1`, headless Chromium, this machine). Profile first:
  `Profiler.start` through the DevTools protocol on the wasm build with its names kept (`wasm-bindgen --keep-debug`) showed
  `prep_8tap_rust` at 16% of the time of the page, then `msac` (7%), our colour conversion (5%), CDEF (5%), `decode_coefs` (4%), our
  scaler (6% with `blend_rows`), `avg` (3.5%). The 8-tap filters kept their intermediate rows in a 34 KB array that was zeroed for every
  block and read the picture one pixel at a time through the checked accessor; the fix (the same integer sums, row-wise and
  vectorisable, `MidRows` not cleared), loops over zipped slices in `avg`/`w_avg`/`mask`, a 64-bit arithmetic-decoder window (the
  32-bit one on wasm32 refilled twice as often) and block-size-generic CDEF are `third_party/rav1d/PATCHES.md` items 9-12. The decoded
  pictures are bit-identical: the 1800 frames of the stream hash the same before and after (`cargo run --release -p rvp-codec-av1
  --example av1_bench`, which also times the RGBA conversion), the conformance tests pass and `cargo xtask wasm-smoke` agrees native,
  wasm and wasm SIMD128 for 8 and 10 bit. Our colour conversion and the frame copy are untouched: the profile shows the conversion
  already at its SIMD128 limit (1.4 ms a frame) and the copy at 1%.
  Result, before and after, same harness and machine, alternating runs of 60 s (the machine was shared with other builds, so the
  absolute numbers wander from run to run; compare the rows of a pair):

  | Pair (1080p30 AV1, single-threaded, 60 s) | Dropped before | Dropped after | Session ms per tick before | after |
  | --- | --- | --- | --- | --- |
  | 1 | 21.4% | 14.2% | 15.45 | 10.77 |
  | 2 | 20.2% | 16.6% | 16.19 | 12.07 |
  | 3 | 27.3% | 25.3% | 21.42 | 16.52 |

  A CPU profile of 20 s on a quieter minute: the main thread's busy time per frame fell from 22.8 to 19.7 ms (-14%) and the dropped
  share from 18.5% to 11%. The time per tick is 23 to 30% lower in every pair; the dropped share moves less, and by a smaller step
  when the machine is busy, because a frame is dropped when a tick (the decode of one frame plus drawing) runs past two vsyncs, which
  depends on the tail of the load more than on its mean. The 9.57% of M9 was measured on a quieter machine. What remains is rav1d
  itself in portable Rust (the arithmetic decoder, coefficient reading and the inverse transforms are about a third of the time); the
  threads build is still the answer for 1080p AV1 on a slow CPU.

**M11 The standalone desktop app and packaging.** *(done 2026-10-05; see the notes below)* The "standalone" edition (a):
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

M11 notes (what was built, what was checked, what is not there):
- **Desktop host** (`crates/rvp-host-desktop`, binary `rvp`): `winit` 0.30 (Wayland and X11, Windows), `softbuffer` (the app's RGBA frame converted to the window's pixels;
  no `pixels`/wgpu), `cpal` through a ring buffer (the device callback reports its latency; a silent wall-clock sink stands in without a device), `rfd` dialogs on a
  worker thread (so decoding and sound never wait for a dialog), drag and drop of files and folders, `file://` URIs, full screen (`F`, `F11`, Alt+Enter), HiDPI, window size
  remembered, MPRIS (Linux) and SMTC (Windows) through `souvlaki`, files for settings and the library (atomic writes), folders walked on threads, video decoded on its own
  thread with H.264 pipelined and the pixel kernels on a pool (`rvp-par`). It depends on nothing browser- or Rusty Bucket-specific. Scripting options (`--exit-after`,
  `--screenshot`, `--report`, `--press`, `--data-dir`, `--no-audio`) make the Xvfb smoke tests and package checks possible.
- **souvlaki is vendored** (`third_party/souvlaki`): its D-Bus backend applied a state change only when the next D-Bus message arrived or a second had passed, so
  `playerctl status` right after `pause` showed the old state. Patch and reasons in its `PATCHES.md`. The `zbus` backend does not build on the nightly toolchain
  (old `rustix`), which is why the `dbus` backend (and `libdbus-1`) is used. MPRIS clients read `Position` as a plain number, so the position is re-sent every second.
- **M10 polish done:** a bundled Noto Sans subset (Greek, Cyrillic, Latin extended, Vietnamese; 68 KB, in every host) and a host `FontLoader` hook; the desktop
  finds a system font for other scripts (CJK, Arabic, ...) on first use, once per character, with a cap on failed searches; thumbnails sit in a byte-budgeted LRU
  (`rvp-library::thumbs`, evicted ones are read back on demand, unsaved ones are never dropped); the queue and position are restored (`rvp-app::restore`, tests in
  `m11_restore.rs`: positions, shuffle and repeat, command-line files win, damaged blobs, browser-like ids).
- **Brand:** the app is **Rusty Wave** (renamed from "Rusty Video Player" on 2026-10-05). The official icon master (a rusted-metal play triangle with neon waves,
  `assets/brand/rusty-wave-icon-master.png`) is sized by `tools/gen-brand.py` into hicolor PNGs 16 to 512, `.ico`, `.icns`, installer bitmaps, PWA icons (maskable
  on the night background), the favicon and the in-app logo; 32 px and below use a tighter crop. No scalable SVG (no vector source). No VLC cone.
- **Packaging** is described in [`packaging.md`](../release/packaging.md): `cargo xtask dist`, Flatpak, AppImage, .deb, .rpm, Windows exe and Inno Setup installer, PWA, signing hooks,
  release workflows (tag or manual only), and the verification record.
- **Findings along the way:** winit's X11 backend panics when `libxkbcommon-x11` is missing, so packages depend on it; `cargo-deb` ignores its own copyright asset when a
  copyright is generated (use `license-file`); Inno Setup under Wine needs `ProgramW6432Dir` set; Docker Hub's CDN is not reachable from every network (an ECR mirror
  works for the Ubuntu base).
- **Not done / limits (as of M11; since then the release workflows have run for every tag and macOS has a beta build, see `release/packaging.md`):** no single-instance
  forwarding (a second `rvp file` starts a second window; it gets an MPRIS name with `.instance<pid>`); the web build still draws CJK as boxes (no system fonts in a page);
  the Flathub submission is not made.

**Audio settings: crossfade and automatic level** *(done 2026-10-05, after M11, on its own branch)*. Two user-visible audio settings in an Audio panel
(`U`, or "Audio effects" in the context menu of either face), kept in host `Storage` under `settings/audio` (so the browser's `localStorage` and the desktop's data
directory both have them) and shown in the app snapshot (`audio`, `audio_panel`). Everything is off by default and **off means the old paths, sample for sample**.

- **Crossfade** (on/off, 2 to 10 s in 1 s steps). Equal-power (cos/sin) between consecutive queue items, mixed in `rvp-player::audio::AudioOut`: each item has its own *lane*
  (mix to the sink's channels, resample, trim) and the fade mixer sums the end of the first and the start of the second into one stream. The item decides where it begins
  (`Session::fade_threshold`): the audio fed has reached its length minus the fade, so the second item must be open and decoding by then (the app queues it
  `crossfade_secs` earlier than before). **The clock, the item (`ItemStarted`, now-playing, titles) and the visualizer's time switch in the middle of the fade**, where both are
  3 dB down: the timeline gets a new segment there, so heard position and `heard_item` stay exact. The tap and the sink get the mixed output.
  *Which joins.* Only audio-only items: any picture on either side keeps the plain gapless join (a fade of the sound would leave the picture of the first item hanging over the
  second item's first seconds, or cut the second item's first seconds of picture; not worth it). Consecutive tracks of one album that are marked as a gapless album (iTunes
  `pgap`, `iTunPGAP`, Vorbis `ITUNPGAP`/`GAPLESS`; same album, adjacent track numbers) run gaplessly; tracks of such an album that are not adjacent (shuffle) are faded. The fade is
  at most a third of the shorter of the two items (a 30 s track gets 10 s at most, a 2 s one 0.67 s), and when that is under 0.25 s the join stays gapless; repeat-one is not faded into itself. **A manual skip
  (Next, Previous, a click in the queue) is a hard cut, as before**: the user asked for the other track now, and a fade would need the old session's audio to outlive it. If the
  first item ends sooner than its container said, the rest of the fade is the second item alone (a 10 ms ramp to full); if it runs on longer, its tail is dropped when the fade ends.
  A seek, a speed change or an audio-track change abandons a fade. The fade is as long as asked minus at most one decoder buffer (the feed stops at the first buffer past the
  fade point: up to 85 ms for FLAC).
- **Auto-level** (on/off, target -23 to -10 LUFS, default -14, track or album). The gain of an item is the target minus its loudness, from, in order: the file's own tags
  (ReplayGain 2 in ID3v2 `TXXX`, Vorbis comments in FLAC, Ogg and Opus, Matroska `SimpleTag`s, MP4 `----` items and QuickTime `mdta` keys; Opus `R128_*_GAIN`, Q7.8 against
  -23 LUFS on top of the header gain, which the decoder applies), then the library's measurement (below), then a **running estimate**: the item's BS.1770 loudness so far, used
  once 8 blocks (about 1.1 s) are above the gate, with its goal moved only when the estimate changes by more than 0.5 dB, and the gain following it at 1.5 dB a second up and 4 dB a
  second down (so a loud start is tamed before a quiet one is lifted); nothing is applied before the first estimate. Gains from tags or the library are reached in about 0.1 s
  (120 dB/s slew, no click). The gain is held to +12 dB and -24 dB. Album mode uses the album's figure (tag, or, for library tracks, the duration-weighted energy mean of the album's
  tracks once all are known) and falls back to the track's own; files outside the library have no album figure. Mono is measured as dual mono (what comes out of the speakers).
  **Clipping:** a true-peak look-ahead limiter on the output (ceiling -1 dBTP, 2 ms attack, 60 ms release, 4x oversampled peak detection, a final clip on the samples), engaged
  only while auto-level is on, so a boost cannot clip; it holds back about 3 ms of audio for its look-ahead and lets go when the sink is nearly empty. The visualizer tap sees the audio
  after gain and limiter (it is still before volume and mute).
- **The meter** (`rvp-core::loudness`): ITU-R BS.1770-4 K-weighting (coefficients computed for any sample rate; they reproduce the 48 kHz ones to 1e-9), 400 ms blocks every 100 ms,
  absolute gate -70 LUFS and relative gate -10 LU, momentary and short-term, channel weights for up to 8 channels in film order, optional true peak. Tests: the EBU Tech 3341
  tone cases (1 and 2: steady -23 and -33 LUFS; 3: relative gate; 4: absolute gate; 5: two levels; true peak of an fs/4 tone at 45 degrees and of a sine), and, on generated files through
  every codec and at 22.05, 44.1 and 48 kHz, **within 0.01 LU of `ffmpeg -af ebur128`** for the integrated loudness and 0.02 dB for the true peak
  (`rvp-host-headless/tests/levels_meter.rs`; ffmpeg's reading is saved when the fixtures are made).
- **The library** keeps each track's loudness in the index (format version 2; version 1 still loads and has none): the tags' figures come with the scan, and, while the
  automatic level is on, tracks without one are decoded and measured in the background (`Scanner::start_analysis`: one file at a time, 4 ms of decoding per tick, a turn for the
  UI every 16k frames, saved every 25 tracks, not while a picture plays, shown as "Measuring n / m"). Unchanged files are not measured again; a changed file loses its figure.
  Turning the setting on later measures what the library already holds.
- **Tests.** `rvp-core` (meter, limiter ceiling on hostile input and under every chunking, gain stage, equal-power law, settings text), `rvp-demux` (tag parsing per format in memory and
  from real files in every container, `tests/levels_tags.rs`), `rvp-player::audio` (fade sums, timing and timeline, early end of the first item, too-short fades, level ramps,
  convergence without pumping, limiter), `rvp-host-headless/tests/levels_crossfade.rs` (sums and timing on tones through the whole session; the clock and item switch in the middle;
  off equals the gapless path sample for sample; gapless albums, short and tiny tracks, video never faded; a seek during the fade; the tap sees the mixture),
  `levels_auto.rs` (gains from tags in every container, track and album mode, the library hint, the running estimate settling at the right gain without pumping, the limiter),
  `levels_library.rs` (scan, measurement, persistence, an old index), `levels_app.rs` (panel with keyboard and mouse on both faces, settings kept and restored and damaged storage
  ignored, a crossfade through the app, background measurement), a desktop smoke test (keyboard changes, a restart keeps them) and `tests/e2e/levels.spec.js` in the browser.
- **Not done / limits.** No crossfade on manual skips and none with a picture; no per-album setting; the running estimate starts at 0 dB for its first second; ReplayGain in APE tags,
  LAME header gains and `RVA2` frames are not read; the Opus header gain alone (without R128 tags) says nothing about loudness; the album figure for library tracks needs every track of the
  album to be measured; the measurement decodes whole files (a fast native machine does a track in a fraction of a second, WebAssembly several times slower, which is why it is background
  work and only runs while the setting is on); the limiter's true-peak filter is a 12-tap Kaiser design, not the standard's exact table (within 0.1 dB on the Tech 3341 cases).

**M12 Rusty Bucket adapter** (was M10). **Done against App API v0.3: tested on a mock host and run in the real Bucket Simulator (`cargo xtask bucket-e2e`); running on the OS itself is left (QEMU, runtime choice).**
`rvp-host-rb` maps `rvp-host` (playback, `NowPlaying`, `VisualizerTap`, `Library`) to Rusty Bucket's App API (`bucket_v0`), whose
media interfaces rust-os models on ours (ADR-0026). Nothing in core, `rvp-host`, `rvp-ui` or `rvp-app` was changed for it; the
standalone rule holds (none of them knows Rusty Bucket). The API itself took our review (`docs/reviews/app-api-v0-review.md`: 28 items in
v0.2, 13 deltas in v0.3, all folded in), so the adapter has no workarounds left for ambiguities.

- **Crates.** `bucket-v0-sys` (raw imports of module `bucket_v0`, `#[repr(C)]` structs with compile-time size and offset asserts, constants
  and error codes, the function table, a WebAssembly import parser and checker; no dependencies and no rvp knowledge, so Rusty Bucket
  can adopt it), `bucket-v0-mock` (a deterministic mock host on virtual time for native tests), `rvp-host-rb` (the adapter and the loop),
  `rvp-wave-bucket` (the wasm module: `bucket_main`, `bucket_save_state`, `bucket_thread_start`, the codecs).
- **What the adapter does.** One loop: tick, then `events_wait(timeout = request_wake - now)`; events become `InputEvent`s and requests
  (wheel lines are 40 x scale px). Files are non-blocking (`-BUSY` is `Pending`, `IO_READY` ends the sleep; a bounded wait for slow
  `kv_load` and `file_open_id`, other events kept meanwhile). `Storage` on kv (thumbnails in the cache class). Audio on `audio_open`,
  `audio_queued` and `audio_latency_us` (derived from `audio_clock` when missing; a clock-driven silent stand-in without a device).
  `NowPlaying` with `TRANSPORT` commands (strings cut at 4 KiB on a character, covers over the limit or not PNG/JPEG left out, replayed
  when the shell appears), `VisualizerTap` through `viz_block` and one `viz_summary_n` per tick, `Library` through the listing cursor
  (partial listings append; escapes in `library_roots`), picks, saves, drops and launch files (one event per file, grouped into one
  queue), `power_inhibit` (system awake while anything plays, display while a picture does), `launch_reason` (a toast after a crash), `restart`
  (saves first), `TERMINATE` (save, flush, release the session), `SUSPEND`/`RESUME`/`VISIBILITY` (no frames while hidden, a full redraw
  after), `MEMORY_PRESSURE` (thumbnail budget), `CAPS_CHANGED` (the optional interfaces follow the bits).
- **Video.** Plan A: the app composes picture and UI and the adapter calls `canvas_present` (non-blocking; `-BUSY` for a stale size is
  dropped, the `RESIZE` fixes it). `video_present` exists as `rvp_host_rb::video::VideoLayer` (layout tested, behind `VIDEO_YUV`) but
  nothing calls it: it needs an app that leaves a transparent hole, which is a change to `rvp-app` we do not make for one host.
- **Threads** (the threads build only, per the start-up contract): the app allocates each thread's stack and TLS and passes them in
  `arg`; `bucket_thread_start` switches the stack pointer before it touches the stack, then runs `__wasm_init_tls`; the main thread
  initialises its own TLS first. Checked in Node with real Workers (`cargo xtask bucket-smoke`; a build that skips the stack switch fails it).
- **Builds and package.** `cargo xtask bucket` builds `app.wasm` (baseline, no SIMD), `app.threads.wasm` (SIMD128, atomics, shared
  memory, nightly + `rust-src`; `--simd` adds `app.simd.wasm`), checks every module's imports against the documented set, and packs
  `target/bucket/Rusty Wave.bucket` (ZIP: `manifest.toml`, modules, icons, `CHECKSUMS`, `SIGNATURE` from `RVP_BUCKET_SIGN_CMD`), then
  runs the modules in Node. The manifest is `packaging/bucket/manifest.toml.in` (app ID `io.github.unicorntearsproject.RustyWave`, class `media`,
  `[[builds]]`, file types, `restart = "on-trap"`).
- **Tests** (all native and headless, no Simulator): `bucket-v0-sys` (layout asserts, function table equal to the documented set, the
  parser and the drift checker), `bucket-v0-mock`, `rvp-host-rb/tests` (lifecycle and events, files with `-BUSY`/`IO_READY`, audio clock and
  devices, kv, now-playing and transport, visualizer, power, library cursor, video layout, manifest), `rvp-wave-bucket/tests/imports.rs`
  (builds the module for wasm32 and requires its imports to be exactly the documented functions with the documented signatures).
- **Bucket Simulator** (`../rust-os/tools/bucket-sim`, built with `CARGO_TARGET_DIR=/tmp/... cargo build --release --manifest-path ../rust-os/tools/bucket-sim/Cargo.toml`):
  `cargo xtask bucket-e2e [--sim PATH] [--only NAME] [-v]` (or `$BUCKET_SIM`; skipped when there is none) runs the packed `Rusty Wave.bucket` headless on
  the virtual clock through 15 scenarios (`xtask/src/bucket_e2e.rs`): the empty state; H.264 + AAC (threads build) and the baseline build; FLAC with tags and
  cover with the audio clock at 1x; pause and resume (the audio device stops with the clock, checked on the captured audio); seeking by key and by click;
  now-playing and every `TRANSPORT` command; several launch files as one queue; the library (add folder, one OS walk, listing, play, launch listing,
  restart restores the queue paused); hot reload; a cold volume (`-BUSY`/`IO_READY`) for video and library; failing fetches (`-IO`); an audio device change
  and failure; resize, scale, fullscreen, hidden windows. Screenshots go to `target/bucket-e2e/<scenario>/`. A run fails on any app warning or unexpected
  simulator message. The adapter writes level-4 trace lines the scenarios read. Mismatches between the spec, the simulator and our expectations:
  `docs/reviews/app-api-v0-review.md`, "M12 vs bucket-sim" (found and fixed: a second library walk after `FOLDER_ADDED`, launch files replacing each other,
  `restart()` read as returning, cover art checked by type only).
- **v0.3 clarifications** applied in the adapter and in `bucket-v0-mock` (which now reads the pages as the simulator does; `bucket-v0-mock/tests/v03.rs`).
  Decision 102 (rust-os `e00194b`, simulator `96963ee`) is applied too: `OPEN` groups by flag bit 1 (our batching stays as the fallback for older hosts), launch files only on
  `launch_reason` 0, the first now-playing report after metadata counts as a change, the OS walks a never-finished root at launch, a 0-byte save is not logged, an
  underrun is a mid-play gap only. The `bucket-e2e` workarounds are gone and the 15 scenarios pass against the current simulator.
- **Not done / limits.** No `FRAME` pacing (the core presents against `now`), no hot-reload state (everything is in the store; a reload comes back paused), no theme or
  Bucket Bar commands or clipboard (we draw our own look), no device selection, no `file_open_sibling` use (the host traits have no sidecar open). Still open:
  QEMU under Rusty Bucket (runtime choice: AOT or interpreter, risk R3), `video_present` in the real pipeline, the answers to items 8 to 11 of the review section.

### Phase A of the 2026-10-06 batch (v0.0.5)

Features added on top of M12 (details in the docs named in brackets):

- **First run and the last face** ([`host-api.md`](../reference/host-api.md)): the first run opens on the Library and adds the system's Music and Videos folders; later runs reopen the last face.
- **Settings, default media player, app-menu offer** ([`host-api.md`](../reference/host-api.md), [`packaging.md`](../release/packaging.md)): one Settings dialog (rail button, right-click menu, Ctrl+,); a checklist of every
  media type the player opens (`MEDIA_TYPES`) with per-platform behaviour (Linux `mimeapps.list`, Windows registration plus *Default apps*, macOS Launch Services); "No thanks" is final.
- **Video library**: the same folders, index and queue as the music, a Videos view (poster grid or list, sort, search, resume markers), posters made by decoding one frame in the background.
- **Theming**: a pasted Claude Design link or CSS becomes a theme (`rvp_ui::theming`): colours and corner radii mapped onto the runtime tokens (`rvp_ui::tk`), contrast checked, previewed live, kept per user.
  Fonts are only noted (the app keeps its bundled faces). Fetching goes through the optional `Net` host capability; without it, paste the CSS.
- **Visualizer**: a Rainbow palette and three scenes in the spirit of Unicorn Viz (Bass machine, Unicorn Tears, Disco ball), CPU-drawn, calm with reduced motion. None needed heavy core work.
- **Library folders after a restart (1.0.0-rc7)**: on the desktop the Music and Videos folders found on the first run were never written to the saved folder list (`desktop/roots`; only folders added by hand were), so after a restart the library listed everything from its saved index but every folder was disconnected and nothing could play ("Those files aren't reachable right now"). The desktop library now notes every change to its folders (`roots_dirty`) and the window saves the list at once, remembers a folder that is missing at a start (an unplugged drive) instead of dropping it, and reconnects every folder it can read at each start. The smoke report gained `library_playable_tracks`, `library_playable_videos` and `library_roots_connected`, and `the_libraries_found_on_the_first_run_still_play_after_a_restart` runs the whole story with a throwaway home: discover, play, restart, play an album, a song and a film from the library, hide a folder, bring it back. The web host keeps a folder's handle when the folder is added (IndexedDB) and the Rusty Bucket host asks the OS for its roots each time: neither had the bug.
- **Web film audio under load (1.0.0-rc7)**: (the sink ring is 1 s and the player aims to keep 900 ms of audio ahead when pictures take the turn.) A 1080p film in the single-thread web build at a quarter speed starved its audio (422 silences, 32 s of silence in 60) because the scheduler gave the sound one packet per round and every round decoded a whole picture, and later because the demuxer stopped reading when the picture packets piled up. A turn now works pictures only while its 8 ms budget lasts, then goes on for up to 25 ms with cheap rounds (demuxing and audio decoding) while less than 900 ms of audio is waiting; the demuxer keeps reading past the picture queue's limit while the sound is short; and a decoder 96 packets (about three seconds) behind jumps to the newest keyframe of the backlog. `audio-underrun.spec.js` plays a 1080p film with `--disable-gpu` and a four-times-slower page while menus open: zero silent frames and at least 200 ms in the ring at its lowest, on both builds (the threads build needed no change; its decoder is off the main thread).
- **Web audio under a busy page (1.0.0-rc6)**: demo QA heard trains of 25-40 ms silences when the poster grid or a menu redrew over a song or a film in headless software-rendered Chromium. The AudioWorklet now counts what it plays when it has nothing (`window.rvp.audio()`: `underFrames`, `underRuns`, `ringLow`, `ring`; `window.rvp.audioReset()` restarts them) and `tests/e2e/audio-underrun.spec.js` plays a song and a film while the page runs at a quarter speed (Chrome's CPU throttle) and the grid scrolls and the menus open and close: zero silent frames and at least a quarter second left in the ring at its lowest. The fixes: the audio decoder takes up to 16 packets per scheduler turn (a late frame used to leave it one packet's worth, about 20 ms, per turn), the decoded-ahead target is 800 ms (was 500), the web sink held 750 ms in rc6 (1 s again from rc7) and the AudioContext asks for the `playback` latency hint (a bigger output buffer rides over a busy audio thread; the player's clock already accounts for the output latency; `?audiohint=interactive` restores the short one). Control latency is unchanged or better: volume and mute are the output's gain node, pause holds the ring, seek and skip flush it (a second test of the same spec checks them with a full ring), the visualizer tap follows what has been heard, not what was queued; the level gain and the crossfade are applied before queueing as before, so a change of either is heard after at most the ring's length (1 s).
- **Visualizer round (1.0.0-rc5)**: two more effects ported in spirit from Unicorn Viz (MIT; written from scratch for the CPU): **Bass cathedral** (an endless nave, bass-flexed pillars, a core glow, godrays, a recoil and shockwave on the beat; a table-driven per-pixel pass at a sixth of the window) and **Sun Ship 3000** (enemy formations, laser streams, a bass-pushed ship, a beat volley and explosion). The **Bass machine**'s first scene became a speaker wall filling the picture (two subs, a mid column and a row of tweeters, each cone pumping with its band, light drips running down the edges and sparkles all round the frame; the machine now draws at a third of the window); the **Disco ball** got 28x14 mirror facets with hard glints against three orbiting lights, a denser lattice of tilted reflected spots with star glints, drifting room sparkles and beat flashes on a changing subset of facets. All of them: palettes work, calm mode has no beat reaction at all, no beat changes the mean brightness by more than the 12 percent flash limit (Bass cathedral clamps it), entity counts are capped. Cost per frame (`cargo run --release -p rvp-viz --example cost`; wasm SIMD128 in Node, `node tools/wasm-selftest.mjs <wasm> --bench`), 1080p window: desktop 2.7 / 1.3 / 4.3 / 4.0 ms (cathedral, ship, machine, ball), web single-thread 4.8 / 3.9 / 7.6 / 7.9 ms; the existing Spectrum costs 9 ms on the web, so nothing new is the slowest.
- **Web build**: content-hashed names, `build-info.json`, the threaded build in the zip, a service worker that precaches the build the browser can run; `dist --target rustybucket` lays a release out for
  Rusty Bucket's release site.

### rc8 round (2026-10-07)

- **Installer icons**: a 128 px `share/pixmaps` icon for deb/rpm, the `.ico` on the Windows shortcuts and in Add/Remove Programs, `.VolumeIcon.icns` on the macOS disk image; `xtask verify` checks the icon in the Flatpak bundle and the AppImage.
- **Back on every Settings page** (button, Esc, Backspace, Alt+Left; the keyboard returns to the button that opened the page).
- **First run never adds `$HOME`**: a Videos or Music folder that is `$HOME` or above it is left out with a note; the walker follows symlinked folders without looping, skips hidden and junk files, and is bounded. Scale check on a real 10,954-track, 3,659-album music folder: about 130 s on an idle-priority scan, 137 MB peak, no errors.
- **Play history**: each play of a song or video (counts after 30 s or half the item, whichever comes first; the time heard and finished-or-skipped are kept) is stored by the same identity as favorites, bounded to 10,000 plays (`library/history`). A **History** view (key `0`) lists Music and Videos newest first, grouped by day (Today, Yesterday, dates) with the time and a play-count badge; rows play, queue, heart, open the album, and **Delete** (or the menu) removes a play. **Clear history** asks first. The Tracks and Videos views sort by *Most played* and *Last played* and show the count and last day while they do. **Pause history** in Settings stops recording. Plays are dated by `HostClock::unix_time` and grouped by the local day (`utc_offset_secs`).

- **Platform video decoders**: HEVC (Main, Main 10) and 10-bit H.264 through the system where we have no decoder of our own: WebCodecs in the browser, VA-API (dlopen, no build-time dependency; verified bit-exact against ffmpeg on 10 x265 streams: slices, weighted prediction, scaling lists, open GOP, odd sizes, Main 10), VideoToolbox, Media Foundation. The stream layer (`rvp-codec-hevc`: parameter sets, slice headers, POC, RPS, DPB, output order) is shared and is the base of our own HEVC decoder (rc9, below). HDR10/HLG is tone-mapped to SDR (`rvp_core::hdr`). Every user-facing error now says what, why and what next (`rvp-app/src/messages.rs`); toasts and the error card wrap at any width.
- **Phone layout**: under 600 px the rail is a drawer behind a menu button, the bar and the player controls are touch-sized rows (44 px targets), the first tap on a playing picture shows the controls, the install strip sits above the canvas; tested at 390x844, 360x800 and 3x.
- **Install guidance per platform**: a computer is offered the desktop app first (software.rustybucket.ai) and the web app second; Android/ChromeOS the browser's prompt; iOS Add to Home Screen with the note that folder libraries are not available there; Safari on a Mac Add to Dock; Firefox on a computer the desktop app.

### rc9 round (2026-10-07): our own HEVC decoder

- **Software HEVC decoder** (`rvp-codec-hevc`, `sw/`; clean-room from ITU-T H.265, ffmpeg and x265 only as binary oracles; `no_std + alloc`, builds for wasm32): Main and Main 10, 4:2:0, progressive; CABAC; every intra mode; 4 to 32 transforms, DST, transform skip, lossless and PCM; scaling lists; merge, AMVP and temporal motion vector prediction; all partitions incl. AMP; weighted prediction; deblocking and SAO; tiles, WPP, several slices and dependent slice segments; long-term references; reordering and bumping; VUI/SEI colour information feeds the existing HDR to SDR path. Range extensions, SCC, multilayer and interlaced streams are refused with the clear message (and go to the platform decoder where there is one).
- **Ours first** in every host (desktop, web, headless, Rusty Bucket); the platform decoder stays behind it for what ours refuses (`FallbackVideo`), so HEVC plays everywhere, including where no system decoder exists.
- **Conformance**: bit-exact with ffmpeg on 60 x265 streams (8/10-bit, presets, CTU 16/32/64, B-pyramid, open GOP, rect/AMP, weighted prediction, deblock/SAO variants, lossless, scaling lists, slices) and on 16 structure streams written by a test-only HEVC writer with its own CABAC encoder (`tests/structure.rs`: tiles uniform and not, one-CTB-wide tiles, WPP, tiles with WPP, slices and dependent segments in every place, PCM). ffmpeg numbers WPP rows by tile-scan position, which is wrong with tiles, so tiles + WPP are compared with ffmpeg's decode of the same content without WPP (the picture does not depend on WPP). Robustness: `tests/robust.rs` damages packets (flipped bits, cuts, drops, repeats, zeroed runs) on nine streams; `RVP_FUZZ_SEEDS=150` found and fixed three panics.
- **Speed** (release, one thread, 1080p30, 7 Mbit/s typical, `--example bench`): 38.7 fps Main, 36.0 fps Main 10 on desktop. In the browser (`cargo xtask perf-web --both --only hevc`, 15 s): single-threaded build 0.0 % dropped Main, 1.7 % Main 10; threaded build 0.0 % and 0.4 %. No SIMD yet (the compiler vectorises the loops it can); threads only move the decoder off the UI thread.
- Size: the wasm grew by about 190 KB (4.55 to 4.74 MB single, 4.87 to 5.06 MB threads).

### rc10 round (2026-10-08): quit, help, favorites

- **Ctrl+Q quits** the desktop app (Cmd+Q on a Mac) and Rusty Bucket's app (the host's shutdown); a browser tab leaves the key alone (`Host::can_quit`, `Effect::Quit`, a "Quit" menu entry only where it applies).
- **Help overlay** (`H` or `?`; Esc, `H` or `?` close it; the menu's "Keyboard shortcuts", the About page's button): a themed, scrollable keyboard and mouse reference with Playback, Library, Visualizer, App and Mouse groups and a tips list, at any width including a phone (a finger drags it). The shortcut rows are generated from `SHORTCUTS`, so they cannot drift (a test fails for a shortcut with no words or no row); the library's own keys (matched where they act) are in `help::library_keys()`, and a test presses each one that has a probe.
- **Favorites moved from `H` to `Ctrl+F`** (tooltips, menus, hints, docs and tests follow); search is `/` only.
- **Transport `<< < play > >>`** on every bar (player, library, phone; 44 px touch targets): the outer pair is restart-or-previous (`P`: restarts the item, within 3 s of its start goes to the previous one) and next (`N`; greyed out and inert when nothing follows, `UiModel.can_next`), the inner pair stays the 10 s seeks (`J`/`L`). Shuffle and repeat stay outside. A player window narrower than 680 px keeps the inner three; shuffle and repeat now appear from 880 px.
- **About**: a closing line, "Please provide suggestions, requests & bug reports via X or GitHub!", whose X and GitHub are links (X profile, the GitHub issues) opened like the other links; where the host cannot open links the addresses follow the words as text.

### After rc9.1 (1.0.0 final)

- **Music button in the player bar**: the music-note button (every layout: desktop from 600 px, phone portrait, the Bucket adapter; in the Tab order) now asks for the music library (`Action::ShowMusic`): the Library face, on Albums when it was last on Videos or About, otherwise on the view it was. What is playing keeps playing (navigation never stops playback). Tooltip: "Music library".

- **Echo out on skip** (setting, on by default; Audio panel and menu): skipping a song in the middle (Next, `>>`, choosing another track, a library Play) throws its last 300 ms into a wet feedback echo that fades out over 2 s while the next song rises over about 0.5 s. Songs only, never a natural end, Back, a seek, a pause or a video. The echo is computed once at the skip from a copy of what the sink was given (`rvp_core::echo`, no_std; the player keeps the copy only while the setting is on and a song plays), plus the next 15 ms of the old song faded out so the cut cannot click, and is mixed into the first two seconds of the new item before the limiter: no delay line runs on the incoming song and no latency is added. It is separate from the crossfade (which only joins a natural end; the throw uses its own fixed lengths).

- **The queue is the order Next plays**: the Queue view shows the song playing on top, then what "next" visits, one row each press: the shuffled order, the rest of the list, and with repeat all the songs already played (or, shuffled and on the last song, the round that is already decided). Played songs drop off the top. `Playlist::play_view` is the one definition; a test walks Next against it for every shuffle and repeat setting.
- **Play next stacks**: "Play next" (menus, Ctrl+Enter, the album/artist/playlist button, the queue row) puts a song right behind the current one and behind earlier play-nexts still waiting, so they play in the order added; the block ends when its songs have played, shuffle does not touch it, and "Add to queue" stays at the end (shuffled, an added song lands anywhere behind the block). The toast says "Playing next: <title>".

### Phase A2 of the 2026-10-06 batch

- **Favorites**: a heart on every song and video (rows, posters, the album page's *Favorite* button, the now-playing card, the bar, context menus, key `H`, since rc10 `Ctrl+F`), a Favorites view (key `9`) with Music and Videos
  sections that plays as one queue. Kept by folder name, file name and length (`library/favorites`), so rescans, moved folders and tag edits keep them.
- **About RW** (last in the rail, `F1`): three short paragraphs (DJ Unicorn Tears, Rusty Wave, Rusty Bucket), the version and build commit (`build.rs` of `rvp-app`; `RVP_BUILD_COMMIT` overrides it
  for builds without git), links through the host (`Effect::OpenUrl`) and the licenses.
- **Visualizer cycle**: `Shift+V` (or the button) changes the effect by itself; Settings has the order (in turn or random) and the time (15 s, 30 s, 1, 2, 5, 10 min); it holds still with reduced motion.
- **Edit tags** (right-click, `E`): title, artist, album, album artist, track, disc, year, genre and the cover for a song, or the shared tags of an album. `rvp-tagwrite` writes ID3v2.4, Vorbis comments and
  MP4 `ilst`, safely; see [`host-api.md`](../reference/host-api.md) for what each host does. Multi-select editing is not there (the library has no multi-select); an album's shared fields are the cheap version of it.
- **Tooltips**: every control (rail, buttons, toggles, sliders, rows, dialogs, the tag editor, the audio panel) says what it does and its key, after 450 ms or when the keyboard reaches it, wrapped at 140
  characters, inside the window; Settings has *Show tooltips*. `tips.rs` decides the text for every `LibHit` exhaustively, and a test walks every control of every view.
- Search covers favorites' hearts, videos and edited tags at once (the library is patched the moment a file is written, and the folder is read again to confirm it).

### Desktop CPU while playing, after the audit (2026-10-06)

Desktop 1080p30, 20 s, user time ÷ wall (shared host, ±10 %): H.264 typical 0.93 → 0.87–0.94, stress 1.94 → 1.78–1.98, VP9 0.62 → 0.67–0.69, AV1 0.91 → 0.88–0.91. The decode pool and the H.264 pipeline no longer poll (they park and are woken); what is left is scalar
kernel code (no native SIMD on the desktop build). Posters decode with single-threaded decoders: scanning 12 1080p films went from 182 MB to 56 MB RSS. Details in `docs/audits/audit-1.0.0-rc1.md`.

## 12. Risks and open questions

| # | Risk | Plan |
| --- | --- | --- |
| R1 | `rav1d` 1.1.0 does not compile on `wasm32-unknown-unknown` (libc imports). | **Resolved in M4**: a private `libc` shim module (see `third_party/rav1d/PATCHES.md`) was the only change needed. The vendored copy decodes bit-exact in Node (`cargo xtask wasm-smoke`). Upstreaming the shim is still worthwhile. |
| R2 | Real-time 1080p in single-threaded wasm for H.264/AV1/VP9. | **Resolved in M9**: SIMD128 kernels (single-threaded build: H.264 typical and VP9 at 0% dropped, H.264 25 Mbit/s 5%, AV1 9.6%) and the opt-in threads build (0% dropped on all four, 60 s each). Numbers in the M9 notes. |
| R3 | Rusty Bucket's `wasmi` is an interpreter: video will not be realtime there. | M12: the manifest offers a baseline, an optional SIMD and a threads build, so the OS runs the best one its engine loads (Rusty Bucket's own plan: a compile-ahead engine); tracked in `../rust-os/docs/planning/architecture.md` (D12 compile-ahead engine, D16 threads). |
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
