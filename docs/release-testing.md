# Release testing checklist

Everything that cannot be checked in a container or under Wine: real machines, real audio hardware, real desktops, a real Windows PC and real
browsers. Nothing here is public yet; build and sign locally, test, then decide. Tick the boxes, note the machine and the version in the table at the
end. See [`packaging.md`](packaging.md) for how the files are built and signed.

Release key: `E13F F843 723D 5406 8E45  A3FF 54BF 2FA4 0709 3CEE` (key ID `54BF2FA407093CEE`, "Rusty Wave Release", expires 2028-10-04).
Version below: `0.0.2` (replace with what you built).

## 0. Build and collect the files (on the dev machine)

```sh
source ~/.cargo/env
cargo xtask dist linux --sign          # stage, tarball, deb, rpm, AppImage, Flatpak (signed repo + bundle), apt repo
cargo xtask dist windows && cargo xtask dist installer      # exe, zip, Setup.exe (Wine image; unsigned by GPG, see section 5)
cargo xtask dist pwa
cargo xtask dist checksums --sign      # SHA256SUMS and a .asc beside every file
cargo xtask dist verify                # every signature against packaging/keys/rusty-wave-release.asc
```

Files are in `target/dist/release/`; the apt repo is `target/dist/apt-repo/`, the Flatpak repo `target/dist/flatpak/repo/`. To hand files to
another machine:

```sh
cd target/dist && python3 -m http.server 8000       # on the dev machine; then http://<dev-machine>:8000/release/... elsewhere
# or: scp -r target/dist/release target/dist/apt-repo user@host:rusty-wave-test/
```

Test media: your own files (a mix of MP4/H.264, MKV, WebM/AV1, MP3, FLAC, Opus, a folder of music with CJK and accented names) plus
`cargo xtask fixtures` (`target/fixtures/h264_aac.mp4`, `av1_opus.webm`, `vp9_vorbis.webm`, `h264_flac.mkv`, `library/`).

## 1. Verify the download (every machine, before installing)

Linux / macOS (needs `gpg`; the key file is `packaging/keys/rusty-wave-release.asc`, also `rusty-wave-release.asc` inside the apt repo):

```sh
gpg --import rusty-wave-release.asc
gpg --fingerprint 54BF2FA407093CEE         # must print E13F F843 723D 5406 8E45  A3FF 54BF 2FA4 0709 3CEE
gpg --verify SHA256SUMS.asc SHA256SUMS      # "Good signature from Rusty Wave Release" (the "not certified" warning is expected)
sha256sum -c SHA256SUMS --ignore-missing    # every file you downloaded says OK
gpg --verify <file>.asc <file>              # any single file
```

- [ ] Fingerprint matches the one above (compare it with this document, not with a message that came with the files).
- [ ] `SHA256SUMS` signature good; every checksum OK.
- [ ] Tamper test: copy a package, change one byte (`printf x | dd of=copy bs=1 seek=1000 conv=notrunc`), `gpg --verify` / `sha256sum -c` must fail.

Windows (PowerShell; the signature check needs Gpg4win, the checksum does not):

```powershell
Get-FileHash .\rusty-wave-0.0.2-x64-Setup.exe -Algorithm SHA256     # compare with the line in SHA256SUMS
gpg --import rusty-wave-release.asc; gpg --verify SHA256SUMS.asc SHA256SUMS      # optional, needs Gpg4win
```

## 2. What to look for in every desktop build (Linux, Windows and macOS)

Do this once per package type on each machine, with real speakers or headphones.

- [ ] **Starts**: `rusty-wave --version` prints `rusty-wave 0.0.2`; the window opens in about a second with the right icon and title; no console window on Windows.
- [ ] **Video**: play an H.264 MP4, an AV1 WebM and a VP9 WebM. Smooth, no tearing, lip sync right (a clapper or a speaking face), seeking with the arrows
  and by clicking the bar lands where it should; full screen (`F` / `F11`) and back; `Space` pauses; `[` `]` change speed with pitch preserved.
