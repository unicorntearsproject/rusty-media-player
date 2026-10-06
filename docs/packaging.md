# Packaging and releasing

Rusty Wave ships as a native desktop app (Linux and Windows) and as an installable web app (PWA). Everything is built by
`cargo xtask dist <target>` (`cargo xtask dist` lists the targets) and, for releases, by `.github/workflows/release.yml`.

| Format | Built by | Needs | Output (`target/dist/release/`) |
| --- | --- | --- | --- |
| Binary + installed tree | `dist stage` | Rust; `--container` for a portable binary | `target/dist/linux/stage/usr/...` |
| Tarball | `dist tarball` | | `rusty-wave-<ver>-linux-x86_64.tar.gz` (and `rusty-wave-<ver>-src.tar.gz`) |
| .deb | `dist deb` | `cargo install cargo-deb` | `rusty-wave_<ver>_amd64.deb` |
| .rpm | `dist rpm` | `cargo install cargo-generate-rpm`, rpm tools | `rusty-wave-<ver>-1.x86_64.rpm` |
| AppImage | `dist appimage` | `appimagetool` (downloaded to `~/.local/bin` on first use), `zsyncmake` (`apt install zsync`, `dnf install zsync`) | `rusty-wave-<ver>-x86_64.AppImage` and `rusty-wave-<ver>-x86_64.AppImage.zsync` |
| Flatpak | `dist flatpak [--sign]` | `flatpak-builder`, the 25.08 runtime, SDK and `rust-stable` extension | `io.github.idometeor.RustyWave-<ver>.flatpak` (with `--sign` also `.flatpakrepo`, `.flatpakref` and the repo in `target/dist/flatpak/repo`) |
| Windows exe + zip | `dist windows` | MSVC or GNU toolchain; on Linux the wine image (podman) | `rusty-wave-<ver>-windows-x64.zip` |
| Windows installer | `dist installer` | Inno Setup 6 (`ISCC.exe`); on Linux wine in the image | `rusty-wave-<ver>-x64-Setup.exe` |
| PWA | `dist pwa` | `cargo xtask web` prerequisites | `rusty-wave-web-<ver>.zip` |
| Signed apt repo | `dist apt-repo --sign` | gpg | `target/dist/apt-repo/` |
| Checksums, signatures | `dist checksums [--sign]` | gpg for `--sign` | `SHA256SUMS` (+ `.asc` files); `dist verify` checks them |
| Update manifest | `dist manifest [--base-url U] [--windows] [--macos] [--sign]` | the built files in `target/dist/release` | `rusty-wave-latest.json` (+ `.asc` with `--sign`), for the in-app updater |
| Publish | `dist publish --sign [--windows] [--macos] [--dry-run]` | the signed, verified deb, rpm and AppImage (and `.zsync`) in `target/dist/release` | copies them, their `.asc`, `rusty-wave-<ver>-SHA256SUMS` and `.asc` to `/home/jj/projects/_software-dist/rusty-wave/` and `s3://ut-software-dist/` (bucket root), then the `latest` aliases and the manifest |

`cargo xtask dist check` validates the metadata without building (desktop file, AppStream, man page, that the media types agree between the
`.desktop` file and the AppStream file, that the Windows installer registers the main extensions). `linux` runs stage to flatpak, `all`
everything. `--version V` stamps another version (a dry run such as `0.0.0-ci1`); `--no-build` reuses what is there.

## App id, names, paths

- App id `io.github.idometeor.RustyWave` (desktop file, icon, AppStream, Flatpak, Wayland app id, X11 class, MPRIS name, Windows
  AppUserModelID). Binary `rusty-wave`; packages `rusty-wave`.
- Until 2026-10-05 the app was called Rusty Video Player (binary `rvp`, id `io.github.idometeor.RustyVideoPlayer`); nothing shipped under that name,
  so there is no migration.
- Data (settings, library index, thumbnails, saved queue, resume positions): `~/.local/share/rusty-wave/` (`%APPDATA%\rusty-wave\data` on Windows; in the Flatpak
  `~/.var/app/<id>/data/rusty-wave/`). `--data-dir` overrides.

## Publishing builds

