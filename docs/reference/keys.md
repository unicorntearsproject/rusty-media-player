# Keys and mouse

[Documentation index](../README.md) · [Developing](development.md) · [Host interfaces](host-api.md)

Press `H` or `?` in the app for this list (it is built from the real keymap, so it is always right); `Esc`, `H` or `?` closes it.

Browser player: drop a file or a folder on the page or press `O`. Two faces, switched with `B` (or the music-note button in the player bar, or the switch in the
rail): the **Player** for video and the **Library** for music. A song opened from outside goes to the Library, a video to the Player.

Transport: `<<` (`P`) restarts the item, and again within 3 s of its start goes to the previous one; `<` and `>` (`J`/`L`) seek 10 s; `>>` (`N`) goes to the next item.

Player keys: `Space`/`K` play, arrows seek 5 s (Shift 30 s), `J`/`L` 10 s, `Up`/`Down` volume, `M` mute, `F` fullscreen, `[`/`]` speed, `\` normal
speed, `Home`/`End`, `S`/`A` subtitle and audio track, `.`/`,` frame step, `I` A-B loop, `N`/`P` next and previous, `R` repeat, `Z` shuffle, `Q`
playlist, Page Up/Down chapters, `U` audio settings, `Ctrl+F` heart (favorite), `Ctrl+Q` quit (the desktop app and Rusty Bucket; `Cmd+Q` on a Mac), `H` or `?` the keyboard and mouse reference; right-click for a menu with all of them. Open several files to make a playlist; they play gaplessly.
The Audio settings (`U`, or right-click, Audio effects) turn on a **crossfade** between songs (2 to 10 s, never for video or between tracks of a gapless album) and an
**automatic level** that brings every song to one loudness (-23 to -10 LUFS; from ReplayGain or R128 tags, or measured; by track or by album).

Library keys: `1` Albums, `2` Artists, `3` Tracks, `4` Playlists, `5` Queue, `6` Now playing, `7` or `V` the visualizer, `8` Videos, `9` Favorites, `0` History, `F1` About, `/` search (type;
`Esc` clears and goes back). Plain arrows, `Home`/`End`, `PageUp`/`PageDown` move through the list or grid, `Enter` plays from the selected row (or
opens an album, artist or playlist), `Shift+Enter` adds to the queue, `Ctrl+Enter` plays next, `Delete` removes from the queue or a playlist,
`Alt+Up`/`Alt+Down` move an item, `Backspace` or `Esc` go back, `Tab` walks rail, content and bar, the menu key or `Shift+F10` opens the context menu
of the selection. `Ctrl+Left`/`Ctrl+Right` seek and `Ctrl+Up`/`Ctrl+Down` change the volume (`J`/`L` and `M` still work). In the visualizer:
`Left`/`Right` change the effect, `C` the colours, `T` the title, `Enter` turns it on or off, `Shift+V` changes the effect by itself. With the pointer: click a card or row (double click plays),
the play button on a card, right-click anything for its menu, drag queue rows to reorder, the mouse's back button goes back, the wheel scrolls.
Add a folder with the button in the rail (`Add folder`), drop one on the page, or open playlist files (`.m3u`, `.m3u8`, `.pls`) to import them.
See [`host-api.md`](host-api.md) for the now-playing, visualizer and library interfaces. Screenshots are in [`docs/screenshots`](../screenshots).