- [ ] **Audio**: sound comes out of the default device at the right pitch and speed (no chipmunk or slow-motion, no crackle when the window is dragged or the
  machine is busy). Volume (`Up`/`Down`) and mute (`M`) work. Change the default output device while playing (plug in headphones, switch Bluetooth): playback
  continues on the new device or recovers within a couple of seconds, without a crash. Gapless: play an album (a live or classical one) and listen across track
  changes. Play MP3, FLAC, Opus, Vorbis, AAC/M4A and WAV.
- [ ] **Media keys and the shell integration**:
  - Linux: the hardware play/pause, next, previous keys act on the player; the desktop's media widget (GNOME top bar, KDE panel) shows title, artist and cover
    and its buttons work; `playerctl -l` lists `io.github.idometeor.RustyWave`, `playerctl -p io.github.idometeor.RustyWave status|metadata|play-pause|next`.
  - Windows: the hardware keys, the volume flyout (Win + the volume keys) and the lock screen show title, artist and cover; Bluetooth headset buttons work;
    the keys do not also control another player at the same time.
  - With another player (a browser tab playing) open: the keys go to the most recently active one, as with other apps.
- [ ] **Library and queue**: open a folder (`O` or drop it on the window): albums, artists, tracks, cover art, search (`Ctrl+F`); the queue (`Q`); a saved M3U/M3U8/PLS
  playlist imports. Quit during playback and start again: the queue and the position come back. CJK, Cyrillic and accented names render (needs a CJK font on the system).
- [ ] **Subtitles and tracks**: an MKV or MP4 with subtitles shows them (`S`), a sidecar `.srt` next to a video loads, `A` switches audio tracks.
- [ ] **File associations**: double-click an `.mp4`, `.mkv`, `.mp3`, `.flac` in the file manager after choosing Rusty Wave; "Open with" lists it with its icon; opening
  several files at once queues them all; a file opened while the player is already running (see how the OS handles it: a second window or the same one) does not lose the first.
- [ ] **HiDPI**: sharp text and icons at 100%, 125%, 150%, 200% (and fractional on GNOME/KDE Wayland); window size sensible; moving the window between monitors with different
  scales redraws crisply; full screen on the second monitor; no blur from the compositor (Wayland) or Windows' bitmap scaling.
- [ ] **Resources**: idle CPU near zero when paused; 1080p30 playback below roughly one core on a modern machine; memory settles after a long library scan; the window can be resized
  while playing without freezing.
