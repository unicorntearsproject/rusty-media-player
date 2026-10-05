# Host API: now-playing, the visualizer tap and the library

Status: shapes stable since M8 (2026-10-05). They live in `crates/rvp-host/src/media.rs` and are the source of truth;
Rusty Bucket's App API media interfaces are modelled on them (rust-os ADR-0026) and the mapping lives in `rvp-host-rb`.
Nothing here is specific to any one host. Changes are listed at the bottom; the full host trait set is in
[`PLAN.md`](PLAN.md) section 4.

All three are **optional** capabilities of `Host`; a host that has no media shell, no use for audio analysis and no directory
access keeps the default (`None`) and pays nothing.

```rust
trait Host {
    // ...playback traits...
    fn now_playing(&mut self) -> Option<&mut dyn NowPlaying> { None }
    fn visualizer(&mut self) -> Option<&mut dyn VisualizerTap> { None }
    fn library(&mut self) -> Option<&mut dyn Library> { None }          // M10
}
```

## NowPlaying (MPRIS-like)

The player reports what is playing and where; the host mirrors it to its OS or shell (browser: Media Session API;
Linux desktop: MPRIS; Rusty Bucket: the shell's media interface) and hands transport commands back.

```rust
trait NowPlaying {
    fn set_metadata(&mut self, meta: &NowPlayingMeta);   // a different item, or its tags became known
    fn set_playback(&mut self, playback: &Playback);     // state or position changed
    fn poll_command(&mut self) -> Option<TransportCommand>; // next pending command from outside
}

struct NowPlayingMeta { title: String, artist: String, album: String,    // title falls back to the file name
                        art: Option<Art /* encoded JPEG/PNG: mime + bytes */>,
                        duration_us: Option<i64>, has_video: bool }
enum PlayState { Stopped, Playing, Paused }
struct Playback { state: PlayState, position_us: i64, rate: f32,
                  can_next: bool, can_prev: bool, can_seek: bool }
enum TransportCommand { Play, Pause, Toggle, Stop, Next, Prev,
                        SeekTo(i64 /* us */), SeekBy(i64 /* us, negative = back */),
                        SetRate(f32), SetVolume(f32 /* 0..=1 */) }
```

Rules:

- `set_playback` is sent when the state, rate, available commands or duration change, and when the position jumps (a seek,
  a new item) by more than 0.5 s from where the host would extrapolate it (`position + elapsed * rate` while
  `Playing`). It is **not** sent every tick: the host extrapolates (the Media Session API and MPRIS both do).
- `set_metadata` is followed by a `set_playback` for the same item. A host that does not show album art ignores `art`.
- `poll_command` is drained once per tick. Commands that do not apply are ignored (`Pause` while paused). `Stop` pauses
  and goes back to the start. `Next` and `Prev` follow the playlist (`Prev` restarts the item when it has played for more
  than 3 s).
- Positions and durations are microseconds, as everywhere in the core.

Mapping hints: `Playing/Paused/Stopped` = Media Session `playbackState` `playing/paused/none` = MPRIS `PlaybackStatus`
`Playing/Paused/Stopped`; `SeekTo` = Media Session `seekto` = MPRIS `SetPosition`; `SeekBy` = `seekforward/seekbackward`
= MPRIS `Seek`; `can_*` = MPRIS `CanGoNext/CanGoPrevious/CanSeek`; `SetRate/SetVolume` = MPRIS `Rate/Volume`.

## VisualizerTap

The audio pipeline hands the tap what is being **heard** (not what is merely queued): output frames as the device has
played them, with device latency removed, and the core computes the analysis, so every host gets the same numbers. The
core only does the work while the host provides a tap.

```rust
trait VisualizerTap {
    fn push_block(&mut self, block: &VizBlock<'_>);   // PCM that was just heard
    fn push_summary(&mut self, s: &VizSummary);       // zero or more per block
}

struct VizBlock<'a> { pts_us: i64,            // stream time of the first frame
                      sample_rate: u32, channels: u16,
                      samples: &'a [f32] }     // interleaved, before volume and mute (a quiet setting does not blank the picture)

struct VizSummary { pts_us: i64,              // stream time of the start of the analysed hop
                    level: f32, peak: f32,    // 0..=1 (RMS of the hop; loudest channel peak)
                    bands: [f32; 32],         // 32 log-spaced bands from ~30 Hz to ~16 kHz, 0..=1 on a -70..0 dB scale
                    bass: f32, mid: f32, treble: f32,   // means of bands 0..8, 8..24, 24..32
                    onset: bool, onset_strength: f32,   // a hit begins in this hop; strength relative to the adaptive threshold
                    tempo_bpm: f32 }          // 60..200 bpm, 0.0 until a stable estimate exists (about 3 s in)
```

Rules:

- One summary per hop of 512 frames: about 94 a second at 48 kHz. Blocks have whatever size accumulated since the previous
  tick (typically 10 to 20 ms); summaries are independent of the block size.
- Timestamps are stream time and increase within an item; they jump back on a seek or an item change (the analyzer is
  reset then, so tempo starts over).
- Paused playback delivers nothing. A seek clears the analysis state.
- A full-scale sine reads 1.0 in its band. The 32-band layout and the hop are fixed in `rvp-viz`; changing them is a
  breaking change to this document.

## Library (M10)

Directory access for the audio-first view (`crates/rvp-host/src/library.rs`). The host only lists; reading bytes is the ordinary
`Host::open` with an id, so a listing is cheap and the player reads tags and covers itself (`rvp-library`).

```rust
trait Library {
    fn take_listing(&mut self) -> Option<Listing>;   // a finished walk since the last call
    fn connected_roots(&self) -> Vec<String>;        // roots readable right now (a remembered browser folder is not until allowed)
}
struct Listing { root: String /* stable id */, name: String, files: Vec<FileEntry> }
struct FileEntry { id: String /* for OpenRequest::Id, this session */, path: String /* below the root, '/' separated */,
                   size: u64, mtime_ms: i64 }
```

How it is used:

- The player asks for work through its **effects** (`rvp_app::Effect`): `AddFolder` (show a folder picker; in a browser this must happen
  inside the user's input event), `Rescan(root)`, `Forget(root)`, `ImportPlaylist` (show a picker for `.m3u`, `.m3u8` and `.pls`) and
  `Download { name, mime, data }` (give the user an exported playlist). The host does it however it likes and, for a folder, answers with a
  `Listing` through `take_listing`. Files of all kinds may be listed; the library takes audio files (`mp3 flac ogg oga opus wav m4a m4b aac mka`)
  and cover pictures (`cover`, `folder`, `front`, `albumart`, `album` or `art` as `jpg`, `jpeg` or `png`) and ignores the rest.
- A root id must be stable across sessions (the browser uses `dir:<folder name>`), because the index is keyed by root and path. `FileEntry::id`
  only has to work until the next listing of that root.
- A rescan is incremental: files whose path, size and modification time are unchanged are not read again, new and changed files are read,
  files that are gone leave the index. A host with no modification times reports 0 and gets size-only change detection.
- **Persistence** goes through `Storage` under the keys `library/index`, `library/playlists` and `library/art/<16 hex digits>`; an empty value
  deletes a key. Values can be large (a thumbnail is about 62 KB, the index 100 to 200 bytes per track), so a host should not put them in
  something with a small quota; the browser host keeps them in IndexedDB behind a synchronous copy (see `web/library.js`). The format is
  versioned and a blob that does not parse is ignored (the library is rescanned).
- The **built-in visualizer** does not need a `VisualizerTap`: while its view is up the player asks the session for the same analysis
  (`Session::set_viz_capture`), so any host can show it. A host that wants the numbers too still gets them through its tap.

## Changes

- 2026-10-05: first version (M8).
- 2026-10-05: `Library` capability, effects for folders and playlist files, storage keys (M10).
