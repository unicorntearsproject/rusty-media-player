# Patches to souvlaki 0.8.3

Upstream: https://github.com/Sinono3/souvlaki (MIT, see `LICENSE`). Vendored for rusty-video-player (docs/PLAN.md M11) because the
D-Bus backend has a latency bug that `playerctl` shows: its service loop alternates `recv_timeout(10 ms)` on the channel of
state changes with `Connection::process(1000 ms)`, so a `set_playback` or `set_metadata` that arrives while the loop sits in
`process` is only applied (and announced) when the next D-Bus message arrives or a second has passed. A client that reads
`PlaybackStatus` right after a `Pause` therefore sees the status from before it.

1. `src/platform/mpris/dbus/controls.rs`: `process` waits 20 ms instead of 1000 ms, so queued changes are applied within
   about 20 ms.
2. Same file: the method-call handler ignores the result of `handle_message` instead of `unwrap`ping it (an unhandled message
   would panic the service thread, which aborts the whole process in our release profile).
3. `Cargo.toml`: the dev-dependencies and the examples were dropped. Everything else is unchanged.

Not used by us, kept so the crate stays whole: the macOS and `zbus` backends.