- [ ] **Data lives where documented**: `~/.local/share/rusty-wave/` (Flatpak `~/.var/app/io.github.idometeor.RustyWave/data/rusty-wave/`, Windows `%APPDATA%\rusty-wave\data`); `rusty-wave --data-dir DIR` redirects it.
- [ ] **Uninstall is clean** (the package's own steps below), and the user data is left unless you chose to remove it.

## 3. Linux packages

Test on at least: Ubuntu 22.04 and 24.04, Debian 12 (and 13), Fedora (current), openSUSE Tumbleweed or Leap, Arch or another rolling distro, with GNOME
(Wayland), KDE Plasma (Wayland) and one X11 session (XFCE or Xorg GNOME/KDE). Runtime libraries are not bundled except in the Flatpak: `libasound2`,
`libdbus-1`, `libxkbcommon(-x11)` and the X11/Wayland client libraries. Note any missing-library error verbatim: it means a dependency line needs fixing.

### 3.1 `.deb` (Debian, Ubuntu, Mint, Pop!_OS)

Direct install:

```sh
gpg --verify rusty-wave_0.0.2_amd64.deb.asc rusty-wave_0.0.2_amd64.deb
sudo apt install ./rusty-wave_0.0.2_amd64.deb          # pulls the dependencies; apt may warn that ./file is "not sandboxed", that is normal
dpkg -s rusty-wave | grep -E 'Version|Depends'
dpkg -L rusty-wave | head -30                          # /usr/bin/rusty-wave, desktop file, metainfo, icons, man page, licences
rusty-wave --version && man rusty-wave
update-desktop-database ~/.local/share/applications 2>/dev/null; gio mime video/mp4 | head -3
```

Through the signed apt repo (this is the check that apt verifies the signature):

```sh
# dev machine: cd target/dist/apt-repo && python3 -m http.server 8000     (or copy the folder and use file:/path)
curl -fsSL http://<dev-machine>:8000/rusty-wave-release.gpg | sudo tee /usr/share/keyrings/rusty-wave-release.gpg >/dev/null
gpg --show-keys /usr/share/keyrings/rusty-wave-release.gpg             # fingerprint must match
echo 'deb [signed-by=/usr/share/keyrings/rusty-wave-release.gpg] http://<dev-machine>:8000 stable main' | sudo tee /etc/apt/sources.list.d/rusty-wave.list
sudo apt update          # a line "Get: ... stable InRelease" and no "NO_PUBKEY" or "not signed" warnings
apt-cache policy rusty-wave
sudo apt install rusty-wave
```

- [ ] `apt update` accepts the repo; with a wrong key in `signed-by` (try the Unicorn Viz key or an empty file) it refuses ("NO_PUBKEY" / "not signed").
- [ ] Dependencies resolved on a minimal desktop; on 24.04 and Debian 13 `libasound2t64` satisfies the `libasound2 | libasound2t64` alternative.
- [ ] Menu entry "Rusty Wave" with the icon, in the Sound & Video category; "Open with" offers it for mp4/mkv/mp3/flac.
- [ ] Section 2 checks pass.
- [ ] Uninstall: `sudo apt remove rusty-wave` (binary, menu entry, icons gone; `~/.local/share/rusty-wave` stays); `sudo apt purge rusty-wave`; then remove the repo:
  `sudo rm /etc/apt/sources.list.d/rusty-wave.list /usr/share/keyrings/rusty-wave-release.gpg && sudo apt update`.

### 3.2 `.rpm` (Fedora, openSUSE, RHEL clones)

```sh
sudo rpm --import rusty-wave-release.asc                 # once; rpm now trusts the key (this is system-wide: remove it again below)
rpm -q gpg-pubkey --qf '%{NAME}-%{VERSION}-%{RELEASE}  %{SUMMARY}\n' | grep -i 'Rusty Video'
rpm -Kv rusty-wave-0.0.2-1.x86_64.rpm     # "EdDSA/SHA512 signature, key fingerprint: e13ff843...3cee: OK", header and payload digests OK
sudo dnf install ./rusty-wave-0.0.2-1.x86_64.rpm          # openSUSE: sudo zypper install ./rusty-wave-0.0.2-1.x86_64.rpm
rpm -qi rusty-wave; rpm -ql rusty-wave | head -30
rusty-wave --version
```

- [ ] Before `rpm --import`, `rpm -K` says NOKEY / "SIGNATURES NOT OK"; after, OK. A modified copy fails (`printf x | dd of=copy bs=1 seek=100000 conv=notrunc`).
- [ ] An older rpm (RHEL/Alma 8, openSUSE Leap) that cannot verify EdDSA signatures: note the error (rpm below 4.14.3/4.16 does not know ed25519; the package still installs with `--nosignature`).
- [ ] Dependencies resolve (`libxkbcommon-x11`, `alsa-lib`, `dbus-libs`); on openSUSE names differ (`libxkbcommon-x11-0`, `libasound2`): note any that `zypper` cannot find.
- [ ] Section 2 checks pass.
- [ ] Uninstall: `sudo dnf remove rusty-wave` (zypper: `sudo zypper remove rusty-wave`), then drop the key:
  `sudo rpm -e gpg-pubkey-54bf2fa4-<release>` (the name from the `rpm -q gpg-pubkey` line above).

### 3.3 AppImage (any distro; the Arch / immutable-distro case)

```sh
gpg --verify rusty-wave-0.0.2-x86_64.AppImage.asc rusty-wave-0.0.2-x86_64.AppImage
python3 tools/packaging/verify-appimage-sig.py rusty-wave-0.0.2-x86_64.AppImage E13FF843723D54068E45A3FF54BF2FA407093CEE packaging/keys/rusty-wave-release.asc   # the embedded signature
chmod +x rusty-wave-0.0.2-x86_64.AppImage
./rusty-wave-0.0.2-x86_64.AppImage --version
./rusty-wave-0.0.2-x86_64.AppImage ~/Videos/some.mp4
./rusty-wave-0.0.2-x86_64.AppImage --appimage-extract-and-run --version     # where FUSE 2 is missing (Fedora 40+, Ubuntu 22.04+ without libfuse2)
```

- [ ] Runs from `~/Downloads` and from a path with spaces; with FUSE missing it prints the usual "libfuse.so.2" error and `--appimage-extract-and-run` works.
- [ ] Menu integration is *not* automatic: with AppImageLauncher or `appimaged` it appears in the menu with the icon; without them it does not (expected, note it).
- [ ] Section 2 checks pass (file associations only after integration).
- [ ] Uninstall: delete the file (and the AppImageLauncher entry); data stays in `~/.local/share/rusty-wave`.

### 3.4 Flatpak (any distro with Flatpak; GNOME Software / Discover too)

The signed repo, the `.flatpakrepo`, the `.flatpakref` and the single-file bundle are all built by `dist flatpak --sign`. The runtime (`org.freedesktop.Platform//25.08`)
comes from Flathub, so the machine needs the `flathub` remote (`flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo`).

a) From the signed repo. The repo is a folder; serve it (`cd target/dist/flatpak && python3 -m http.server 8000`) or copy it over. The `.flatpakrepo` carries the key and
`Url=file:///home/jj/...` (the dev machine's path), so on another machine either rebuild with `dist flatpak --sign --repo-url http://<dev-machine>:8000/repo`,
or add the remote by hand with the key:

```sh
flatpak remote-add --user --if-not-exists --gpg-import=rusty-wave-release.gpg rusty-wave-test http://<dev-machine>:8000/repo     # rusty-wave-release.gpg from packaging/keys
# on the dev machine the remote already exists (disabled): flatpak remote-modify --user --enable rusty-wave-test
flatpak remotes --user -d | grep rusty-wave-test        # the options column must NOT say no-gpg-verify
flatpak install --user rusty-wave-test io.github.idometeor.RustyWave
flatpak info --user io.github.idometeor.RustyWave       # Origin rusty-wave-test, Branch stable, Commit ...
flatpak run io.github.idometeor.RustyWave ~/Videos/some.mp4
```

- [ ] Wrong-key test: `flatpak remote-add --user --gpg-import=<some other public key> rvp-bad http://<dev-machine>:8000/repo` then `flatpak install --user rvp-bad ...` fails with
  "Can't check signature: public key not found". Remove it: `flatpak remote-delete --user rvp-bad`.

b) From the `.flatpakref` (installs the app and adds the remote named `rusty-wave`): `flatpak install --user --from io.github.idometeor.RustyWave.flatpakref`
(its `Url=` must be reachable, as above). GNOME Software opens `.flatpakref` files directly.

