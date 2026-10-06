# Updates and app-menu integration (desktop)

The desktop app (`rusty-wave`) can look for a newer version, install it itself where that is safe, and add itself to the desktop's application
menu when it runs as an AppImage or as the portable Windows program. The web app (PWA) has its own service-worker update flow and none of this.
The code is in `crates/rvp-update` (everything that touches the network, files and signatures), `crates/rvp-host-desktop/src/services.rs` (the host side) and
`crates/rvp-app/src/services.rs` (what the dialogs say and what is remembered). Hosts without these services (browser, Rusty Bucket, headless) show none of it.

## What the user sees

Right-click menu (both faces): **Check for updates...**, and, for AppImage and portable Windows, **Add to the app menu** / **Remove from the app menu**.
A themed dialog shows the state: checking, "Rusty Wave 0.0.3 is available" (Update now / Later / Skip this version), a progress bar, "Restart to finish
updating" (Restart now / Later), or a plain-language failure. The same dialog has two switches: **Check for updates automatically** and **Show in the app menu**.

| Installed as | "Update now" does |
| --- | --- |
| AppImage (`$APPIMAGE` set) | downloads the new AppImage next to the old one, checks size, SHA-256 and signature, then replaces the file in one `rename` (keeps its permissions), and offers **Restart now** |
| Windows, installed with Setup.exe (`unins000.exe` beside the program) | downloads Setup.exe to `%TEMP%`, checks it, runs it with `/SILENT /CLOSEAPPLICATIONS /RESTARTAPPLICATIONS` after the window has closed |
| Windows portable (a bare `rusty-wave.exe`) | downloads the portable zip, checks it, extracts `rusty-wave.exe` as `rusty-wave.exe.new`, and on restart renames the running exe to `.old`, moves the new one in and starts it; the next start deletes the `.old` file |
| .deb, .rpm (the program is under `/usr/bin`) | nothing: "Update through your package manager (apt, dnf or your software centre)" |
| Flatpak | nothing, and no network is used (the sandbox has none): "Update through Flatpak or your software store" |
| macOS app | the dialog names the new version and says to download it; no self-update |
| anything else (tarball, a build from source) | "Download the new version from the Rusty Wave site" |

"Restart now" does not start the new copy at once: the window closes first (state is saved, the single-instance lock is released) and only then is the new
program started, so the two never run side by side.

## The automatic check

Off by default, switched on in the dialog. When on, the app asks at most once a day (it keeps the time of the last check; a clock set far back does not
silence it forever), only at startup, and only speaks up if something newer than the running version exists that the user has not skipped. A failed
check (offline) is silent. "Later" keeps quiet until the next run; "Skip this version" is remembered until a newer version appears.

Remembered in `settings/app` under the data directory (`~/.local/share/rusty-wave/` and equivalents): `auto_check`, `last_check` (Unix seconds), `skipped`
(version), `integration` (`ask`, `never` or `done`).

**Privacy.** A check is one HTTPS GET of `rusty-wave-latest.json` and one of its `.asc`, with `User-Agent: RustyWave/<version>` and nothing else (no cookie,
no identifier, no telemetry). Nothing is sent unless the user chose to check or switched the automatic check on.

## What is trusted

The release key is compiled into the program (`packaging/keys/rusty-wave-release.asc`, fingerprint `E13F F843 723D 5406 8E45  A3FF 54BF 2FA4 0709 3CEE`).
Verification is pure Rust (`pgp`, rPGP) with no gpg binary needed.

1. The manifest (`rusty-wave-latest.json`, written by `cargo xtask dist manifest`, see [`packaging.md`](packaging.md)) is only read after its detached
   signature `rusty-wave-latest.json.asc` verifies with the release key; it must name that key's fingerprint; a newer `schema` than the program knows is
   refused; versions are compared as semantic versions, so an older (replayed) manifest never offers a downgrade and `0.0.3-ci1` is older than `0.0.3`.
2. A file is only used after its size and SHA-256 match the manifest *and* its own detached `.asc` verifies with the release key. The signature must carry
   the key's fingerprint, must not predate the key or postdate its expiry, and the key must not be revoked.
3. A manifest's URLs must be `https://` (a plain-`http://` redirect is refused; `file://` URLs and paths exist for tests). File names must be plain names.
4. Anything that fails leaves the installed program untouched and removes the download.

