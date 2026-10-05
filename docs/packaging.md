# Packaging and releasing

Rusty Video Player ships as a native desktop app (Linux and Windows) and as an installable web app (PWA). Everything is built by
`cargo xtask dist <target>` (`cargo xtask dist` lists the targets) and, for releases, by `.github/workflows/release.yml`.

| Format | Built by | Needs | Output (`target/dist/release/`) |
| --- | --- | --- | --- |
| Binary + installed tree | `dist stage` | Rust; `--container` for a portable binary | `target/dist/linux/stage/usr/...` |
| Tarball | `dist tarball` | | `rvp-<ver>-linux-x86_64.tar.gz` (and `rusty-video-player-<ver>-src.tar.gz`) |
| .deb | `dist deb` | `cargo install cargo-deb` | `rusty-video-player_<ver>_amd64.deb` |
| .rpm | `dist rpm` | `cargo install cargo-generate-rpm`, rpm tools | `rusty-video-player-<ver>-1.x86_64.rpm` |
| AppImage | `dist appimage` | `appimagetool` (downloaded to `~/.local/bin` on first use) | `RustyVideoPlayer-<ver>-x86_64.AppImage` |
| Flatpak | `dist flatpak` | `flatpak-builder`, the 25.08 runtime, SDK and `rust-stable` extension | `io.github.idometeor.RustyVideoPlayer-<ver>.flatpak` |
| Windows exe + zip | `dist windows` | MSVC or GNU toolchain; on Linux the wine image (podman) | `rvp-<ver>-windows-x64.zip` |
| Windows installer | `dist installer` | Inno Setup 6 (`ISCC.exe`); on Linux wine in the image | `RustyVideoPlayer-<ver>-x64-Setup.exe` |
| PWA | `dist pwa` | `cargo xtask web` prerequisites | `rusty-video-player-web-<ver>.zip` |
| Checksums, signatures | `dist checksums` | | `SHA256SUMS` (+ whatever `RVP_SIGN_CMD` writes) |

`cargo xtask dist check` validates the metadata without building (desktop file, AppStream, man page, that the media types agree between the
`.desktop` file and the AppStream file, that the Windows installer registers the main extensions). `linux` runs stage to flatpak, `all`
everything. `--version V` stamps another version (a dry run such as `0.0.0-ci1`); `--no-build` reuses what is there.

## App id, names, paths

- App id `io.github.idometeor.RustyVideoPlayer` (desktop file, icon, AppStream, Flatpak, Wayland app id, X11 class, MPRIS name, Windows
  AppUserModelID). Binary `rvp`; packages `rusty-video-player`.
- Data (settings, library index, thumbnails, saved queue, resume positions): `~/.local/share/rvp/` (`%APPDATA%\rvp\data` on Windows; in the Flatpak
  `~/.var/app/<id>/data/rvp/`). `--data-dir` overrides.

## Version stamping

The version is `[workspace.package] version` in `Cargo.toml`. `dist` stamps it into the AppStream release entry, the man page, the `.iss`
(`/DAppVersion`), file names and the PWA service worker (`<version>-<hash of the page's files>`). The release date is the last commit's date
(`SOURCE_DATE_EPOCH` wins). To release: bump the version, commit, tag `v<version>`, push the tag; the workflow refuses a tag that does not
match `Cargo.toml`.

## Shared metadata (`packaging/shared`, `packaging/icons`)

- `io.github.idometeor.RustyVideoPlayer.desktop`: `Exec=rvp %U`, categories, `MimeType=` for every format we play (all of them are in
  shared-mime-info, so no MIME package is shipped). Keep in step with `<provides><mediatype>` in the metainfo (checked by `dist check`) and the
  extension list in `packaging/windows/rvp.iss`.
- `...metainfo.xml.in`: AppStream (screenshots are `docs/screenshots/desktop/*.png` by raw GitHub URL, so they show once the repository is public or the
  images are hosted elsewhere), OARS content rating, branding colours, releases. `@VERSION@` and `@DATE@` are stamped.
- `rvp.1.in`: the man page.
- Icons come from `tools/gen-brand.py` (needs `resvg`, Pillow, fontTools): the SVG sources in `assets/brand`, hicolor PNGs 16 to 512 and
  scalable SVG, `rvp.ico`, `rvp.icns` (for macOS later), the Inno Setup wizard bitmaps, and the PWA icons and favicon in `web/icons`. The
  generated files are committed; packaging never regenerates them.

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