c) From the single-file bundle (works offline except for the runtime): `flatpak install --user --bundle io.github.idometeor.RustyWave-0.0.2.flatpak`.

- [ ] All three routes install; the app appears in the menu with its icon; `flatpak run` starts it.
- [ ] Sandbox: `flatpak info --show-permissions io.github.idometeor.RustyWave` lists Wayland, fallback X11, PulseAudio, `xdg-music:ro`, `xdg-videos:ro`, the MPRIS names, and no network, no home, no GPU.
- [ ] Opening a file from `~/Downloads` or another folder: through the file chooser (portal) and by dragging a file from the file manager onto the window.
  A folder outside Music and Videos can be added through the chooser.
- [ ] Media keys, the shell's media widget and `playerctl` see the player from the host (MPRIS inside the sandbox): `playerctl -l`.
- [ ] Audio through PipeWire/PulseAudio; works on a PulseAudio-only and on a PipeWire system.
- [ ] Section 2 checks pass (file associations come from the exported desktop file: "Open with" in the host file manager).
- [ ] Update path: install the bundle, rebuild with a bumped version and a new repo, `flatpak update --user` picks it up (only if you serve the repo).
- [ ] Uninstall: `flatpak uninstall --user io.github.idometeor.RustyWave` (`--delete-data` also removes `~/.var/app/io.github.idometeor.RustyWave`),
  `flatpak remote-delete --user rusty-wave-test` (and `rusty-wave`, `rustywave-origin` if they were added), `flatpak uninstall --user --unused`.

