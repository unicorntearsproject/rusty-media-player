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

## Saved queue and `stable_ids` (M11)

`Host::stable_ids()` (default `false`) says whether the ids given to `OpenRequest::Id` (file paths) still open the same file after a restart. The
app saves the queue (`library/queue`: names, library track ids, current item, shuffle, repeat) and the position in the current item
(`session/position`, every 5 s while playing and on `save_state`). At the next start, if nothing was opened from outside, the queue comes back and the current
item opens *paused* at that position. With stable ids the file ids are saved too; without them (a browser) only library tracks come back, and they open once
the library folder is listed again. Cover thumbnails are kept in memory under a byte budget (`App::set_thumb_budget`, default 32 MiB, least recently used
first) and read back from `library/art/<id>` when a view needs one.

## Audio settings (crossfade, automatic level)

Nothing new is asked of a host. The app keeps the settings in `Storage` under `settings/audio` (a few `key=value` lines under a version line,
`rvp_core::AudioSettings::to_text`; unknown keys and unparsable values are ignored, numbers are clamped), so a host only has to persist that key like the others (the browser's
`localStorage`, the desktop's data directory). Two things a host can observe:

- With **crossfade** on, the audio of two queue items overlaps in the stream the sink receives (it is the equal-power sum, one stream, no new `open`), and the
  `NowPlaying` metadata changes in the middle of the fade, at the moment the second item is the louder of the two.
- With **auto-level** on, the audio the `AudioSink` and the `VisualizerTap` get has had the loudness gain and the limiter applied (still before volume and mute), and
  the limiter holds back up to about 3 ms of the newest audio unless `queued_frames` is nearly zero.

The library index (`library/index`) is at format version 2: each track also carries its integrated loudness (from tags or measured) and the tags' album figure; version 1 loads without them.

## AppServices (update checks, app-menu entry)

Optional, `Host::app_services() -> Option<&mut dyn AppServices>` (default `None`). Only the desktop host offers it; with `None` the app shows none of it (no menu entries, no
dialogs, nothing stored). The host does the work on its own threads; the app polls `update_state()` each tick: `Idle`, `Checking`, `UpToDate`, `Managed(text)` (updates come from elsewhere,
the text says how), `Available { version, how: Install | Manual { message, url } }`, `Downloading { done, total }`, `Installing`, `Ready { version }`, `Failed(text)`.
Requests: `check_updates`, `install_update`, `cancel_update`, `reset_update`, `restart` (the host quits after its window closes and then starts the new copy). `integration()` is `Unavailable`, `Off` or `On`
and `set_integration(bool)` changes it. `unix_time()` is wall-clock seconds, because the host clock is monotonic. The app keeps what the user chose under `settings/app`
(`auto_check`, `last_check`, `skipped`, `integration`) in `Storage`. `ScriptedServices` in `rvp-host` is the scripted implementation for tests. Details: [`updates.md`](updates.md).

## Rusty Bucket mapping (M12)

`rvp-host-rb` implements these traits on Rusty Bucket's App API (`bucket_v0`, draft v0.3) through the raw bindings of `bucket-v0-sys`. The
mapping table is in the crate's docs and in `../rust-os/docs/developer/app-api.md`; what is specific to it:

- **Capabilities decide which optional interface exists**: `now_playing()` needs `NOW_PLAYING`, `visualizer()` needs `VISUALIZER` (the shell shows
  or hides it at any time, `CAPS_CHANGED`), `library()` needs `LIBRARY`. When the shell starts showing now-playing after an item began, the
  adapter sends the item again.
- **Transport**: a `TRANSPORT` event becomes a `TransportCommand` (microseconds for the seeks, `f32` in the low four bytes for rate and volume).
- **The viz tap** batches the summaries of one tick into one `viz_summary_n`; PCM goes out as it is heard (`viz_block`).
- **`stable_ids()` is true**: `file_id` gives an id that reopens the file after a restart. A file the OS cannot name again gets a handle kept by the
  adapter under a session id (`rb-handle:n`); such an id does not survive a restart (the saved queue then shows it as unavailable).
- **Storage** uses the kv store; keys `library/art/...` are the cache class (the OS may evict them without telling us; the library rebuilds them).
- **Files** are non-blocking: `-BUSY` is `Pending`, `IO_READY` ends the loop's sleep. `file_open_id` and `kv_load` that are `-BUSY` are waited for
  (bounded to 5 s) while other events are kept for the next tick.
- **Power**: the adapter calls `power_inhibit` from the player state (system awake while anything plays, display while a picture does).

Answers from the Rusty Bucket side (v0.3 clarifications, 2026-10-05) and what the adapter does about them; run against the Simulator
(`cargo xtask bucket-e2e`, see `docs/reviews/app-api-v0-review.md`, "M12 vs bucket-sim"):

1. After `library_add_folder` and `FOLDER_ADDED` the OS starts the first walk itself: **the adapter no longer rescans** (it had caused a second walk).
2. `AUDIO_ERROR` closes the stream (`-CLOSED` after it): the adapter closes its handle and plays on silently, telling the user once.
3. A cancelled multi-select pick: handled both ways, unchanged.
4. `OPEN` while running: the app's choice; we replace the queue (as a drop). Files that arrive together in one `events_wait` are one queue.
5. `bucket_save_state` runs in the next `events_wait` after `RELOAD`, re-entrantly, with memory the host adds; the adapter reacts to `RELOAD` by storing its
   state, and `bucket_save_state` only flushes the key-value store and returns 0. `bucket_restore_state` is exported and takes nothing; the new instance restores
   from the store (queue paused at the position). Fresh memory: nothing is kept in it.
6. Threads: a new instance starts at the module's initial stack pointer; the entry switches stacks first (stated in the threads section of the API pages; the limit counts the main thread).
7. `thread_priority(tid, 1)`: still unused.
8. `IO_READY` only means "retry now"; a retry may return `-IO` (a failed fetch): the player shows the error (files) or starts without the value (keys, with a log line).

`restart()` and `exit()` do not return; the adapter's `restart` therefore returns only when the OS refused. The adapter writes trace lines (log level 4: `np.meta`,
`np.playback`, `np.command`, `audio.open`, `audio.paused`, `audio.exit`, `library.*`, `threads:`, `open`), which the Simulator scenarios read.

