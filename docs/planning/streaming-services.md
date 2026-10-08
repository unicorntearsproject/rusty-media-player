# Streaming services plan: Spotify, Apple Music, YouTube Music, Amazon Music

[Documentation index](../README.md) · [Plan](PLAN.md) · [1.1 media server](v1.1-media-server.md) · [Host interfaces](../reference/host-api.md)

Status: proposal for decision, 2026-10-05. No code yet. Product name Rusty Wave, codename rvp. This plan follows the rules in
[`CLAUDE.md`](../../CLAUDE.md): clean-room, MIT OR Apache-2.0, standalone first, nothing in core depends on Rusty Bucket, the host-neutral
traits are the source of truth ([`host-api.md`](../reference/host-api.md), rust-os ADR-0023 and ADR-0026). Service facts were checked against the
vendors' own pages on 2026-10-05 (links in section 11). Vendor terms change often, so every milestone that depends on a term re-checks it.

## 1. Summary

**The rule that shapes everything:** all four services deliver DRM-protected audio (Widevine or FairPlay) or no public playback API at
all. Rusty Wave's own pipeline (our demuxers, decoders and `AudioSink`) can never decode their audio. So a streaming service is never "another
file source". It is a **provider** that browses and manages content through the official API, plus an **external player**: the
vendor's own official playback engine (their JavaScript SDK, or a Connect device) driven by us. We stay a controller, a catalog
browser and a queue, and the vendor's engine makes the sound.

| Service | Official route | Feasibility (one line) | Recommendation |
| --- | --- | --- | --- |
| Spotify | Web API + Web Playback SDK (+ Connect control) | **Yes, with limits**: full playback in the PWA and remote control from desktop, for Premium users; public release is capped at 5 users per app unless Spotify grants extended quota (250k MAU business), so ship "bring your own Spotify app". | Build first. |
| Apple Music | Apple Music API + MusicKit JS (+ MusicKit for Android) | **Yes, costs $99/yr**: full playback in a Widevine/FairPlay browser (the PWA); no user cap; no remote-control API, so desktop means a browser helper or the installed PWA. | Build second, if the user pays for the Apple account. |
| YouTube Music | None. YouTube Data API + IFrame player | **Partly, and not "Music"**: only the generic YouTube catalog and a visible video player; no YT Music library or radio; ~100 searches/day per Google project by default; no background or audio-only play. | Build last, optional, labelled "YouTube". |
| Amazon Music | Web API (closed beta) | **No, today**: access only through an Amazon business-development contact, no public SDK. | Leave a stub; watch; ask Amazon. |

Per edition:

| Service | Web / PWA | Desktop (Linux, Windows) | Rusty Bucket |
| --- | --- | --- | --- |
| Spotify | Full playback (Web Playback SDK, Premium) | Remote control of any Connect device (the official Spotify app, a phone, a speaker) and a helper browser tab as "this computer"; Premium | Remote control only, once Rusty Bucket has TLS and a token broker |
| Apple Music | Full playback (MusicKit JS) | Use the installed PWA; helper tab is a stretch goal | Not feasible (no CDM, no remote API) |
| YouTube | Visible IFrame player + Data API | Helper window or the PWA | Not feasible (no browser engine) |
| Amazon Music | Not available | Not available | Not available |

## 2. What the services officially allow

### 2.1 Spotify

