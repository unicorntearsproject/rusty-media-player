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
| Flatpak | `dist flatpak [--sign]` | `flatpak-builder`, the 25.08 runtime, SDK and `rust-stable` extension | `io.github.idometeor.RustyVideoPlayer-<ver>.flatpak` (with `--sign` also `.flatpakrepo`, `.flatpakref` and the repo in `target/dist/flatpak/repo`) |
| Windows exe + zip | `dist windows` | MSVC or GNU toolchain; on Linux the wine image (podman) | `rvp-<ver>-windows-x64.zip` |
| Windows installer | `dist installer` | Inno Setup 6 (`ISCC.exe`); on Linux wine in the image | `RustyVideoPlayer-<ver>-x64-Setup.exe` |
| PWA | `dist pwa` | `cargo xtask web` prerequisites | `rusty-video-player-web-<ver>.zip` |
| Signed apt repo | `dist apt-repo --sign` | gpg | `target/dist/apt-repo/` |
| Checksums, signatures | `dist checksums [--sign]` | gpg for `--sign` | `SHA256SUMS` (+ `.asc` files); `dist verify` checks them |

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

No secret keys or passwords are in the repository. The public half of the release key is (`packaging/keys/rvp-release.asc`, and the binary
`rvp-release.gpg` that apt and Flatpak import).

**Release key**: "Rusty Video Player Release <noreply@users.noreply.github.com>", ed25519, sign-only (`[SC]`), created 2026-10-05, expires 2028-10-04.

    Fingerprint  E13F F843 723D 5406 8E45  A3FF 54BF 2FA4 0709 3CEE     (key ID 54BF2FA407093CEE)

It is a dedicated key (not the Unicorn Viz one) and mirrors how that one is kept: one `[SC]` key in the maintainer's gpg keyring, no passphrase
(protected by the account and disk, as `unicorn-viz`'s is; add one with `gpg --edit-key E13FF843723D54068E45A3FF54BF2FA407093CEE passwd` and gpg-agent
will ask), the revocation certificate that `gpg` wrote at creation, and the public key committed. Outside the repository, in `~/.local/share/rvp-release/`
(directory 0700, files 0600): `revocation-<fingerprint>.rev` (publish it only to revoke the key) and `rvp-release-secret.asc` (a secret export, for
backup or for CI). Move both to offline storage; never commit them. The repository secret `RELEASE_GPG_PRIVATE_KEY` for CI is not set and nothing
uploads a secret; `release.yml` signs `SHA256SUMS` only if the maintainer adds that secret. To extend the expiry before 2028:
`gpg --quick-set-expire <fingerprint> 2y` and re-export `packaging/keys/*`.

**Signing locally**: `cargo xtask dist <target> --sign` (key from `RVP_GPG_KEY` or `--sign-key`, default the one in `packaging/keys`; its secret half
must be in your keyring, otherwise the command stops at once). With `--sign`:

| Artifact | Signature | Checked with |
| --- | --- | --- |
| `.rpm` | embedded (`rpmsign`), plus `.asc` | `rpm -K` after `rpm --import packaging/keys/rvp-release.asc` |
| `.deb` | detached `.asc`; apt checks the repo instead (below), `dpkg-sig` is not used because apt ignores it | `gpg --verify x.deb.asc x.deb` |
| apt repo (`dist apt-repo`, `target/dist/apt-repo`) | `InRelease` (clearsigned) and `Release.gpg` | `apt-get update` with `signed-by=rvp-release.gpg` |
| AppImage | embedded by `appimagetool --sign` (ELF sections `.sha256_sig`, `.sig_key`), plus `.asc`; the runtime does not check it | `tools/packaging/verify-appimage-sig.py` or `gpg --verify` |
| Flatpak | the commit and the repo summary (`--gpg-sign`), the key in `.flatpakrepo`, `.flatpakref` and the bundle; plus `.asc` of the bundle | `flatpak install` from the remote verifies; `ostree show` lists the signature |
| tarballs, zip, exe, installer, `.flatpakref/.flatpakrepo` | detached `.asc` (from `dist checksums --sign`) | `gpg --verify` |
| `SHA256SUMS` | `SHA256SUMS.asc` | `gpg --verify SHA256SUMS.asc SHA256SUMS`, then `sha256sum -c SHA256SUMS` |

`cargo xtask dist verify` checks everything under `target/dist` against the committed public key in a throwaway keyring and rpm database (so it shows what a
user with only the published key sees). `tools/packaging/verify-linux.sh apt-signed|rpm-signed` do the same in clean Ubuntu and Fedora containers and
check that a tampered repository or rpm is refused. The Flatpak repo and its `.flatpakrepo`/`.flatpakref` use `file://` URLs unless you pass
`--repo-url https://...` (re-run `dist flatpak --sign --repo-url ...` or just edit the `Url=` lines).

Other hooks: `RVP_SIGN_CMD` runs a command per artifact and for `SHA256SUMS` (`{}` is the path) and still works without `--sign`.
`RVP_WINDOWS_SIGN_CMD` is a signtool or `osslsigncode` command line with `$f` for the file; `dist windows` signs `rvp.exe` with it and `dist installer`
passes it to Inno Setup (`/DSign=1 /Srvpsign=...`), which signs the installer and the uninstaller. In CI it is set when the `WINDOWS_CERT_PFX_BASE64` and
`WINDOWS_CERT_PASSWORD` secrets exist. There is no Windows code-signing certificate yet: unsigned Windows files show SmartScreen's "More info > Run
anyway". Flathub signs its own builds. The GPG key does not replace a Windows certificate.

Testing every package by hand: [`release-testing.md`](release-testing.md).

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
| Signing (release key `E13F...3CEE`) | `dist ... --sign` for deb, rpm, AppImage, Flatpak repo and bundle, apt repo, checksums; `dist verify` (19 checks, throwaway keyring); `.deb` and rpm tamper tests; installed from the signed apt repo in a clean Ubuntu 22.04 container (a modified `InRelease` is refused) and the signed rpm in Fedora 44 (`rpm --import`, `rpm -K`, a modified rpm is refused); AppImage embedded signature checked and the AppImage still runs; Flatpak installed `--user` from the signed local repo (`.flatpakrepo`), from the `.flatpakref` and from the bundle, ran under Xvfb at 1.00x; a remote with another key is refused | rpm below 4.14 (no EdDSA), Flathub, signing from CI, a Windows certificate |
| Workflows | `actionlint` clean; never run (they must not be triggered from here) | A real run |