### 3.5 Tarball

`rvp-0.0.2-linux-x86_64.tar.gz`: unpack to a temp folder, `./usr/bin/rusty-wave --version` (needs the runtime libraries of section 3); the tree mirrors `/usr`.

### 3.6 Testing the update path against a local manifest (AppImage, no network)

Build a "new" version and serve it from a local folder: `cargo xtask dist appimage --sign --version 0.0.9 --base-url file:///tmp/rw-dist` (embeds `zsync|file:///tmp/rw-dist/...`
and writes the `.zsync` with that base), `cargo xtask dist checksums --sign`, then `cargo xtask dist manifest --sign --base-url file:///tmp/rw-dist` and copy the AppImage, its `.asc`, the
`.zsync` (also as `rusty-wave-latest-x86_64.AppImage.zsync`), `rusty-wave-latest.json` and its `.asc` into `/tmp/rw-dist`. Point the installed (older) Rusty Wave at that manifest
as described in [`updates.md`](updates.md) (the other end of this contract) and check that it finds 0.0.9, verifies the SHA-256 and the signature, and replaces itself. `publish` never
accepts `--base-url`, so a test build can never reach the bucket with local URLs.

## 4. Quick container checks (dev machine, no hardware)

```sh
tools/packaging/verify-linux.sh deb          # clean Ubuntu 22.04: install, run under Xvfb, remove
tools/packaging/verify-linux.sh rpm          # clean Fedora: same
tools/packaging/verify-linux.sh appimage
tools/packaging/verify-linux.sh apt-signed   # install from the signed apt repo; a tampered InRelease is refused
tools/packaging/verify-linux.sh rpm-signed   # import only the public key, rpm -K, install; a modified rpm is refused
```

## 5. Windows (a real PC: Windows 10 22H2 and Windows 11; ideally one laptop with a touch/HiDPI screen and one desktop with two monitors at different scales)

Files: `rusty-wave-0.0.2-x64-Setup.exe` (installer) and `rvp-0.0.2-windows-x64.zip` (portable). Both are **unsigned** (no code-signing certificate yet; the GPG
signature of the checksums is the only proof). Take a snapshot or use a throwaway user/VM as well as the real machine.

**SmartScreen and Defender** (what an unsigned download looks like):

- [ ] Download the Setup.exe through a browser (so it gets the Mark of the Web). Running it shows "Windows protected your PC" with publisher "Unknown publisher"; "More info" reveals
  "Run anyway". Record the exact text and whether it appears for a file copied from a USB stick or a network share (usually not).
- [ ] Defender / SmartScreen does not quarantine or delete `rusty-wave.exe` (a Rust GUI exe can trip heuristics): if it does, record the detection name and submit the file to Microsoft as a false positive.
- [ ] Right-click the exe, Properties: no digital-signature tab (expected); Details tab shows product name, version 0.0.2, file description, copyright, the icon.