**Flatpak.** `packaging/flatpak/io.github.idometeor.RustyVideoPlayer.yml`: runtime `org.freedesktop.Platform//25.08`, `rust-stable` extension, the crates
vendored offline from `cargo-sources.json` (regenerate with `cargo xtask dist flatpak-sources` after any `Cargo.lock` change; the generator is
`tools/flatpak-cargo-generator.py`, MIT). Permissions: Wayland, fallback X11, PulseAudio (PipeWire's socket), `xdg-music:ro`, `xdg-videos:ro`, and the
MPRIS names `org.mpris.MediaPlayer2.io.github.idometeor.RustyVideoPlayer[.*]`; no GPU, no network, no home access (other places come through the file
chooser portal). The manifest builds a release tag; `dist flatpak` rewrites the source between the `APP-SOURCE` markers to a tarball of the working
tree. For Flathub, copy the manifest and `cargo-sources.json` to the Flathub repository with the tag and a commit.

## Windows

`rvp.exe` is a GUI-subsystem program with the icon, version information and a manifest (per-monitor DPI, UTF-8, long paths) embedded by
`crates/rvp-host-desktop/build.rs`. It attaches to the parent console for `--help` and `--version`.

On Windows (and in CI) `dist windows` uses the host Rust toolchain (MSVC) and `dist installer` runs `ISCC.exe` (`ISCC` env var if not on `PATH`).
On Linux both run in `packaging/windows/Containerfile` (Fedora, MinGW-w64, Wine; `x86_64-pc-windows-gnu`), with Inno Setup 6.7.3 installed once into
`target/dist/wineprefix` (the download is checked against a pinned SHA-256).

`packaging/windows/rvp.iss`: per-user install by default (the wizard can switch to all users), Start menu entry, optional desktop shortcut and
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
Nothing runs on a push or a pull request. A manual run is a dry run: it builds and tests every format under the version you type and keeps the files as
workflow artifacts. A tag run also creates a *draft* release with the files and `SHA256SUMS`. Jobs: Linux (metadata check, Xvfb smoke tests with
`playerctl`, deb, AppImage, tarball), rpm (Fedora container, install test), Flatpak (flatpak-builder action), Windows (exe, zip, installer, silent install
test), PWA (build and Playwright), publish.

## Signing

No keys or secrets are in the repository. Hooks:

- `RVP_WINDOWS_SIGN_CMD`: a signtool or `osslsigncode` command line with `$f` for the file. `dist windows` signs `rvp.exe` with it and `dist installer` passes it to
  Inno Setup (`/DSign=1 /Srvpsign=...`), which signs the installer and the uninstaller. In CI it is set when the `WINDOWS_CERT_PFX_BASE64` and
  `WINDOWS_CERT_PASSWORD` secrets exist.
- `RVP_SIGN_CMD`: run per Linux artifact and for `SHA256SUMS` with `{}` for the path (for example `gpg --batch --armor --detach-sign {}`). In CI the
  checksums are signed when `RELEASE_GPG_PRIVATE_KEY` and `RELEASE_GPG_PASSPHRASE` exist.
- Flathub signs its own builds; a standalone `.flatpak` is unsigned. Unsigned Windows files show SmartScreen's "More info > Run anyway".

## Verification record (2026-10-05, Fedora 44 host)

| Format | What was done | Not verified |
| --- | --- | --- |
| Desktop app | `cargo test -p rvp-host-desktop` (unit tests and four Xvfb smoke tests: plays a fixture at 1.00x clock, screenshot with the video on screen, 201-track library scan with Cyrillic and Japanese names, queue and position after a restart, `playerctl` metadata, pause, seek, play over MPRIS); native Wayland (weston headless) at scale 1 and 2 with full screen; real PipeWire output with the audio clock at 1.00x | A physical Wayland desktop session, other window managers, audible output quality |
| .deb | built from the Ubuntu 22.04 binary; installed with `apt` in a clean `ubuntu:22.04` container (dependencies resolved from the archive); `rvp --version`; Xvfb run played the H.264 fixture (clock 1.001, 150 frames); removed cleanly (`tools/packaging/verify-linux.sh deb`) | Debian, other Ubuntu releases, `lintian` |
| .rpm | requirements from `find-requires`; installed with `dnf` in a clean Fedora 44 container, same smoke run, removed (`verify-linux.sh rpm`) | openSUSE, RHEL clones |
| AppImage | ran on the host (FUSE); ran in a clean Ubuntu 22.04 container with only the runtime libraries (`verify-linux.sh appimage`) | A system without any of those libraries |
| Flatpak | built with `flatpak-builder` (offline crates), installed from the bundle, ran under Xvfb: video at 1.00x, audio device opened through the sandbox, MPRIS visible to host `playerctl` (status, title, pause); then uninstalled | Flathub linter, Wayland inside the sandbox, portals' folder pickers |
| Windows exe | cross-built (`x86_64-pc-windows-gnu`, MinGW-w64); ran under Wine 11 on Xvfb: H.264 video at 1.00x, screenshot correct | Real Windows (WASAPI, SMTC, DPI, SmartScreen), the MSVC build (CI only) |
| Installer | built with Inno Setup 6.7.3 under Wine; silent install: files, Start menu and desktop shortcuts, `OpenWithProgids` and Capabilities registry entries, installed program ran, uninstall removed registry entries and shortcuts | Real Windows' Default apps page, signing, the dialog flow |
| PWA | Playwright (Chromium): valid manifest, every icon exists with the declared size, service worker precache, reload and play a local file with the network off, update flow | Install prompt UI, file handlers and share target on real OSes, Firefox and Safari |
| Workflows | `actionlint` clean; never run (they must not be triggered from here) | A real run |