Per the project's distribution rule, each verified build goes to `/home/jj/projects/_software-dist/rusty-wave/` and to the root of `s3://ut-software-dist/`.
The bucket (us-east-1) is publicly readable through its bucket policy, so a file is served at `https://ut-software-dist.s3.amazonaws.com/<key>` (the
constant `DIST_BASE_URL` in `xtask/src/dist/manifest.rs`; the AppImage's update information and the manifest start with it).

`cargo xtask dist publish --sign` first runs `dist verify`, takes `rusty-wave_<ver>_amd64.deb`, `rusty-wave-<ver>-1.x86_64.rpm`, `rusty-wave-<ver>-x86_64.AppImage`
and its `.zsync` (plus `-x64-Setup.exe` and `-windows-x64.zip` with `--windows`, `-macos-universal.dmg` with `--macos`, only when they were built and verified in the same run),
writes and signs `rusty-wave-<ver>-SHA256SUMS` over exactly those files, builds and signs the manifest, and checks both destinations (`ls` and `aws s3api head-object`
per file): if any *versioned* file of this version exists in either, it stops before copying anything. Bump `version` in `Cargo.toml` for the next publish.
`--dry-run` does all checks and copies nothing. Build the files with `dist stage --container`, then `dist deb|rpm|appimage --no-build --sign`, then
`dist checksums --sign` (move old files out of `target/dist/release`, `linux/stage` and `appimage` first: stale files from an earlier name would be packaged or fail `verify`).

**Stable `latest` aliases.** After every versioned file is uploaded and confirmed by `head-object` (size must match), publish uploads byte copies under stable names
(the only objects it ever overwrites; `Cache-Control: public, max-age=300`, versioned files get `immutable`), the manifest last, so a half-failed publish never points
`latest` at a missing file. Each has the `.asc` of its source (a detached signature covers the content, not the name):

| Alias | Is |
| --- | --- |
| `rusty-wave-latest-x86_64.AppImage` (+ `.asc`) | the versioned AppImage |
| `rusty-wave-latest-x86_64.AppImage.zsync` (+ `.asc`) | the versioned `.zsync` |
| `rusty-wave-latest_amd64.deb`, `rusty-wave-latest-1.x86_64.rpm` (+ `.asc`) | the deb, the rpm |
| `rusty-wave-latest-x64-Setup.exe`, `rusty-wave-latest-windows-x64.zip` (+ `.asc`) | with `--windows` |
| `rusty-wave-latest-macos-universal.dmg` (+ `.asc`) | with `--macos` |
| `rusty-wave-latest.json` (+ `.asc`, content type `application/json`) | the update manifest, below |

**AppImage delta updates.** `dist appimage` embeds the update information `zsync|<base>/rusty-wave-latest-x86_64.AppImage.zsync` (appimagetool `-u`; AppImageUpdate
and the in-app updater read it) *before* signing, so the embedded signature and the `.asc` cover the final file. The `.zsync` is made by `zsyncmake -u <base>/rusty-wave-<ver>-x86_64.AppImage`:
its internal URL is the immutable versioned file, so the `latest` alias of the `.zsync` can never fetch a mismatching AppImage. `dist verify` checks that the update
information is embedded and that the `.zsync` header has the versioned URL, the file's length and its SHA-1.

**Update manifest** (`rusty-wave-latest.json`, schema 1; the in-app updater is documented in [`updates.md`](updates.md)). Generated by `dist manifest` (publish does it from the staged
files, signs it with the release key and verifies the signature). Entries exist only for the files of that publish; URLs name the immutable versioned files, except `zsync_url`, which is the
stable alias:

```json
{ "schema": 1, "version": "0.0.3", "released": "2026-10-06", "key_fingerprint": "E13FF843723D54068E45A3FF54BF2FA407093CEE",
  "files": { "linux-appimage": { "arch": "x86_64", "name": "rusty-wave-0.0.3-x86_64.AppImage", "url": "<base>/rusty-wave-0.0.3-x86_64.AppImage",
             "size": 123, "sha256": "<hex>", "signature_url": "<base>/rusty-wave-0.0.3-x86_64.AppImage.asc",
             "zsync_url": "<base>/rusty-wave-latest-x86_64.AppImage.zsync" },
             "linux-deb": { "arch": "amd64", "...": "same keys without zsync_url" }, "linux-rpm": {}, "windows-installer": {}, "windows-portable": {}, "macos-dmg": {} } }
```

`--base-url` (an `https://` URL or `file:///path`) changes `<base>` for `dist manifest` and for the update information `dist appimage` embeds, for local update tests
(see [`release-testing.md`](release-testing.md), "Testing the update path"); `publish` refuses it. The release date is the last commit's date.

## Version stamping

The version is `[workspace.package] version` in `Cargo.toml`. `dist` stamps it into the AppStream release entry, the man page, the `.iss`
(`/DAppVersion`), file names and the PWA service worker (`<version>-<hash of the page's files>`). The release date is the last commit's date
(`SOURCE_DATE_EPOCH` wins). To release: bump the version, commit, tag `v<version>`, push the tag; the workflow refuses a tag that does not
match `Cargo.toml`.

## Shared metadata (`packaging/shared`, `packaging/icons`)

- `io.github.idometeor.RustyWave.desktop`: `Exec=rusty-wave %U`, categories, `MimeType=` for every format we play (all of them are in
  shared-mime-info, so no MIME package is shipped). Keep in step with `<provides><mediatype>` in the metainfo (checked by `dist check`) and the
  extension list in `packaging/windows/rusty-wave.iss`.
- `...metainfo.xml.in`: AppStream (screenshots are `docs/screenshots/desktop/*.png` by raw GitHub URL, so they show once the repository is public or the
  images are hosted elsewhere), OARS content rating, branding colours, releases. `@VERSION@` and `@DATE@` are stamped.
- `rusty-wave.1.in`: the man page.
- Icons come from `tools/gen-brand.py` (needs Pillow and numpy), made only from the official master `assets/brand/rusty-wave-icon-master.png`
  (1254 px RGBA, from Rusty Bucket's icon set; nothing is redrawn): hicolor PNGs 16 to 512, `rusty-wave.ico`, `rusty-wave.icns` (for macOS later),
  the Inno Setup wizard bitmaps, the PWA icons (any, maskable on the dark Unicorn Tears night background, apple touch) and the favicon in
  `web/icons`, and the in-app logo (`crates/rvp-ui/assets/logo-*.rgba`). Lanczos downscaling plus a light unsharp mask; 32 px and below use a tighter
  crop around the triangle (the full art is mush there). There is no vector source, so there is no scalable SVG icon. `docs/screenshots/brand-sizes.png`
  is a contact sheet of every size. The generated files are committed; packaging never regenerates them.

## Linux

**Portable binary.** `cargo xtask dist stage --container` builds in `packaging/linux/Containerfile.build` (Ubuntu 22.04, glibc 2.35, stable Rust) so
the binary runs on that and anything newer (it needs glibc 2.34 symbols at most). Where Docker Hub is not reachable, `RVP_BASE_IMAGE=public.ecr.aws/ubuntu/ubuntu:22.04`.
CI builds on the `ubuntu-22.04` runner. Packages use the `dist` profile (release, but unwinding: a panic on the media-controls or dialog thread must not
take the player down).

**Runtime libraries** (not bundled anywhere except the Flatpak runtime): `libasound.so.2` and `libdbus-1.so.3` (linked), `libxkbcommon-x11` and the X11 and
Wayland client libraries (loaded when the window opens). The deb depends on `libasound2 | libasound2t64, libdbus-1-3, libxkbcommon0, libxkbcommon-x11-0`
and recommends the rest; the rpm requires what rpm's own scan finds in the binary plus `libxkbcommon-x11`. Fonts for CJK come from the system
(`fonts-noto-cjk`, `google-noto-sans-cjk-fonts`, ...).

**.deb / .rpm.** `cargo-deb` and `cargo-generate-rpm` read `[package.metadata.deb]` and `[package.metadata.generate-rpm]` in
`crates/rvp-host-desktop/Cargo.toml` and package the staged tree. The rpm's requirements come from `find-requires`, so build it where rpm tools
exist (CI does it in a Fedora container from the staged binary).

**AppImage.** The staged tree plus `AppRun`; no libraries are bundled (the ones above must exist on the host, as on any desktop). Run with
`--appimage-extract-and-run` where FUSE is missing.

**Flatpak.** `packaging/flatpak/io.github.idometeor.RustyWave.yml`: runtime `org.freedesktop.Platform//25.08`, `rust-stable` extension, the crates
vendored offline from `cargo-sources.json` (regenerate with `cargo xtask dist flatpak-sources` after any `Cargo.lock` change; the generator is
`tools/flatpak-cargo-generator.py`, MIT). Permissions: Wayland, fallback X11, PulseAudio (PipeWire's socket), `xdg-music:ro`, `xdg-videos:ro`, and the
MPRIS names `org.mpris.MediaPlayer2.io.github.idometeor.RustyWave[.*]`; no GPU, no network, no home access (other places come through the file
chooser portal). The manifest builds a release tag; `dist flatpak` rewrites the source between the `APP-SOURCE` markers to a tarball of the working
tree. For Flathub, copy the manifest and `cargo-sources.json` to the Flathub repository with the tag and a commit.

## Windows

`rusty-wave.exe` is a GUI-subsystem program with the icon, version information and a manifest (per-monitor DPI, UTF-8, long paths) embedded by
`crates/rvp-host-desktop/build.rs`. It attaches to the parent console for `--help` and `--version`.

On Windows (and in CI) `dist windows` uses the host Rust toolchain (MSVC) and `dist installer` runs `ISCC.exe` (`ISCC` env var if not on `PATH`).
On Linux both run in `packaging/windows/Containerfile` (Fedora, MinGW-w64, Wine; `x86_64-pc-windows-gnu`), with Inno Setup 6.7.3 installed once into
`target/dist/wineprefix` (the download is checked against a pinned SHA-256).

`packaging/windows/rusty-wave.iss`: per-user install by default (the wizard can switch to all users), Start menu entry, optional desktop shortcut and
PATH entry, licence page, wizard bitmaps from the brand script, uninstaller (which also asks whether to delete settings and the library index).
File associations are *registered*, not forced (Windows 10 and 11 reserve the default for the user): the program appears under "Open with" and in
Default apps for mp4, m4v, mkv, webm, mka, mp3, flac, ogg, oga, opus, wav, m4a, m4b, aac, m3u, m3u8 and pls.

## Web app (PWA)

`cargo xtask web` produces `target/web` with `manifest.webmanifest` (name, icons incl. maskable, `display: standalone`, file handlers, a share target,
a shortcut), `sw.js` and `pwa.js`. The service worker precaches the page, the wasm and the icons in a cache named by version, so the app starts
offline; a changed page is a new worker that waits and shows an "Update available: reload" button; "Install app" appears where the browser offers
installing; files opened with the installed app or shared to it are opened in the player. Host it over HTTPS (or on localhost). For the threaded
decoder the server also needs `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp` (`cargo xtask serve` sends them);
without them the page runs the single-threaded build. `tests/e2e/pwa.spec.js` checks the manifest and icons, the precache, playing a local file with the
network off, and the update flow.

## CI

`.github/workflows/release.yml` runs **only** on a `v*` tag or by hand (`workflow_dispatch`); `check.yml` (tests, clippy, `cargo xtask check`) only by hand.
Nothing runs on a push or a pull request, and nothing on a schedule (`dist check` fails if a workflow has a `schedule:` trigger). CI only runs on changes: the first job of
both workflows, `gate` (`tools/ci/gate.sh`, needs `actions: read`), skips every other job when the same workflow already succeeded on this commit (manual runs) or, for a tag
run, when that tag's release already has assets; a skipped workflow is green. The `force` input of a manual run overrides the gate. A manual run is a dry run: it builds and tests every format under the version you type and keeps the files as
workflow artifacts. A tag run also creates a *draft* release with the files and `SHA256SUMS`. Jobs: Linux (metadata check, Xvfb smoke tests with
`playerctl`, deb, AppImage, tarball), rpm (Fedora container, install test), Flatpak (flatpak-builder action), Windows (exe, zip, installer, silent install
test), PWA (build and Playwright), publish.

## Signing

No secret keys or passwords are in the repository. The public half of the release key is (`packaging/keys/rusty-wave-release.asc`, and the binary
`rusty-wave-release.gpg` that apt and Flatpak import).

**Release key**: "Rusty Wave Release <noreply@users.noreply.github.com>" (primary user ID; the key was created as "Rusty Video Player Release", which stays on the key as a
second user ID), ed25519, sign-only (`[SC]`), created 2026-10-05, expires 2028-10-04.

    Fingerprint  E13F F843 723D 5406 8E45  A3FF 54BF 2FA4 0709 3CEE     (key ID 54BF2FA407093CEE)

It is a dedicated key (not the Unicorn Viz one) and mirrors how that one is kept: one `[SC]` key in the maintainer's gpg keyring, no passphrase
(protected by the account and disk, as `unicorn-viz`'s is; add one with `gpg --edit-key E13FF843723D54068E45A3FF54BF2FA407093CEE passwd` and gpg-agent
will ask), the revocation certificate that `gpg` wrote at creation, and the public key committed. Outside the repository, in `~/.local/share/rusty-wave-release/`
(directory 0700, files 0600): `revocation-<fingerprint>.rev` (publish it only to revoke the key) and `rusty-wave-release-secret.asc` (a secret export, for
backup or for CI). Move both to offline storage; never commit them. The repository secret `RELEASE_GPG_PRIVATE_KEY` for CI is not set and nothing
uploads a secret; `release.yml` signs `SHA256SUMS` only if the maintainer adds that secret. To extend the expiry before 2028:
`gpg --quick-set-expire <fingerprint> 2y` and re-export `packaging/keys/*`.

**Signing locally**: `cargo xtask dist <target> --sign` (key from `RVP_GPG_KEY` or `--sign-key`, default the one in `packaging/keys`; its secret half
must be in your keyring, otherwise the command stops at once). With `--sign`:

| Artifact | Signature | Checked with |
| --- | --- | --- |
| `.rpm` | embedded (`rpmsign`), plus `.asc` | `rpm -K` after `rpm --import packaging/keys/rusty-wave-release.asc` |
| `.deb` | detached `.asc`; apt checks the repo instead (below), `dpkg-sig` is not used because apt ignores it | `gpg --verify x.deb.asc x.deb` |
| apt repo (`dist apt-repo`, `target/dist/apt-repo`) | `InRelease` (clearsigned) and `Release.gpg` | `apt-get update` with `signed-by=rusty-wave-release.gpg` |
| AppImage | embedded by `appimagetool --sign` (ELF sections `.sha256_sig`, `.sig_key`; made after the update information is embedded), plus `.asc` of the final file; the runtime does not check it | `tools/packaging/verify-appimage-sig.py` or `gpg --verify` |
| `.zsync` | detached `.asc` (from `dist checksums --sign`); also covered by `SHA256SUMS` | `gpg --verify`; `dist verify` also checks its header against the AppImage |
| `rusty-wave-latest.json` | detached `.asc` by the release key (`dist manifest --sign`, `dist publish`); the manifest names the key fingerprint and every file's SHA-256 | `gpg --verify rusty-wave-latest.json.asc rusty-wave-latest.json` |
| `latest` aliases | copies of the versioned files with copies of their `.asc` | `gpg --verify` works on either name |
| Flatpak | the commit and the repo summary (`--gpg-sign`), the key in `.flatpakrepo`, `.flatpakref` and the bundle; plus `.asc` of the bundle | `flatpak install` from the remote verifies; `ostree show` lists the signature |
| tarballs, zip, exe, installer, `.flatpakref/.flatpakrepo` | detached `.asc` (from `dist checksums --sign`) | `gpg --verify` |
| `SHA256SUMS` | `SHA256SUMS.asc` | `gpg --verify SHA256SUMS.asc SHA256SUMS`, then `sha256sum -c SHA256SUMS` |

`cargo xtask dist verify` checks everything under `target/dist` against the committed public key in a throwaway keyring and rpm database (so it shows what a
user with only the published key sees). `tools/packaging/verify-linux.sh apt-signed|rpm-signed` do the same in clean Ubuntu and Fedora containers and
check that a tampered repository or rpm is refused. The Flatpak repo and its `.flatpakrepo`/`.flatpakref` use `file://` URLs unless you pass
`--repo-url https://...` (re-run `dist flatpak --sign --repo-url ...` or just edit the `Url=` lines).

Other hooks: `RVP_SIGN_CMD` runs a command per artifact and for `SHA256SUMS` (`{}` is the path) and still works without `--sign`.
`RVP_WINDOWS_SIGN_CMD` is a signtool or `osslsigncode` command line with `$f` for the file; `dist windows` signs `rusty-wave.exe` with it and `dist installer`
passes it to Inno Setup (`/DSign=1 /Srvpsign=...`), which signs the installer and the uninstaller. In CI it is set when the `WINDOWS_CERT_PFX_BASE64` and
`WINDOWS_CERT_PASSWORD` secrets exist. There is no Windows code-signing certificate yet: unsigned Windows files show SmartScreen's "More info > Run
anyway". Flathub signs its own builds. The GPG key does not replace a Windows certificate.

Testing every package by hand: [`release-testing.md`](release-testing.md).

## Verification record (2026-10-05, Fedora 44 host)

| Format | What was done | Not verified |
| --- | --- | --- |
| Desktop app | `cargo test -p rvp-host-desktop` (unit tests and four Xvfb smoke tests: plays a fixture at 1.00x clock, screenshot with the video on screen, 201-track library scan with Cyrillic and Japanese names, queue and position after a restart, `playerctl` metadata, pause, seek, play over MPRIS); native Wayland (weston headless) at scale 1 and 2 with full screen; real PipeWire output with the audio clock at 1.00x | A physical Wayland desktop session, other window managers, audible output quality |
| .deb | built from the Ubuntu 22.04 binary; installed with `apt` in a clean `ubuntu:22.04` container (dependencies resolved from the archive); `rusty-wave --version`; Xvfb run played the H.264 fixture (clock 1.001, 150 frames); removed cleanly (`tools/packaging/verify-linux.sh deb`) | Debian, other Ubuntu releases, `lintian` |
| .rpm | requirements from `find-requires`; installed with `dnf` in a clean Fedora 44 container, same smoke run, removed (`verify-linux.sh rpm`) | openSUSE, RHEL clones |
| AppImage | ran on the host (FUSE); ran in a clean Ubuntu 22.04 container with only the runtime libraries (`verify-linux.sh appimage`) | A system without any of those libraries |
| Flatpak | built with `flatpak-builder` (offline crates), installed from the bundle, ran under Xvfb: video at 1.00x, audio device opened through the sandbox, MPRIS visible to host `playerctl` (status, title, pause); then uninstalled | Flathub linter, Wayland inside the sandbox, portals' folder pickers |
| Windows exe | cross-built (`x86_64-pc-windows-gnu`, MinGW-w64); ran under Wine 11 on Xvfb: H.264 video at 1.00x, screenshot correct | Real Windows (WASAPI, SMTC, DPI, SmartScreen), the MSVC build (CI only) |
| Installer | built with Inno Setup 6.7.3 under Wine; silent install: files, Start menu and desktop shortcuts, `OpenWithProgids` and Capabilities registry entries, installed program ran, uninstall removed registry entries and shortcuts | Real Windows' Default apps page, signing, the dialog flow |
| PWA | Playwright (Chromium): valid manifest, every icon exists with the declared size, service worker precache, reload and play a local file with the network off, update flow | Install prompt UI, file handlers and share target on real OSes, Firefox and Safari |
| Signing (release key `E13F...3CEE`) | `dist ... --sign` for deb, rpm, AppImage, Flatpak repo and bundle, apt repo, checksums; `dist verify` (19 checks, throwaway keyring); `.deb` and rpm tamper tests; installed from the signed apt repo in a clean Ubuntu 22.04 container (a modified `InRelease` is refused) and the signed rpm in Fedora 44 (`rpm --import`, `rpm -K`, a modified rpm is refused); AppImage embedded signature checked and the AppImage still runs; Flatpak installed `--user` from the signed local repo (`.flatpakrepo`), from the `.flatpakref` and from the bundle, ran under Xvfb at 1.00x; a remote with another key is refused | rpm below 4.14 (no EdDSA), Flathub, signing from CI, a Windows certificate |
| Workflows | `actionlint` clean; never run (they must not be triggered from here) | A real run |

**0.0.2 (2026-10-05, includes crossfade and auto-level).** `cargo test --workspace` (467 passed, 5 ignored), clippy `-D warnings`, `cargo xtask check`, `cargo xtask e2e` and
`e2e --threads` (54 passed each, 6 skipped screenshot specs) all clean. Built the deb, rpm and AppImage from the container binary and signed them; `dist verify` 12 ok. The AppImage ran
under `xvfb-run` (display variables cleared): state Playing, clock 1.0008, 150 video frames. The Windows exe, zip and Setup.exe were built in the wine image; under Wine in Xvfb the portable exe and
the silently installed program both played the H.264 fixture at 1.00x (150 frames) and the uninstaller removed the program. Published with `dist publish --sign --windows` (12 files, local folder and S3).