**Installer** (`Setup.exe`):

- [ ] Wizard: licence page (MIT or Apache-2.0), the task page (Open-with registration ticked, desktop shortcut and "add rusty-wave to PATH" unticked), the wizard images are sharp at 100% and 200% scaling.
  Default is a per-user install in `%LOCALAPPDATA%\Programs\Rusty Wave` without an admin prompt; the dialog offers "install for all users" (then `C:\Program Files\Rusty Wave`, with UAC).
- [ ] Finish page offers "Launch"; the app starts.
- [ ] Start menu has "Rusty Wave" and "Uninstall Rusty Wave"; Settings > Apps > Installed apps lists it with version, publisher and icon.
- [ ] Silent install, as an admin would script it: `Setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=%TEMP%\rvp-install.log` (add `/ALLUSERS` or `/CURRENTUSER`, `/DIR="D:\Apps\RVP"`,
  `/TASKS="assoc,desktopicon,addtopath"`); the log shows no errors.
- [ ] A path with spaces and non-ASCII (`C:\Users\Zoë Müller\Music`) installs and plays.
- [ ] Reinstall over an existing install keeps settings; install while the app is running asks to close it.

**File associations** (registered, not forced: Windows 10/11 reserve the default for the user):

- [ ] Settings > Apps > Default apps > "Rusty Wave" lists `.mp4 .m4v .mkv .webm .mka .mp3 .flac .ogg .oga .opus .wav .m4a .m4b .aac .m3u .m3u8 .pls`.
- [ ] Right-click an `.mp4` > Open with > Choose another app > Rusty Wave (with its icon, not the generic one) > "Always" makes double-click work; same for `.mp3` and `.flac`.
  The context menu also has "Add to Rusty Wave queue" where applicable.
- [ ] Selecting 10 files in Explorer and pressing Enter queues all of them in one window.
- [ ] Registry (PowerShell): `reg query HKCU\Software\RegisteredApplications`, `reg query HKCU\Software\Classes\.mp4\OpenWithProgids`, `reg query "HKCU\Software\RustyWave\Capabilities\FileAssociations"` (HKLM for an all-users install).

**Running it**: section 2, plus:

- [ ] Audio through WASAPI at the right pitch on built-in speakers, USB and Bluetooth devices; switching the default device mid-playback; unplugging headphones; a 44.1 kHz file on a 48 kHz device;
  sleep/resume of the PC during playback does not leave silence or a hang.
- [ ] Media keys and the System Media Transport Controls (volume flyout, lock screen, Bluetooth buttons) show title, artist, cover; they control Rusty Wave and no other player at once.
- [ ] DPI: at 100/125/150/175/200% the window, text and icons are sharp (the exe declares per-monitor-v2 awareness); change the scale in Settings while it runs; drag between monitors with different scales;
  full screen on each monitor; taskbar icon crisp; the title bar looks right in light and dark mode.
- [ ] Drag a file from Explorer onto the window; drag from a network share and from `\\server\share` (UNC) paths; a path longer than 260 characters opens (long paths enabled in the manifest);
  CJK and emoji in file names and in the title bar.
- [ ] `rusty-wave --help` and `rusty-wave --version` print into the terminal you start it from (it is a GUI-subsystem exe that attaches to the parent console); with "add to PATH" ticked, `rusty-wave` works from a new terminal.
- [ ] Task Manager: the app shows as "Rusty Wave" with its icon; CPU near zero when paused; no leaked handles after 30 minutes of playback.
- [ ] Windows Firewall does not prompt (the app makes no network connections).

**Uninstall**:

