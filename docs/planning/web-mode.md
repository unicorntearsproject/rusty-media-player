# Web mode: play from a URL (plan, not started)

Back to the [docs index](../README.md). Related: [v1.1 media server](v1.1-media-server.md) (shares its network crates and
the drop-in interface), [streaming services](streaming-services.md) (YouTube, Spotify and the like are out of scope here).

Status: written up 2026-10-11 at the user's request, **not scheduled**. Decisions still open are listed at the end.

## 1. What it is

A third mode next to **Music** and **Video**: **Web**, with a globe icon, to the right of the two existing buttons at
the top of the left-hand menu. In Web mode the user:

1. **Opens a URL** by pasting, typing, or dropping a link, or with Ctrl+L.
2. Rusty Wave works out what is behind it and **plays it**: a media file, a playlist, a stream, or a web page with
   media or a playlist embedded in it. When a page has several, it **picks the primary one** smartly and offers the
   others in a "Also on this page" list.
3. Everything played is **remembered** in a Web list with a **nice name**: the title, the station or show, and the
   site. Each entry has an icon or cover, can be renamed, can be favorited, and can be replayed with one click.

## 2. What a URL can point to

| Kind | Examples | Handling |
| --- | --- | --- |
| A media file | `.mp3 .m4a .flac .ogg .opus .wav .mp4 .m4v .mkv .webm` | Streamed with HTTP range reads and seekable; the existing demuxers and decoders play it. |
| A playlist file | M3U / M3U8 (non-HLS), PLS, XSPF, ASX | The existing `listfile` parser (M3U, PLS), plus XSPF and ASX readers; entries become a queue with their titles. |
| Internet radio | Icecast / SHOUTcast (MP3, AAC, Ogg/Opus) | An endless stream, not seekable; ICY metadata gives the "now playing" song title. |
| Adaptive streams | HLS (`.m3u8` with `#EXT-X-STREAM-INF` / segments), later DASH (`.mpd`) | HLS needs an MPEG-TS demuxer (we have fMP4/MP4 already) and a variant picker. DASH follows if wanted. |
| A web page | Any `text/html` | Read once, with no scripts run, and scanned for media (section 3). |
| Unsupported | DRM (Widevine/FairPlay), pages that build media in JavaScript only, YouTube and other platforms that forbid it | A clear message that says why, per our error rules ("This page's video is DRM-protected; Rusty Wave can't play it"). |

The content type is detected from `Content-Type`, then by sniffing the first bytes, then from the extension. The
server's word is never trusted alone.

## 3. Finding the media in a page, and choosing the primary

**Candidates.** Rusty Wave collects them from the HTML without running any JavaScript:
- `<video>`, `<audio>` and their `<source>` children;
- `og:video`, `og:audio` and `twitter:player:stream`;
- JSON-LD `VideoObject` / `AudioObject` (`contentUrl`);
- `<link rel="alternate">` with a media or playlist type;
- `<a href>` links to media or playlist files;
- `<iframe>` sources on the same site, followed one level deep.

**Scoring the primary.** The highest score wins:
- **Declared intent:** JSON-LD or Open Graph media first, then the first `<video>`/`<audio>` in the main content
  (`<main>`/`<article>`), then the rest.
- **Inside the main content:** being in the main content beats a header, footer, `<aside>` or `<nav>`.
- **Size and length:** the largest declared size or longest duration wins, which helps rank a feature against a trailer
  against a thumbnail clip.
- **Penalties:** for ad or tracking hosts (a small built-in list) and for names like `preview`, `teaser`, `ad`, `promo`,
  `thumb`.
- **Supported first:** formats we can play rank ahead of ones we can't, and within one item the best variant we decode
  wins (resolution capped at the screen, codecs in our order: AV1, HEVC, VP9, H.264).
- **Ties:** document order breaks them.

**When it isn't clear-cut.** If the best score doesn't beat the next one clearly, Rusty Wave still plays the best one
and shows the "Also on this page" list open, so a wrong pick is one click from fixed. If a page has a playlist and
single media, the playlist wins when it contains the single item.

## 4. Remembering what was played (the Web list)

- Stored with the library data (desktop data dir, the browser's IndexedDB, Rusty Bucket's store), keyed by the final
  URL after redirects.
- Each entry keeps the original URL, the type, the play count and the last time played.
- **The nice name** comes from the best source available:
  1. the stream or file tags (ICY `StreamTitle` for the station, ID3 or Vorbis title and artist);
  2. the playlist's `#EXTINF`, `Title` or `<title>`;
  3. the page's `og:title`, falling back to `<title>`, cleaned up (" | Site", " - YouTube"-style suffixes removed);
  4. the file name, prettified.

  The site name (`og:site_name`, or the host) is shown under it. The user can rename an entry, and a rename always wins.
- **The icon or cover** comes from `og:image`, the playlist's or the stream's art, or the site's favicon. It is cached
  small, the same as library thumbnails.
- **The view** shows sections for Favorites, Recent (newest first), and Stations (radio, grouped). It has search,
  rename, remove, favorite, Play next and Add to queue, with the same keys and context menu as the library.