## First run, default folders and the default media player

Three small additions to the optional capabilities (all with defaults, so a host that has nothing to offer changes nothing):

```rust
trait Library {
    // ...take_listing, connected_roots...
    fn standard_folders(&mut self) -> Vec<StandardFolder> { Vec::new() }   // the system's Music and Videos folders that exist
    fn add_path(&mut self, path: &str) -> bool { false }                    // walk a folder without asking; the listing arrives by take_listing
}
struct StandardFolder { kind: StandardKind /* Music | Videos */, name: String, path: String }

trait AppServices {
    // ...updates, integration...
    fn offers_enabled(&self) -> bool { true }                // false: no first-run pop-ups (tests, kiosks); `RVP_NO_OFFERS=1` on the desktop
    fn default_player(&mut self) -> DefaultPlayer { Unavailable }
    fn set_default_player(&mut self, type_ids: &[String]) -> Result<DefaultOutcome, String> { Err(..) }
}
enum DefaultPlayer { Unavailable, Available { note: String, silent: bool } }   // silent: the app may set it itself (Linux, macOS)
enum DefaultOutcome { Set(usize), UserMustConfirm(String) }                    // Windows: registered, Default apps opened, the user confirms
```

* **First run** (no `settings/setup` value): the app opens on the Library face, and, when the library is empty, asks the host for `standard_folders()` and `add_path`s
  each. Later runs open on the face the last one ended on; a Player face with nothing to show falls back to the Library. A file opened from outside still goes to the
  face that suits it. `settings/setup` holds `face=` and `folders=done`.
* **Standard folders** on the desktop come from the `directories` crate: the XDG user directories (`user-dirs.dirs`) on Linux, Known Folders on Windows, `~/Music` and
  `~/Movies` on macOS. Browsers and Rusty Bucket keep the default (none).