- **APIs.** Web API (catalog, search, library, playlists, playback state) and the Web Playback SDK, a JavaScript SDK that makes the page a
  Spotify Connect device and plays audio itself ([Web Playback SDK](https://developer.spotify.com/documentation/web-playback-sdk)).
  The Web API's `/me/player` endpoints control playback on any Connect device the user owns and need Premium
  ([start playback](https://developer.spotify.com/documentation/web-api/reference/start-a-users-playback)).
  Scopes: `streaming`, `user-modify-playback-state`, `user-read-playback-state`, `user-library-read/modify`, `playlist-*`
  ([scopes](https://developer.spotify.com/documentation/web-api/concepts/scopes)).
- **Playback and DRM.** The SDK needs Premium (mobile-only plans are excluded), works in Chrome, Firefox, Safari and Edge on desktop and
  mobile, uses EME, does not expose raw audio, and must not be used commercially without Spotify's written approval. Cross-origin
  iframes need `allow="encrypted-media; autoplay"`.
- **Auth.** Authorization Code with PKCE for browsers and native apps; the implicit grant is deprecated. Redirect URIs must be HTTPS or
  a loopback IP literal (`http://127.0.0.1:port`, never `localhost`); a loopback URI may be registered without a port
  ([authorization](https://developer.spotify.com/documentation/web-api/concepts/authorization),
  [redirect URIs](https://developer.spotify.com/documentation/web-api/concepts/redirect_uri)).
- **2026 changes that matter.** Development Mode needs an app owner with an active Premium subscription, allows 5 allow-listed users
  ([quota modes](https://developer.spotify.com/documentation/web-api/concepts/quota-modes),
  [TechCrunch, 2026-02-06](https://techcrunch.com/2026/02/06/spotify-changes-developer-mode-api-to-require-premium-accounts-limits-test-users/)),
  and since 2026-07-23 allows 25 client IDs per developer account that share one quota
  ([quota update](https://developer.spotify.com/blog/2026-07-23-web-api-quota-updates)). Extended quota (unlimited users, higher rate
  limits) needs a legally registered business, an active launched service, 250,000 monthly active users and a review of about six
  weeks. The February 2026 migration removed batch fetches, browse and artist top-tracks endpoints, merged the library endpoints, moved
  playlist `/tracks` to `/items`, and capped search at 10 results per call
  ([migration guide](https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide)). Audio features and
  recommendations were removed for new apps in November 2024 (TechCrunch above), so there is **no beat or tempo data** from Spotify.
  Refresh tokens reportedly now expire six months after the original authorization, and refreshing does not reset the clock (secondary
  source: [vorplabs](https://vorplabs.com/agent-tools/spotify-api-changes); re-check against Spotify's own pages in S0).
- **Terms.** [Developer Terms](https://developer.spotify.com/terms) and [Developer Policy](https://developer.spotify.com/policy):
  no local caching of Spotify content except temporary metadata and cover art (and Premium-only conditional downloads); no stream
  ripping or anything that eases permanent copies; no reverse engineering; no ML training on Spotify content; do not "segue, mix,
  re-mix or overlap" Spotify content with other audio (no crossfade or gapless blending with local files); do not synchronize recordings
  with visual media; metadata, cover art and previews must carry a link back to Spotify and play only with cover art and metadata shown;
  Spotify marks only per the branding guidelines; streaming apps may not be monetized; users need a working "disconnect", and their
  data is deleted within five days of it; use the official SDKs.

### 2.2 Apple Music

- **APIs.** The [Apple Music API](https://developer.apple.com/documentation/applemusicapi) (catalog, library, playlists,
  recommendations) and MusicKit client libraries for Apple platforms, the web (MusicKit JS v3) and Android
  ([MusicKit](https://developer.apple.com/musickit/)). No API remotely controls another device's player.
- **Playback and DRM.** MusicKit JS plays full songs through EME (FairPlay in Safari, Widevine elsewhere; community reports say v3 is more
  reliable in Chrome and Firefox than v1: [forum](https://developer.apple.com/forums/thread/771759)). The listener needs an Apple Music
  subscription. MusicKit for Android needs the Apple Music app installed for sign-in. Windows and Linux have no official SDK; community
  clients wrap the web player in a Widevine-enabled Electron fork ([Sidra](https://github.com/wimpysworld/sidra)), which on macOS and
  Windows needs castLabs VMP signing ([EVS](https://github.com/castlabs/electron-releases/wiki/EVS): free, signs macOS and Windows
  packages only; the Linux Widevine CDM has no VMP).
- **Account and cost.** Requests need a developer token (a JWT signed with a MusicKit private key, ES256, valid for at most six months,
  with an optional `origin` claim that limits it to your website). Creating the media identifier and key needs the Apple Developer Program,
  Account Holder or Admin role ([help](https://developer.apple.com/help/account/capabilities/create-a-media-identifier-and-private-key/),
  [token docs](https://developer.apple.com/documentation/applemusicapi/getting_keys_and_creating_tokens)): **99 USD per year**, individual
  or organisation ([membership](https://developer.apple.com/programs/whats-included/)). Each listener signs in through MusicKit JS and
  gets a Music User Token. There is no per-app user cap.
- **Terms** ([Developer Program License Agreement](https://developer.apple.com/support/terms/apple-developer-program-license-agreement),
  [App Review 4.5.2 discussion](https://developer.apple.com/forums/thread/681105)): users may not download, upload or modify MusicKit
  content or sync it with other content; content plays only as rendered by MusicKit; full songs must be playable and playback must be
  user-initiated with standard controls; album art and text may not be used apart from playback or playlist management; you may not
  require payment for, or indirectly monetize, access to Apple Music. The agreement text is long and the MusicKit schedule was not
  readable by our fetch tool: S0 re-reads it in full.

### 2.3 YouTube Music

- **No official YouTube Music API exists.** The supported route is the [YouTube Data API v3](https://developers.google.com/youtube/v3/getting-started)
  (search, playlists, video metadata) plus the [IFrame Player API](https://developers.google.com/youtube/iframe_api_reference)
  (play, pause, seek, volume, events). Music-specific features (the YT Music library, albums as products, radio, uploads, Premium
  background play) are not available through either.
- **Playback.** Only the embedded player, at least 200x200 px (480x270 recommended with controls), visible, not obscured, with a
  Referer/identity. The API cannot access audio data, and there is no audio-only mode.
- **Quota and account.** Default 10,000 units a day and a limit of about 100 `search.list` calls a day per Google Cloud project, shared by all users
  of that project ([getting started](https://developers.google.com/youtube/v3/getting-started),
  [quota costs](https://developers.google.com/youtube/v3/determine_quota_cost); the cost page's wording was ambiguous, so S0 measures it);
  more through a quota extension form and compliance audit. User data (playlists, liked videos) needs OAuth with `youtube.readonly`, a
  sensitive scope: public apps need Google's verification, and an unverified app in Testing status gets refresh tokens that expire after
  7 days ([OAuth guide](https://developers.google.com/youtube/v3/guides/auth/installed-apps), community report on the
  [7-day limit](https://dev.to/ko-hi/googles-oauth-testing-mode-expires-refresh-tokens-in-7-days-publish-the-consent-screen-before-24hm)).
  Installed apps use a loopback redirect and PKCE; custom URI schemes are no longer supported
  ([native apps](https://developers.google.com/identity/protocols/oauth2/native-app)).
- **Terms** ([Developer Policies](https://developers.google.com/youtube/terms/developer-policies),
  [policy guide](https://developers.google.com/youtube/terms/developer-policies-guide),
  [minimum functionality](https://developers.google.com/youtube/terms/required-minimum-functionality)): do not separate, isolate or
  modify audio or video; no background (undisplayed) player; no downloads or offline copies outside YouTube Premium; do not block ads or
  overlay the player; refresh or delete cached API data every 30 days; YouTube branding where content is shown. This rules out our
  visualizer, EQ, speed change and gapless mixing for YouTube items.

### 2.4 Amazon Music

- Web API V1 and V2 (catalog, library, playlists, playback queue, Login with Amazon OAuth) are in **closed beta**. Access is "limited to
  already approved developers" and requested through an Amazon business-development representative; V2 uses scopes such as
  `music::catalog`, `music::library:read` and `music::history` ([V2 overview](https://developer.amazon.com/docs/music/API_web_overview_v2.html),
  [program overview](https://developer.amazon.com/docs/music/get_started_program-overview.html)). There is **no public SDK**; the program
  page says to contact the team. An Amazon staff reply in the developer forum says the beta has "a full waiting list" and there is no
  sign-up or newsletter; the thread runs to March 2026 without a public date
  ([forum](https://community.amazondeveloper.com/t/how-can-i-access-the-amazon-music-api/9253)). Integrations must be validated by Amazon
  before launch and follow the Amazon Developer Services Agreement and Music Program Requirements.
- Nothing can be planned beyond a stub and a contact request.

### 2.5 DRM and what it means for our pipeline

| Where | Widevine / FairPlay | Consequence |
| --- | --- | --- |
| Browser (PWA) | Chrome, Edge, Firefox ship Widevine CDM; Safari has FairPlay | The vendor SDK plays inside the browser; audio is **not** reachable from our AudioWorklet (EME output cannot be routed into Web Audio), so no visualizer data, EQ or speed |
| Linux desktop (our native app) | Chrome, Edge and Firefox ship a software (L3) CDM (plain Chromium builds do not); WebKitGTK does not support Widevine (community reports, for example [this write-up](https://github.com/aleksanderpalamar/astra-browser/pull/2); S0 re-checks) | We have no browser engine. A bundled Chromium/Electron with Widevine means castLabs ECS or Google licensing and 100+ MB; rejected for v1 |
| Windows desktop | WebView2 Widevine support is an open feature request ([issue](https://github.com/MicrosoftEdge/WebView2Feedback/issues/4828)) | Same as Linux: do not embed, use the user's browser |
| Rusty Bucket | No CDM, no browser engine (its web browsing is a remote-rendered tab: `../rust-os/docs/planning/web-browsing.md`) | Only API-level remote control is possible |

Decision: **the native desktop app does not embed a browser or a CDM.** Where a service must play in a browser engine, the desktop app
serves a small local "player page" and the user opens it in their normal browser (which already has the CDM). The desktop app then
controls it (Spotify: through Connect, so no extra channel; others: a loopback WebSocket authenticated with a random per-session secret).

## 3. Requirements, costs and limits at a glance

| | Spotify | Apple Music | YouTube | Amazon |
| --- | --- | --- | --- | --- |
| User needs | Premium (not Lite/Mini) | Apple Music subscription | A Google account (Premium only changes YouTube's own ads) | Unknown |
| Developer cost | Free, but the app owner needs Premium | 99 USD per year | Free | Business relationship |
| Approval | None for Development Mode; extended quota needs a company and 250k MAU | None beyond the membership | OAuth verification for public apps; quota audit for more than the default | Invite only |
| Users per app | 5 (Development Mode) | Unlimited | Unlimited but a shared quota | n/a |
| Token lifetime | Access 1 h; refresh up to 6 months | Developer token max 6 months; user token long-lived | Access 1 h; refresh unlimited once verified, 7 days in Testing | n/a |
| Monetization | Not for streaming apps | Not allowed | Ads only with independent value | n/a |
| Caching | Temporary metadata and art only | Not stated for metadata; no content copies | Refresh or delete every 30 days | n/a |
| Branding | Spotify marks, link back to Spotify | Apple Music identity guidelines (check in S0) | YouTube branding, visible player | n/a |

Rusty Wave is free and open source with no monetization, which fits all of these. If that ever changes, three of the four services forbid it.

## 4. Architecture

### 4.1 New pieces, all optional and host-neutral

Nothing here changes the existing traits; each piece is an optional capability, defaulting to `None`, like `NowPlaying` and `Library`.
Streaming is behind a cargo feature (`streaming`, on by default in PWA and desktop builds) and a runtime switch that is off until the
user connects an account. No network traffic happens before that.

```text
rvp-provider            no_std + alloc   item model, Provider trait, capabilities, metadata cache with TTL, queue refs
rvp-provider-spotify    no_std + alloc   Web API client over the Http capability
rvp-provider-apple      no_std + alloc   Apple Music API client
rvp-provider-youtube    no_std + alloc   Data API client
rvp-provider-amazon     no_std + alloc   stub: capabilities say "unavailable" until access exists
rvp-host (additions)    Host::http(), Host::accounts(), Host::external_player()   all Option, default None
```

JSON parsing uses `serde` + `serde_json` with `alloc` (MIT OR Apache-2.0), decided in S1.

### 4.2 The Provider trait (sketch)

```rust
pub struct ProviderId(pub &'static str);        // "spotify", "apple-music", "youtube", "amazon-music"
pub struct ItemRef { pub provider: ProviderId, pub kind: ItemKind, pub uri: String }  // Track, Album, Artist, Playlist, Video
pub struct ItemMeta { pub title: String, pub artist: String, pub album: String, pub duration_us: Option<i64>,
                      pub art_url: Option<String>, pub playable: Playable /* Yes | No(Region|NeedsPremium|Unavailable) */,
                      pub fetched_at_us: i64 }

pub struct ProviderCaps {
    pub search: bool, pub library_read: bool, pub library_write: bool, pub playlists_write: bool,
    pub in_app_playback: bool,        // an ExternalPlayer exists on this host
    pub remote_control: bool,         // Connect-style control of another device
    pub needs_visible_player: bool,   // YouTube
    pub can_seek: bool, pub can_set_rate: bool /* false for all */, pub pcm_visible: bool /* false for all */,
}

pub trait Provider {
    fn id(&self) -> ProviderId;
    fn caps(&self) -> ProviderCaps;
    async fn account(&mut self) -> AccountState;   // SignedOut | NeedsPremium | Ready{name} | Expired | Unavailable(reason)
    async fn search(&mut self, q: &str, kind: ItemKind, page: PageReq) -> Result<Page<ItemMeta>, ProviderError>;
    async fn library(&mut self, kind: LibraryKind, page: PageReq) -> Result<Page<ItemMeta>, ProviderError>;
    async fn children(&mut self, parent: &ItemRef, page: PageReq) -> Result<Page<ItemMeta>, ProviderError>; // album, playlist, artist
    async fn edit_playlist(&mut self, op: PlaylistOp) -> Result<(), ProviderError>;
    fn handoff(&self, item: &ItemRef) -> Handoff;  // Embedded(engine) | Remote(device) | DeepLink(url) | Unplayable(reason)
}
```

Host capabilities the providers use:

```rust
pub trait Http {            // async request/response; the host adds credentials
    async fn send(&mut self, account: ProviderId, req: HttpRequest) -> Result<HttpResponse, HostError>;
}
pub trait Accounts {        // OAuth and token custody live here, not in core
    async fn connect(&mut self, p: ProviderId, scopes: &[&str]) -> Result<AccountInfo, AuthError>;
    async fn disconnect(&mut self, p: ProviderId);               // wipes tokens now
    fn status(&self, p: ProviderId) -> AccountInfo;
}
pub trait ExternalPlayer {  // the vendor's own engine, one per provider
    fn supports(&self, p: ProviderId) -> bool;
    fn load(&mut self, item: &ItemRef, start_us: i64);
    fn command(&mut self, c: TransportCommand);                  // the same enum as NowPlaying
    fn poll_event(&mut self) -> Option<ExternalEvent>;           // State, Position, Ended, Error(NeedsPremium|Region|Autoplay|NoDrm|Quota)
    fn devices(&mut self) -> Vec<ConnectDevice>;                 // remote-control targets, empty if not supported
    fn transfer(&mut self, d: &ConnectDevice);
}
```

**Core never holds a token.** `Http::send` takes the provider id and the host attaches `Authorization` (and Apple's `Music-User-Token`),
refreshes on 401 and retries once. This is the same shape as Rusty Bucket's rule that apps never see secrets (ADR-0009), so the Rusty
Bucket adapter can implement `Http` by calling the kernel's broker. It also keeps tokens out of wasm linear memory in the browser.

### 4.3 Playback: two engines, one queue

`Session` (our pipeline) stays unchanged. A queue entry becomes `Local(TrackId or file)` or `Provider(ItemRef)`. When the queue reaches a
provider item the app switches to **external mode**:

- The local session pauses and releases the audio device. The app calls `ExternalPlayer::load`, and `poll_event` drives the UI position,
  state and end-of-track (advance the queue). One provider track at a time is loaded; we never hand a vendor engine our queue.
- `NowPlaying` (Media Session, MPRIS, SMTC) is fed from the external state, and `TransportCommand`s from outside go to
  `ExternalPlayer::command`. Both already exist; only the source of truth for the position changes.
- Transitions between engines are **not gapless and never crossfaded** (Spotify forbids mixing; the engines are separate). The UI says
  nothing about it beyond a brief gap. Speed and A-B loop are disabled for provider items (`can_set_rate: false`); the UI greys them.
- Volume maps to the SDK's volume (Web Audio cannot touch EME output).
- If the engine reports `NeedsPremium`, `Region`, `NoDrm` (browser without a CDM) or `Autoplay` (needs a tap), the UI shows a plain message and
  skips or waits. A first play always happens inside a user gesture, which also satisfies browser autoplay rules.
- YouTube is the exception that needs a surface: `needs_visible_player` makes the Player face reserve its video area for the IFrame (a DOM
  element laid over the canvas by the web host). If the page is hidden or minimized, playback pauses (policy: no background play).

### 4.4 Provider tracks in the library, queue and visualizer

- **Library face.** A new rail section "Services" with one entry per connected account (Search, Your library, Playlists). Provider results
  are **not** merged into the local library index and never written into `library/index`: the terms allow only temporary caching of
  metadata and art. Global search shows local results and, below, one group per connected provider.
- **Metadata cache.** `rvp-provider::MetaCache`, memory only, per-provider TTL (default 24 h, YouTube hard cap 30 days, never persisted to
  disk). Cover art thumbnails go in the existing byte-budgeted LRU and are not saved under `library/art/`.
- **Queue and saved playlists.** A provider item is saved as `ItemRef` only (provider, kind, URI), with no title or art. After a restart the
  queue resolves them again through the API; if the account is not connected they show as "Sign in to Spotify to play this" and are skipped.
  M3U/M3U8/PLS export leaves provider items out (a comment line says how many). Adding a local file to a Spotify playlist is not offered.
- **Visualizer.** Provider audio is DRM and **never reaches `VisualizerTap`**. For a provider item the visualizer view runs the calm
  "ambient" mode (no audio reactivity, driven by the playback clock) and a small label says why. Spotify no longer offers audio-analysis
  data, so beat-sync is not possible from metadata either. We reject loopback capture (WASAPI loopback, a PulseAudio monitor, tab capture)
  as a workaround: it makes stream ripping easier, and Spotify's policy bars syncing recordings with visual media.
- **Not offered for provider items:** EQ, speed, pitch, A-B loop, frame step, subtitles, our gapless trim, downloads, "show in folder".
- **Rusty Bucket's AI tools** (allow-listed tools, ADR-0005) must not receive provider metadata or audio: Spotify forbids ingesting its content
  into AI models. The `rvp-host-rb` adapter must not expose provider items to them.

### 4.5 OAuth, tokens and storage per host

All flows are Authorization Code with PKCE (S256, state, nonce where applicable). The PKCE and state code is small, `no_std`, and lives
in `rvp-provider` (tested against RFC 7636 vectors) so every host shares it; the host only does the redirect and the storage.

| Host | Flow | Token storage | Caveats |
| --- | --- | --- | --- |
| Web / PWA | Spotify: PKCE in a popup or full-page redirect to the PWA origin (HTTPS in production, `http://127.0.0.1:port` in dev, never `localhost`). Apple: `MusicKit.authorize()` popup; the SDK holds the user token. YouTube: Google's browser token flow (short-lived access token, no refresh token; to verify in S0) | IndexedDB, with the refresh token wrapped by a non-extractable WebCrypto key; access tokens in memory | Any script on the origin can use the key, so XSS is the real threat: strict CSP, no third-party scripts except the vendor SDKs, SDKs loaded in their own iframes or at fixed URLs, `Referrer-Policy` set. Storage is per browser profile and can be cleared |
| Desktop (Linux, Windows) | System browser + loopback listener on `127.0.0.1:<random>`, PKCE. Never an embedded web view for sign-in (Google forbids it, users cannot verify the page) | OS keyring: Secret Service on Linux (Flatpak: the Secret portal or `--talk-name=org.freedesktop.secrets`), Credential Manager on Windows, through the MIT/Apache `keyring` crate. **No keyring: tokens stay in memory for the session** and the user signs in again; no plaintext file | Flatpak needs `--share=network` and the OpenURI portal; the single-instance gap noted in `packaging.md` matters little here because the loopback port is per login |
| Rusty Bucket | No browser. Preferred: sign-in through the remote-rendered browser tab (Phase 6c) landing on a loopback or hosted redirect. Fallback: QR code to the phone, a static HTTPS callback page on our PWA origin shows the one-time code, the user types it in (PKCE keeps the verifier on the device). Google also offers a limited-input device flow whose allowed scopes should include YouTube read scopes (to verify) | The kernel vault (Argon2id + XChaCha20-Poly1305, TPM sealed; ADR-0009, ADR-0032). `Http::send` goes through the secrets broker, which injects `Authorization` and refreshes | Needs Rusty Bucket's TLS and networking (its Phase 6d) and a new broker kind "OAuth account" (token endpoint, client id, scopes, refresh) beyond the current "add a key" broker; that is an ADR for rust-os, not something RVP assumes (ADR-0026). Streaming redaction already hides vault values |

Refresh failure (revoked, six-month expiry for Spotify, 7 days in Google Testing mode) moves the account to `Expired` with a "Sign in
again" row; nothing else breaks. "Disconnect" wipes tokens, the metadata cache and art immediately (Spotify requires deletion within five days).

Client IDs and developer tokens:

- **Spotify:** the user registers their own app in the Spotify dashboard (free; the owner needs Premium; the user is the one allow-listed
  user) and pastes the Client ID into Settings. A guided panel shows the exact redirect URIs to register. We also keep our own Development Mode
  app for testing, within the 5-user cap. Extended quota is out of reach (company, 250k MAU).
- **Apple:** one developer token signed by us with an `origin` claim for the PWA's HTTPS origin, minted by a CI job from a key stored as a CI
  secret and rotated every five months, shipped as a static file next to the PWA. The key never ships in any package. Desktop and Rusty Bucket
  cannot use an origin-bound token, which is why Apple Music is web-only for now.
- **Google:** the user's own Google Cloud project (API key + OAuth client) for power users, or our project once its OAuth consent screen is verified
  and the quota is extended. Until then YouTube is "bring your own key".

### 4.6 Web specifics

- Vendor scripts load from their own CDNs at runtime (Spotify `sdk.scdn.co`, Apple `js-cdn.music.apple.com`, YouTube `youtube.com/iframe_api`) and are
  never bundled, cached by our service worker or modified (their terms and DRM require it). CSP gets `script-src`, `frame-src` and `connect-src` entries
  for them, added only while the account is connected.
- The service worker must not precache them. Streaming is disabled offline with a clear message.
- **COOP/COEP.** `cargo xtask serve` sends `Cross-Origin-Embedder-Policy: require-corp` for the threads build, which blocks third-party iframes and scripts that
  do not send CORP. The threads build is only for 1080p video. Streaming runs on the normal single-threaded build; the page uses the threads
  build only when no provider is active, or `credentialless` COEP proves enough (S0 checks).
- Media Session: the vendor SDKs may set their own Media Session metadata; our `mediasession.js` must win and S3/S5 test that.

## 5. Not recommended or rejected

| Option | Why not |
| --- | --- |
| librespot-style reimplementation of Spotify's protocol, or Spotify Connect receivers built on it | Violates the Developer Terms (reverse engineering, DRM circumvention); the [librespot project](https://github.com/librespot-org/librespot) itself says using it with Spotify is probably forbidden and tolerated at best; Premium-only anyway |
| Ripping or caching streams, yt-dlp, innertube or `ytmusicapi`-style scraping for YouTube Music | YouTube policy bans separating audio, downloading, and background play; no public YT Music API exists |
| Using the token embedded in music.apple.com, or decrypting FairPlay/Widevine streams | Abuse of a first-party credential and DRM circumvention |
| Unofficial Amazon Music APIs or capturing its audio | Terms violation; the official API is just not open yet |
| Widevine key extraction, CDM emulation (including OpenWV-style setups) | DRM circumvention; our own pipeline cannot be given DRM audio by any legitimate means |
| System-audio loopback or tab capture to feed the visualizer or recorder | Makes stream ripping easier (Spotify Terms); Spotify's no-sync-with-visual-media rule; breaks on DRM-protected paths anyway |
| Bundling Chromium/CEF/castLabs Electron in the desktop app for full native playback | Not a ToS problem but heavy (100+ MB, Widevine licensing, per-OS VMP signing) and contrary to the small native app. Revisit only if the helper-tab approach proves unusable |
| Wrapping the vendors' web players (Cider, Sidra style) as our UI | Replaces our UI with theirs and breaks the branding and "standard controls" intent; not our product |
| Merging provider tracks into the persistent local library | Caching limits; also means offline copies of metadata that Spotify and YouTube make us expire |

## 6. Per-edition integration

**Web / PWA (the main target).** Full integration for Spotify (Premium), Apple Music (MusicKit JS, once the Apple account exists) and
YouTube (visible player). It is the only edition with an in-process vendor engine. Honest blockers: needs an HTTPS origin (Apple origin claim,
Spotify redirect, Google verification), browsers without a CDM (some Linux distro Chromium builds, locked-down Firefox) show a "No DRM" state,
iOS Safari requires a tap after transfer, the 5-user cap on Spotify, shared YouTube quota.

**Desktop (Linux, Windows).** Spotify is full-featured as a controller: browse, queue, play on any Connect device, including the official
Spotify app, and "This computer" through the helper tab (the page loads the Web Playback SDK and appears as a Connect device named "Rusty
Wave"; the user keeps that tab open). MPRIS and SMTC mirror the state. Apple Music and YouTube use the installed PWA, because their
playback cannot be driven through an API; a helper window that the desktop app drives over loopback is a stretch goal (Apple's terms ask
for standard visible controls and user-initiated playback, so the helper page shows them). Honest blockers: no embedded engine, the user needs a
browser with a CDM, Spotify Connect polling (`/me/player` has no push channel, so poll every 2 s while playing, extrapolate in between, and
back off on 429), Spotify's quota is shared across the developer account.

**Rusty Bucket.** Spotify remote control is the only feasible integration, and only after Rusty Bucket has TLS, a token broker and a way to sign in. Apple
Music, YouTube and Amazon cannot work (no CDM, no browser engine, no remote API). A metadata-only mode is not worth building: Apple forbids using
art and text apart from playback, and YouTube requires the player. The adapter work is therefore small and late (M8).

## 7. Milestones

Numbering continues after M12 as S0 to S9 so it can be merged into `PLAN.md` section 11 once approved. Estimates are working days of the `coder`
agent plus review; vendor waiting time (approvals, accounts) is extra. Every milestone ends with `cargo xtask check`, `cargo test --workspace`,
the wasm and no_std checks, and a commit and push. **No real-account test runs in CI**: automated tests use fakes (below), and
real-account checks go into `docs/release/release-testing.md` for the user.

| # | Milestone | Done when | Est. |
| --- | --- | --- | --- |
| S0 | **Spike and terms check.** Throwaway pages, one per service, plus a re-read of the vendors' terms | One track plays per service in a Chromium and a Firefox page (Spotify, Apple if the account exists, YouTube); results table appended to this plan, including: COEP behaviour, Spotify end-of-track detection, MusicKit on Linux Chromium, Spotify refresh-token expiry, Google token flow without refresh, YouTube search quota cost, Apple identity guidelines. Go or no-go per service recorded | 2-3 |
| S1 | **Provider core.** `rvp-provider`: item model, `Provider`, `Http`, `Accounts`, `ExternalPlayer`, `MetaCache`, `ProviderId`, queue `ItemRef`, mock host and mock provider; PKCE and state module | RFC 7636 vectors pass; a mock provider and mock external player run through the queue (local, provider, local) with correct now-playing events and no panics; TTL expiry test; `cargo check` for wasm32 and `x86_64-unknown-none` passes; a test proves no token type exists in the crate's public API | 4-5 |
| S2 | **Accounts and storage per host.** Web (popup/redirect, IndexedDB + WebCrypto wrap), desktop (loopback listener, `keyring`, session-only fallback), headless (in-memory); fake identity provider for tests; Settings > Accounts UI (connect, disconnect, status, "bring your own client ID") | Against the fake IdP: PKCE round trip in Playwright and on desktop with a loopback test; tokens survive restart, are gone after Disconnect; expired refresh yields `Expired` and a sign-in row; keyring-absent run keeps tokens in memory only | 5-7 |
| S3 | **Spotify on the web.** Web API client, Web Playback SDK adapter (`web/provider-spotify.js`), Services rail (search, library, playlists, albums), queue integration, now-playing mapping, ambient visualizer, branding and link-back, disconnect wipe | Fake Spotify (JSON server with synthetic data, plus a stub `Spotify.Player`) passes Playwright: search, play, pause, seek, next, end-of-track advance, Premium-required error, 429 back-off, offline message, disconnect clears storage. Manual checklist for a real Premium account added to `docs/release/release-testing.md`. Spotify marks shown per its branding guidelines | 8-12 |
| S4 | **Spotify on desktop.** Connect remote control, device picker, polling with extrapolation, helper tab served on loopback ("This computer" device), MPRIS/SMTC mapping | Fake Connect server drives the desktop host headlessly: transfer, play, pause, seek, device list; helper page registers against the stub SDK; MPRIS state matches `playerctl`; Flatpak manifest updated and verified in the container | 5-6 |
| S5 | **Apple Music on the web** (needs the Apple account). MusicKit JS adapter, Apple Music API client, CI token minting with origin claim and rotation reminder, identity-guideline compliance | Fake Apple API plus stub `MusicKit` pass the same Playwright suite as S3; token job produces a JWT that verifies with the public key and has an `origin` claim and an expiry under six months; manual checklist with a real subscription | 6-8 |
| S6 | **YouTube.** Data API client (own key or verified project), IFrame adapter with a visible player in the Player face, pause on hidden, quota and 30-day cache rules, "Open in YouTube Music" deep links | Stub IFrame API and fake Data API pass the suite; a test proves the cache refuses entries older than 30 days and that hidden pages pause; quota-exhausted state shows a clear message | 5-7 |
| S7 | **Amazon stub and watch.** `rvp-provider-amazon` reports `Unavailable("Amazon Music has no public API yet")`; contact Amazon through the user's channel; re-evaluate each quarter | Services settings list Amazon as "not available yet"; a unit test covers it. Full client only if Amazon grants access (then 5-8 d, not planned) | 0.5 |
| S8 | **Rusty Bucket adapter pieces.** ADR proposal to rust-os (OAuth account broker, network and TLS prerequisites); `rvp-host-rb` `Http`/`Accounts`; Spotify remote control only | Under the Bucket Simulator: sign-in through the chosen flow against the fake IdP, `Http::send` through the broker mock, no token visible to app code. **Blocked** on rust-os Phase 6d and the broker | 4-6 |
| S9 | **Polish and release.** Accounts and error copy in our voice, attribution and branding audit, third-party notices (SDKs loaded at runtime, `keyring`, `serde_json`), `docs/release/packaging.md` and `docs/reference/host-api.md` updated, README, privacy note (no telemetry, accounts opt-in) | Checklists complete; accessibility pass (keyboard and pointer parity for every new view); screenshots; release-testing section for each service | 3-4 |

Total about 45 to 60 working days if all of S0 to S6 and S9 are built; the minimum useful release (S0 to S3, S9) is about 25 to 30.

**Test strategy.** No test hits a vendor. Fakes: a local OAuth server, local REST servers per service serving **synthetic** catalog data (not
real vendor content), stub JavaScript SDKs implementing the used surface of `Spotify.Player`, `MusicKit` and `YT.Player`, and a conformance suite
that every `ExternalPlayer` implementation must pass (load, state transitions, seek, end event, errors). Browser tests use Playwright as now.
The real services get the manual checklist, because they need paid accounts, DRM and the vendors' own login pages.

## 8. Recommended order

1. **S0** first and cheaply: it decides whether COEP, EME and end-of-track detection behave, and refreshes every term.
2. **S1, S2:** the host-neutral foundation, useful for any later service.
3. **S3 Spotify (web)**, then **S4 (desktop)**: best API, no cost, covers both main editions.
4. **S5 Apple Music**, in parallel with S3 or S4 if the user pays for the account (it is the only service that can be public without a company).
5. **S6 YouTube** last of the working ones: lowest fit and the weakest quota.
6. **S7** any time (small). **S8** when Rusty Bucket has networking; do not build it earlier. **S9** at the end of each service wave.

Gating: user testing on real accounts before anything public, per the release-gating rule (no tags, no public repo, no Flathub without an explicit go).

## 9. Risks

| # | Risk | Plan |
| --- | --- | --- |
| SR1 | Vendor terms or quotas change again (Spotify changed Development Mode twice in 2026) | Every service sits behind its own crate and a runtime switch; S0 and S9 re-read terms; a remote kill switch is not added (no phoning home), so ship a point release instead |
| SR2 | Spotify's 5-user cap makes a public "Spotify support" claim impossible | Bring-your-own client ID; say so in the UI and README; never imply official partner status |
| SR3 | "Bring your own app" may be judged by Spotify as circumventing quota limits | S0 sends Spotify a short question through their developer support; if refused, Spotify stays developer-only |
| SR4 | Browsers without a CDM, or DRM changes (for example Widevine VMP rules for desktop apps) | A "No DRM" state with a link to the supported browser list; the desktop helper-tab design avoids shipping a CDM |
| SR5 | End-of-track and gapless behaviour of the vendor SDKs differ and may emit duplicate or missing events | One-track-at-a-time loading, a conformance suite, S0 measures it on real accounts |
| SR6 | COEP/threads conflict blocks vendor iframes | S0 test; fallback is the single-threaded build when a provider is active |
| SR7 | Visualizer expectation: users will want reactive visuals on Spotify tracks | Ambient mode with an honest label; no loopback workaround |
| SR8 | Apple developer token key leak | The key lives only in a CI secret; the token carries an `origin` claim; rotation every five months; revoke through the portal |
| SR9 | Amazon never opens its API | Stub only; no sunk cost |
| SR10 | A token or provider metadata leaks to Rusty Bucket's AI or to logs/telemetry | Core never holds tokens; adapter must not forward provider items to AI tools; no telemetry; redaction already knows vault values |
| SR11 | Clean-room and licensing: vendor SDKs are not redistributable | Load from vendor CDNs at runtime; none vendored; note in `THIRD_PARTY_LICENSES.md`; do not copy API client code from other projects without checking their licenses |

## 10. Questions for the user

1. **Apple Developer Program, 99 USD per year:** do you want to pay it (individual or organisation name) so Apple Music can ship? Without it, S5 is dropped.
2. **Spotify distribution:** is "bring your own Spotify app" (each user registers a free app with Premium, plus our own 5-user test app) acceptable, given that extended quota needs a company with 250k users? And do you confirm Rusty Wave stays free and unmonetized (Spotify, Apple and YouTube all forbid monetizing their playback)?
3. **Test accounts:** which can you test with (Spotify Premium, Apple Music, YouTube, Amazon Music)? Real-account checks are manual and yours.
4. **A public HTTPS origin for the PWA:** which domain? Apple's token origin claim, Spotify's HTTPS redirect, and Google's OAuth verification (privacy policy page) all need a stable one.
5. **YouTube:** is a generic "YouTube" provider (visible player, shared small quota, bring your own Google key) worth building, or skip until a Music API exists?
6. **Amazon:** can you ask Amazon for beta access (they route requests through business development)? Otherwise it stays a stub.

(Decided in this plan unless you object: ambient-only visualizer for provider audio; no embedded Chromium/CEF on desktop; tokens in the OS keyring or in memory only; Rusty Bucket gets Spotify remote control only, later.)

## 11. Sources (all read 2026-10-05)

Spotify: [Web Playback SDK](https://developer.spotify.com/documentation/web-playback-sdk) ·
[quota modes](https://developer.spotify.com/documentation/web-api/concepts/quota-modes) ·
[quota update 2026-07-23](https://developer.spotify.com/blog/2026-07-23-web-api-quota-updates) ·
[February 2026 migration](https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide) ·
[TechCrunch](https://techcrunch.com/2026/02/06/spotify-changes-developer-mode-api-to-require-premium-accounts-limits-test-users/) ·
[Developer Terms](https://developer.spotify.com/terms) · [Developer Policy](https://developer.spotify.com/policy) ·
[authorization](https://developer.spotify.com/documentation/web-api/concepts/authorization) ·
[redirect URIs](https://developer.spotify.com/documentation/web-api/concepts/redirect_uri) ·
[scopes](https://developer.spotify.com/documentation/web-api/concepts/scopes) ·
[start playback](https://developer.spotify.com/documentation/web-api/reference/start-a-users-playback) ·
[vorplabs summary](https://vorplabs.com/agent-tools/spotify-api-changes) (secondary).

Apple: [MusicKit](https://developer.apple.com/musickit/) · [Apple Music API](https://developer.apple.com/documentation/applemusicapi) ·
[developer tokens](https://developer.apple.com/documentation/applemusicapi/getting_keys_and_creating_tokens) ·
[media identifier and key](https://developer.apple.com/help/account/capabilities/create-a-media-identifier-and-private-key/) ·
[membership](https://developer.apple.com/programs/whats-included/) ·
[Developer Program License Agreement](https://developer.apple.com/support/terms/apple-developer-program-license-agreement) ·
[licenses forum thread](https://developer.apple.com/forums/thread/681105) ·
[MusicKit JS browser thread](https://developer.apple.com/forums/thread/771759) · [Sidra](https://github.com/wimpysworld/sidra) ·
[castLabs EVS](https://github.com/castlabs/electron-releases/wiki/EVS) · [castLabs Widevine](https://castlabs.com/security/widevine-certification/).

YouTube / Google: [Data API getting started](https://developers.google.com/youtube/v3/getting-started) ·
[quota costs](https://developers.google.com/youtube/v3/determine_quota_cost) ·
[IFrame Player API](https://developers.google.com/youtube/iframe_api_reference) ·
[Developer Policies](https://developers.google.com/youtube/terms/developer-policies) ·
[policy guide](https://developers.google.com/youtube/terms/developer-policies-guide) ·
[required minimum functionality](https://developers.google.com/youtube/terms/required-minimum-functionality) ·
[installed-app OAuth](https://developers.google.com/youtube/v3/guides/auth/installed-apps) ·
[native app OAuth](https://developers.google.com/identity/protocols/oauth2/native-app) ·
[Testing-mode refresh token expiry](https://dev.to/ko-hi/googles-oauth-testing-mode-expires-refresh-tokens-in-7-days-publish-the-consent-screen-before-24hm) ·
[Macamp analysis of a policy-compliant YouTube provider](https://github.com/holgerkrupp/Macamp/issues/17).

Amazon: [Web API V2](https://developer.amazon.com/docs/music/API_web_overview_v2.html) ·
[Web API V1](https://developer.amazon.com/docs/music/API_web_overview.html) ·
[program overview](https://developer.amazon.com/docs/music/get_started_program-overview.html) ·
[developer forum thread](https://community.amazondeveloper.com/t/how-can-i-access-the-amazon-music-api/9253).

DRM on desktop: [WebView2 Widevine request](https://github.com/MicrosoftEdge/WebView2Feedback/issues/4828) ·
[WebKitGTK DRM limit (community report)](https://github.com/aleksanderpalamar/astra-browser/pull/2) · [librespot disclaimer](https://github.com/librespot-org/librespot).

Rusty Bucket: ADR-0009 (secrets broker), ADR-0032 (vault unlock backends), ADR-0023, ADR-0026, `docs/planning/secrets.md`,
`docs/planning/web-browsing.md` in `../rust-os`.

Findings that came from a summarizing fetch tool and were not independently confirmed are marked "verify" or "check in S0"; S0 exists to close them.