- **Privacy:** the list stays local and never leaves the device. A "Clear Web history" control sits in Settings and in
  the list's menu. Credentials in a URL (`user:pass@`) and tracking parameters (`utm_*`, `fbclid`, ...) are removed
  before saving.

## 5. Where it lives in the code

- **A drop-in** (`rvp-dropin-web`) on the v1.1 drop-in interface, with its own tab view. The Web mode button is that
  drop-in's tab, so core, ui and app don't depend on it.
- **Network:** `rvp-net` (HTTP client: redirects with a cap, TLS via rustls, timeouts, size limits) and `rvp-netsource`
  (a seekable `MediaSource` over HTTP range reads, with read-ahead and reconnect), both planned in v1.1.
- **New pieces:**
  - `rvp-demux` gains MPEG-TS (for HLS);
  - `rvp-hls` handles variants, segments and live reloads;
  - `rvp-html-scan` is a small tolerant tokenizer for candidates only. It is not a DOM and runs no scripts.
  - XSPF and ASX readers are added next to `listfile`.
- **Hosts:**
  - **Desktop:** the full feature.
  - **Rusty Bucket:** the full feature, once its app ABI exposes outbound HTTP.
  - **Web app:** limited by the browser's cross-origin rules. Direct media and playlists from servers that allow it will
    play, but page scanning mostly won't. The web app says so plainly, and the web app is the lowest priority anyway.

## 6. Security

- `http` and `https` only. No `file:`, `data:` or `javascript:`, and no other schemes.
- No private or loopback addresses unless the user typed one: a page can't make Rusty Wave probe the LAN.
- Limits:
  - a page at most 5 MB;
  - a playlist at most `MAX_ENTRIES` entries (as `listfile` already enforces);
  - at most 10 redirects;
  - connect and read timeouts;
  - no cookies and no stored logins in v1.
- No JavaScript runs, ever, and nothing from a page is rendered as HTML.
- An `http:` (non-TLS) URL plays with a small "not secure" note.
- A fuzz target covers the HTML scanner, HLS, XSPF and ASX, as with the existing demuxers. It runs occasionally, never
  on the everyday gate.

## 7. UI

- **Mode bar:** the top of the rail becomes Music | Video | Web. When collapsed, or on narrow rails, it shows icons only
  with tooltips (as the collapsed rail does today).
- **The URL bar:** a large field at the top of the Web view with Open and Paste, and Ctrl+L focuses it. A link dropped
  anywhere in the app opens in Web mode.
- **While a URL is read:** a resolving state shows what was found (for example "Page: 3 videos, 1 playlist"), with a
  cancel button.
- **Playback:** it uses the normal player, with Video or Now Playing as for local media. Radio shows the station and the
  current song from ICY.
- **Help and keys:** both get Ctrl+L and the Web mode key (proposed: the next free digit or `W`; decided at build time).

## 8. Tests

- **Unit:**
  - HTML candidate extraction and scoring against a set of saved real-world-shaped pages, made by us, not copied from
    sites;
  - nice-name and URL clean-up;
  - XSPF, ASX, HLS and ICY parsing.
- **Headless:** a local test HTTP server (on loopback, run inside the test) serves a file, a playlist, a page with
  several candidates, an HLS stream and an ICY stream. Each opens, the expected primary plays, the history entry gets
  the expected name and icon, and seeking works where it should.
- **E2E:** the Web tab, the URL bar, the list (rename, favorite, remove, clear), and the keys.
- **Not in the gate:** real sites. The user tries a short list of their own favorite URLs before release.

## 9. Order and estimates (agent hours; same yardstick as the v1.1 plan)

| # | Step | Agent h |
| --- | --- | --- |
| 0 | Prerequisite from v1.1: `rvp-dropin`, the tabs, `rvp-net`, `rvp-netsource` (if Web mode ships before the media server, this moves here) | (6 to 8) |
| 1 | Web tab, URL bar, resolving state, direct media files over HTTP | 3 to 4 |
| 2 | Playlists: M3U/PLS over HTTP, plus XSPF and ASX | 1.5 to 2 |
| 3 | Internet radio (ICY metadata, endless streams, reconnect) | 2 to 3 |
| 4 | Page scanning and the primary choice, "Also on this page" | 3 to 4 |
| 5 | The Web list: names, icons, favorites, rename, search, clear | 3 to 4 |
| 6 | MPEG-TS demuxer and HLS (variants, live) | 6 to 8 |
| 7 | Web-app host limits, docs, help and keys, fuzz targets | 1.5 to 2 |
| | **Total** (without step 0) | **20 to 27** |

**Human gates** (separate from the estimates): the user's real-URL check, a decision on the open questions below, and
the usual go before tagging.

## 10. Open questions for the user

1. **Release:** in 1.1 alongside the media server (it shares the network crates, so it's cheapest there), or its own
   1.0.x/1.1 step first?
2. **HLS in the first cut?** A lot of web video and radio is HLS. Leaving it out halves step 6's risk but misses many
   sites.
3. **Mode bar with the Server tab:** Music | Video | Web | Server, or should Server stay under Settings?
4. **Should Web history entries also appear in the main Search and in History?**