- [ ] Settings > Apps > Rusty Wave > Uninstall (or `"%LOCALAPPDATA%\Programs\Rusty Wave\unins000.exe" /VERYSILENT`): asks whether to delete settings and the library index; answer both ways on two runs.
- [ ] Gone afterwards: the install folder, Start menu and desktop shortcuts, `HKCU\Software\Classes\io.github.idometeor.RustyWave.Media`, the `RegisteredApplications` value, the `OpenWithProgids` values, the PATH entry.
  "Open with" no longer lists it. `%APPDATA%\rusty-wave\data` is gone only if you chose to delete it.

**Portable zip**: unzip to a folder (and to a USB stick), run `rusty-wave.exe`: SmartScreen prompt as above (unblock via file Properties > Unblock to compare), plays, settings go to `%APPDATA%\rusty-wave\data`
(or next to it with `--data-dir`), nothing registered; deleting the folder removes it.

## 5b. macOS (beta: Apple Silicon and Intel, macOS 11 or newer)

Nobody has run Rusty Wave on a Mac yet. The `.dmg` (`rusty-wave-<ver>-macos-universal.dmg`, one binary for arm64 and x86_64) is built by the `macos` job of `release.yml` on a macOS runner (or
`cargo xtask dist macos` on a Mac); this is the first real test. Unless the maintainer has Apple signing secrets set, the app is **ad-hoc signed only: not signed with a Developer ID and not
notarized**, so Gatekeeper blocks the first launch ("Rusty Wave cannot be opened because the developer cannot be verified" or "... is damaged"). That is expected. Verify the download first
(section 1: `gpg --verify x.dmg.asc x.dmg`, `shasum -a 256 -c`), then open the dmg and drag *Rusty Wave* onto *Applications*, and allow it once, whichever way you prefer:

- Finder: right-click (or Control-click) the app, choose **Open**, then **Open** again in the dialog (macOS 14 and older); or
- macOS 15 and newer: try to open it once, then System Settings > Privacy & Security, scroll to Security, **Open Anyway**; or
- Terminal, on your own Mac and only for this app: `xattr -dr com.apple.quarantine "/Applications/Rusty Wave.app"` (this removes the quarantine flag that triggers the check; it is the same as
  allowing the app by hand).

What to test (the checks of section 2 apply, with these macOS parts):

- [ ] Starts on Apple Silicon and on an Intel Mac (`lipo -archs "/Applications/Rusty Wave.app/Contents/MacOS/rusty-wave"` prints `x86_64 arm64`); the window and Dock icon are right; the menu bar shows the app name.
- [ ] Video (H.264, AV1, VP9) and audio (MP3, FLAC, Opus, AAC, WAV) play with sound at the right pitch; changing the output device (AirPods, HDMI) while playing recovers; a Retina display is sharp (HiDPI).
- [ ] Keyboard shortcuts work with Command where the Windows/Linux builds use Ctrl, or at least with Ctrl; full screen (the green button and `F`); the window resizes smoothly.
- [ ] Media keys (F7 to F9, the AirPods squeeze) and Control Centre / Now Playing show title, artist and cover and control playback (MPNowPlayingInfoCenter through the media-controls library).
- [ ] Open a folder and a file from inside the app. Finder integration (**double-click or Open With a `.mp4`/`.mkv`/`.mp3`/`.flac`, or drop a file on the Dock icon**) is the most likely thing to fail: the app reads its command line,
  while Finder passes files as an Apple Event; note exactly what happens (opens empty, opens the file, opens a second copy).
- [ ] "Get Info" on a media file lists Rusty Wave under Open With (the Info.plist declares the types); it does not become the default by itself.
- [ ] Data lives in `~/Library/Application Support/rusty-wave/` (or wherever the app reports with `--help`); `--data-dir` redirects it. Uninstall is: drag the app to the Trash, remove that folder.
- [ ] If the maintainer provided a signed and notarized build: it opens with no Gatekeeper dialog (`spctl --assess --type execute -vv "/Applications/Rusty Wave.app"` says "accepted, source=Notarized Developer ID").