A compromised mirror can withhold updates but cannot make a program install code the release key did not sign.

## Adding the app to the app menu

AppImage: the first time one runs (no entry yet, not asked to stop) a dialog offers **Add to the app menu** / Not now / Don't ask again. "Not now" asks again next
run; "Don't ask again" is remembered; a removal on purpose is also remembered. Adding writes
`~/.local/share/applications/io.github.unicorntearsproject.RustyWave.desktop` (made from the packages' desktop file: `Exec` is the AppImage path quoted per the Desktop Entry
spec, `StartupWMClass` and the window's app id / WM_CLASS are `io.github.unicorntearsproject.RustyWave`, the same MIME types) and the icons under
`~/.local/share/icons/hicolor/<size>x<size>/apps/` (16 to 256 px), then runs `update-desktop-database` and `gtk-update-icon-cache` if they are installed. The file carries
`X-RustyWave-Integrated=true` and the AppImage's path; **only files with that marker are ever changed or removed**, and a desktop file of the same name that
lacks it (a package's) is left alone. If the AppImage is moved, the next start rewrites the entry's path silently (the "repair").
`XDG_DATA_HOME` is honoured.

Portable Windows: "Add to the app menu" writes a per-user Start menu shortcut (`%APPDATA%\Microsoft\Windows\Start Menu\Programs\Rusty Wave.lnk`, made with PowerShell's
`WScript.Shell`) and the per-user (HKCU) registrations the installer makes under HKLM/HKCU: the program under `Applications\rusty-wave.exe`, the `io.github.unicorntearsproject.RustyWave.Media`
ProgId, `Software\RustyWave\Capabilities` + `RegisteredApplications`, and `OpenWithProgids` / `SupportedTypes` for the 17 extensions. The default program is not forced
(Windows reserves that for the user); Rusty Wave appears under "Open with" and Default apps. Removing deletes exactly those keys and the shortcut. The Setup.exe installer
does all of this itself and never shows the offer.

## Command line (scripts and tests)

`rusty-wave --update-now [--update-manifest <url|path>]` checks, installs and exits without a window (status 0 installed, 10 up to date, 11 cannot apply here, 1 failed).
`rusty-wave --integration add|remove|status` changes or shows the app-menu entry and exits. `RVP_UPDATE_MANIFEST_URL` is the environment form of `--update-manifest`.
`--app-services` offers the dialogs even in a scripted run (`--exit-after`, `--screenshot`, `--press`, `--report`), where they are otherwise off so a test is not interrupted.

## Testing the update path locally

See [`release-testing.md`](release-testing.md), "Testing the update path against a local manifest". In short: build the new AppImage and a manifest with
`cargo xtask dist manifest --sign --base-url file:///tmp/rw-dist`, copy the files there, then run the *old* AppImage with
`RVP_UPDATE_MANIFEST_URL=file:///tmp/rw-dist/rusty-wave-latest.json ./Rusty-Wave-old.AppImage --update-now` under `xvfb-run -a` with DISPLAY and WAYLAND_DISPLAY cleared.
Versions 0.0.1 and 0.0.2 have no updater, so the "old" build for such a test is a build of this tree before the version bump.

## Tests

`cargo test -p rvp-update` (signatures good, tampered, wrong key and garbage; checksum and size mismatches; cancel; read-only folder; the AppImage and portable swaps; the integration files
against a temporary data directory including a foreign desktop file; the Windows registry logic against an in-memory registry; one real `gpg` signature by the release key),
`cargo test -p rvp-host-headless --test app_services` (dialogs, persistence, the once-a-day rule, first-run offer) and the dialog tests in `rvp-ui`.
Not covered by an automated run: the real Windows registry and PowerShell shortcut (compiled in the Windows build; exercise it with `--integration` under Wine or on Windows),
a real GNOME/KDE menu picking the entry up, and macOS.

## For maintainers

The program depends on `pgp`, `ureq` (rustls, `ring`), `sha2`, `serde_json` and `zip` (see `THIRD_PARTY_LICENSES.md`). They are vendored for the Flatpak build from
`packaging/flatpak/cargo-sources.json`: regenerate it with `cargo xtask dist flatpak-sources` after any `Cargo.lock` change (this work changed it). The Flatpak has no
network permission and never calls the updater's network code (it answers "update through Flatpak").