* **Default media player**: the checklist is [`MEDIA_TYPES`](../crates/rvp-host/src/types.rs) (every type the player opens, a test keeps it equal to the desktop file's
  `MimeType` and the Windows installer's extensions); the app passes the ids the user ticked. Linux writes `~/.config/mimeapps.list` (`[Default Applications]`, what
  `xdg-mime default` writes; an AppImage gets its app-menu entry first), Windows registers the per-user associations and opens *Default apps* (Windows does not let a
  program take defaults silently, and the dialog says so), macOS calls `LSSetDefaultRoleHandlerForContentType` for each type's UTI. The first-run offer and "No thanks"
  are remembered in `settings/app` (`default_player=ask|never|done`); closing the offer without answering counts as "no thanks". Settings has the button whenever
  `default_player()` is `Available`.
* **App-menu offer** (AppImage, portable Windows): the same rule: "No thanks", or closing the dialog, is final; Settings has "Add to app menu" and "Remove from app menu".

## Input: paste

`InputEvent::Paste(String)` carries text the user pasted: the browser's `paste` event, Ctrl+V (Cmd+V) on the desktop through the system clipboard. The theme dialog's box
and the library's search box and name prompt take it. Hosts without a clipboard never send it.

## Net (fetching a link)

```rust
trait Net { fn fetch_text(&mut self, url: &str) -> u32; fn poll_fetch(&mut self, id: u32) -> Option<Result<String, String>>; }
fn net(&mut self) -> Option<&mut dyn Net> { None }      // on Host
```

One bounded GET (2 MiB) of a text document, polled. Only the theme dialog uses it, and only when the user presses *Preview* on a link. The desktop answers through the
update client (HTTPS only); a page answers with `fetch` (a cross-origin link the server does not allow fails, and the dialog tells the person to paste the CSS instead);
Rusty Bucket keeps the default and the dialog says to paste.

## Opening links (`Host::opens_links`, `Effect::OpenUrl`)