**Not verified (known)**: everything above. Specifically the Info.plist file-type declarations (Launch Services may need `lsregister -f` or a logout to pick them up), Finder file opening, audio on both architectures,
the media controls, notarization, and the unsigned-app flow on a current macOS. Report what you see, the macOS version, the chip, and `rusty-wave --version`.

## 6. Web app (PWA)

Build and serve it: `cargo xtask web && cargo xtask serve` (http://127.0.0.2:8080, with the COOP/COEP headers for the threaded decoder), or unzip `rusty-wave-web-0.0.2.zip` on any HTTPS host.
A service worker and installing need a secure context: `localhost` counts, a plain `http://<lan-ip>:8080` does not. To test from another machine use a tunnel that gives HTTPS
(`ssh -L 8080:localhost:8080 dev-machine` makes `localhost:8080` secure on the test machine) or deploy the zip to an HTTPS host. Without COOP/COEP the page runs the single-threaded decoder
(fine for 720p, slower for 1080p); serve with the headers (`cargo xtask serve` does) to test the threaded decoder.

**Chrome and Edge, desktop (Windows, Linux, macOS)**:

- [ ] The page loads, plays a local file (drop it or the open button), audio and video in sync, the keyboard shortcuts work, the Library face (`B`) scans a folder (File System Access API).
- [ ] "Install app" button in the page and the browser's install icon in the address bar (or menu > Cast, save and share > Install page as app) both install it; the app opens in its own window without browser chrome,
  with the right name, icon (check the maskable icon on Android/ChromeOS) and theme colour; it appears in the Start menu / app launcher / Dock.
- [ ] Offline: with the app installed, turn the network off (DevTools > Network > Offline, or airplane mode), reload: it starts, plays a local file.
- [ ] Update: build again so the version changes (`cargo xtask web`), reload: a bar shows "Update available: reload"; reload applies it; the old cache is gone (DevTools > Application > Cache storage).
- [ ] File handlers (Chromium desktop only): right-click an `.mp4`/`.mp3` > Open with > Rusty Wave; the first time Chrome asks to allow the app to open these types; the file plays. Settings > Apps > Default apps can make it the default.
- [ ] Share target: Chrome on Android, ChromeOS, and the Windows share sheet (where supported): share an audio/video file to the installed app, it opens in the player.
- [ ] The shortcut in the app icon's context menu (jump list / long-press) works.
- [ ] Media Session: hardware media keys and the OS media overlay (Chrome's media hub, the Windows volume flyout, GNOME/KDE widget) show title/artist/artwork and control playback. Background tab keeps playing audio.
- [ ] Uninstall: the app window's menu (three dots) > Uninstall, or `chrome://apps` > right-click > Remove (tick "clear data" to drop IndexedDB and caches); the shortcut disappears.

**Firefox**:

- [ ] Desktop Firefox does not install PWAs: confirm the install button is hidden, the page works in a tab, the service worker registers (`about:debugging` > This Firefox > Service Workers) and the offline start works;
  it must not show errors for the missing File Handling API; the threaded decoder needs the COOP/COEP headers (Firefox supports them).
- [ ] Firefox on Android: "Install" (add to Home screen) creates a standalone app that starts offline.
- [ ] Library folder picking uses the `<input webkitdirectory>` fallback where the File System Access API is missing: choose a folder, it scans.

**Safari / iOS** (optional): Add to Home Screen, plays H.264/AAC through our own decoder (not the system player), audio resumes after the screen locks only if the browser allows it.

## 7. Results

| Date | Machine / OS / desktop | Package and version | Install | Verify | Audio | Media keys | Assoc. | DPI | Uninstall | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| | | | | | | | | | | |

When something fails, record: the exact command or click, the output (for apt/dnf/flatpak the whole message), `rusty-wave --version`, and for crashes the last lines of the terminal
(`RUST_BACKTRACE=1 rusty-wave ...`). Then reproduce with `--no-audio` and `--no-media-keys` to separate the audio and shell-integration layers from the player.