```rust
fn opens_links(&self) -> bool { false }                 // on Host
Effect::OpenUrl(String)                                  // an `https://` address
```

The About page's two links (rustybucket.ai, DJ Unicorn Tears on X). A host that answers `true` opens the address in the person's browser: a page with `window.open(url, "_blank",
"noopener,noreferrer")`, inside the same event handler (so a pop-up blocker lets it through), the desktop with `xdg-open`, `rundll32 url.dll,FileProtocolHandler` or `open`. Only `https` addresses
without spaces or control characters are opened (the desktop checks again, `rvp_host_desktop::links::is_web_url`). A host that cannot (Rusty Bucket) keeps `false`, and the page shows the
addresses as text. The Licenses button is not a link: it saves `THIRD_PARTY_LICENSES.md` through `Effect::Download`.

## Writing files: the tag editor (`FileWriter`)

```rust
trait FileWriter {
    fn can_write(&mut self, root: &str) -> Result<(), String>;                 // Ok, or the words that tell the user why not
    fn write(&mut self, root: &str, path: &str, data: Vec<u8>) -> u32;         // replace `path` below `root` with `data`; a ticket
    fn poll_write(&mut self, ticket: u32) -> Option<Result<(), String>>;       // the answer, once; after an Err the file is as it was
}
fn file_writer(&mut self) -> Option<&mut dyn FileWriter> { None }               // on Host
Effect::PickCover                                                                // the editor's Replace button; answer with App::cover_picked(name, bytes)
```

Editing tags is the one thing the player does that cannot be undone, so the contract is strict. The app reads the file, edits it with `rvp-tagwrite`, and checks that the edited bytes still
read as the same audio before it asks the host to write; the host replaces the file **whole and safely** or changes nothing:

- **Desktop**: a temporary file beside it, `sync_all`, the old file's permissions, an atomic `rename`, and the folder synced. A path that leaves the folder (`..`, absolute) or a file that is not
  there is refused (nothing is ever created).
- **Browser**: `window.rvpCanWrite` / `rvpWriteFile` / `rvpWritePoll` (`web/main.js`), with the File System Access API and the folder's own handle. `rvpCanWrite` runs inside the click that opens
  the editor and asks for the read-write permission then; a browser without the API, or a folder added through the `webkitdirectory` fallback (no handle), answers with the reason and the editor opens
  read-only. The browser's `createWritable()` writes a swap file that replaces the file only on `close()`.
- **Rusty Bucket**: no `FileWriter` (the app ABI has no writes to library files), so "Edit tags" is not offered at all (`AppModel::tags` is false).

Formats and what is kept: MP3 (ID3v2.4; v2.2/v2.3 tags are upgraded, unknown frames kept, an ID3v1 tag kept in step), FLAC (other blocks kept), Ogg Vorbis and Opus (comment header repaginated, the
audio pages' contents untouched, covers as `METADATA_BLOCK_PICTURE`), MP4/M4A (`ilst` atoms; chunk offsets moved when the movie box grows, a `free` box fills the gap when it shrinks). The audio is
never changed.

Storage keys added: `library/favorites` (hearts, kept by folder name, file name and length, so they survive rescans and moves), and the settings `tooltips`, `viz_cycle`, `viz_random`, `viz_secs` in
`settings/app` (these load on every host, also one without `AppServices`).

## `window.rvp.snapshot()`: the stable subset

The browser page exposes `window.rvp` for tests and tooling (the page's own JavaScript, a Playwright suite, a kiosk wrapper). `window.rvp.snapshot()` returns
a JSON object that has many more fields (geometry of the controls, menu rows, library state, perf counters) which are **for the project's own tests and may
change in any release**. These are the fields to depend on; they keep their names, types and meaning across releases of the same major version, and new
fields are only ever added:

| Field | Type | Meaning |
| --- | --- | --- |
| `version` | string | The app's version, `1.0.0-rc5` (a pre-release keeps its suffix). |
| `ready` | boolean | `true` once the player runs; `false` while the page restarts it after a crash (then `state` is `"recovering"` and the rest is the last known). |
| `state` | string | `idle`, `opening`, `paused`, `buffering`, `playing`, `ended`, `failed`, or `recovering` (the page is restarting the player). |
| `position_us` | integer | Playback position of the current item, microseconds (0 when idle). |
| `duration_us` | integer or `null` | Length of the current item, `null` when unknown (a stream, or nothing loaded). |
| `item` | object or `null` | The current item: `{ "id": <queue item id>, "index": <place in the queue>, "title": <title shown> }`; `null` when nothing is loaded. |
| `error` | string or `null` | Why the current item failed, in words for the user, else `null`. |

`window.rvp.ready` (a boolean property set when the module has loaded) is true before the first `snapshot()` call is possible; wait for it, then poll
`snapshot().ready` if you need to ride out a crash recovery. Times are always microseconds, as everywhere in the core. The same fields, with the same meaning,
are what the Rusty Bucket and desktop hosts' smoke output reports where they report anything.

## Changes

- 2026-10-05: first version (M8).
- 2026-10-05: `Library` capability, effects for folders and playlist files, storage keys (M10).
- 2026-10-05: `Host::stable_ids`, the saved queue and position keys, the thumbnail budget (M11).
- 2026-10-05: audio settings (crossfade, automatic level) under `settings/audio`; library index version 2 (loudness).
- 2026-10-05: the Rusty Bucket mapping and its open questions (M12). No trait changed.
- 2026-10-06: optional `AppServices` capability (update checks, app-menu entry); `settings/app` key.
- 2026-10-06: M12 against the Bucket Simulator: answers to the open questions recorded; the pause fix (`Session` pauses the audio sink with the clock) is verified there.
- 2026-10-06 (Phase A2): `Host::opens_links`, `Effect::OpenUrl`, `Effect::PickCover`, the optional `FileWriter` capability and `Host::file_writer`; the `library/favorites` key; app settings load without `AppServices`.
- 2026-10-06: the stable `window.rvp.snapshot()` subset (`version`, `ready`, `state`, `position_us`, `duration_us`, `item`, `error`) is documented; `version` and `item` are new snapshot fields.
