#!/usr/bin/env bash
# Install a Linux package in a clean container and smoke-test the installed program: `rvp --version`, then a headless run on a
# virtual display that opens a fixture, plays it for a few seconds, takes its own screenshot and writes a report.
#
#   tools/packaging/verify-linux.sh deb       [image]   default public.ecr.aws/ubuntu/ubuntu:22.04 (docker.io/library/ubuntu:22.04 in CI)
#   tools/packaging/verify-linux.sh rpm       [image]   default registry.fedoraproject.org/fedora:44
#   tools/packaging/verify-linux.sh appimage  [image]   default public.ecr.aws/ubuntu/ubuntu:22.04
#   tools/packaging/verify-linux.sh apt-signed [image]  install from the signed apt repo (`cargo xtask dist apt-repo --sign`); a tampered copy must be refused
#   tools/packaging/verify-linux.sh rpm-signed [image]  import only the public key, `rpm -K`, install with signature checking on; an unsigned rpm must be refused
#
# Needs podman (or docker with DOCKER=docker), the package in target/dist/release and the fixtures (`cargo xtask fixtures`).
# Prints a report and exits non-zero if anything fails.
set -euo pipefail
kind=${1:?usage: verify-linux.sh deb|rpm|appimage [image]}
root=$(cd "$(dirname "$0")/../.." && pwd)
rel="$root/target/dist/release"
out=$(mktemp -d /tmp/rvp-verify-XXXXXX)
engine=${DOCKER:-podman}
fixture=${RVP_VERIFY_FIXTURE:-h264_aac.mp4}
[ -f "$root/target/fixtures/$fixture" ] || { echo "no fixture: run cargo xtask fixtures"; exit 1; }
if [ "$engine" = podman ]; then
  # An IDE's snap sandbox changes these and podman then refuses its own storage.
  export XDG_DATA_HOME=$HOME/.local/share XDG_CONFIG_HOME=$HOME/.config XDG_CACHE_HOME=$HOME/.cache
  export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
fi

run() { # image, script
  "$engine" run --rm --security-opt label=disable \
    -v "$rel:/pkgs:ro" -v "$root/target/fixtures:/fixtures:ro" -v "$out:/out" "$1" bash -c "$2"
}

# The checks that run inside the container, for the command that starts the program ($1).
smoke() {
  cat <<EOS
echo "== version"; $1 --version
echo "== desktop file, AppStream, icon"
test -f /usr/share/applications/io.github.idometeor.RustyVideoPlayer.desktop && echo desktop ok
test -f /usr/share/metainfo/io.github.idometeor.RustyVideoPlayer.metainfo.xml && echo metainfo ok
test -f /usr/share/icons/hicolor/256x256/apps/io.github.idometeor.RustyVideoPlayer.png && echo icon ok
echo "== run (virtual display, no sound card)"
xvfb-run -a $1 --no-audio --no-media-keys --data-dir /out/data --exit-after 6 --screenshot /out/shot.png --screenshot-after 5 --report /out/report.json /fixtures/$fixture
grep -E '"(state|saw_playing|clock_ratio|video_frames|frames_presented|version)"' /out/report.json
EOS
}

case "$kind" in
  deb)
    image=${2:-public.ecr.aws/ubuntu/ubuntu:22.04}
    deb=$(ls "$rel"/rusty-video-player_*_amd64.deb | head -1)
    run "$image" "
      set -e; export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq >/dev/null
      apt-get install -y -qq --no-install-recommends /pkgs/$(basename "$deb") xvfb xauth libwayland-client0 libx11-6 libxcursor1 libxi6 libxrandr2 fonts-noto-cjk >/dev/null 2>&1
      dpkg -s rusty-video-player | grep -E '^(Version|Depends)'
      $(smoke rvp)
      apt-get remove -y -qq rusty-video-player >/dev/null 2>&1; test ! -e /usr/bin/rvp && echo removed
    " ;;
  rpm)
    image=${2:-registry.fedoraproject.org/fedora:44}
    rpm=$(ls "$rel"/rusty-video-player-*.x86_64.rpm | head -1)
    run "$image" "
      set -e
      dnf install -y -q --setopt=install_weak_deps=False /pkgs/$(basename "$rpm") xorg-x11-server-Xvfb xauth libX11 libXcursor libXi libXrandr libwayland-client >/dev/null 2>&1
      rpm -q rusty-video-player
      $(smoke rvp)
      dnf remove -y -q rusty-video-player >/dev/null 2>&1; test ! -e /usr/bin/rvp && echo removed
    " ;;
  appimage)
    image=${2:-public.ecr.aws/ubuntu/ubuntu:22.04}
    ai=$(ls "$rel"/RustyVideoPlayer-*-x86_64.AppImage | head -1)
    run "$image" "
      set -e; export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq >/dev/null
      apt-get install -y -qq --no-install-recommends xvfb xauth libasound2 libdbus-1-3 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libx11-6 libxcursor1 libxi6 libxrandr2 fonts-noto-cjk >/dev/null 2>&1
      cp /pkgs/$(basename "$ai") /tmp/rvp.AppImage; chmod +x /tmp/rvp.AppImage
      echo '== version'; /tmp/rvp.AppImage --appimage-extract-and-run --version
      echo '== run (virtual display, no sound card)'
      xvfb-run -a /tmp/rvp.AppImage --appimage-extract-and-run --no-audio --no-media-keys --data-dir /out/data --exit-after 6 --screenshot /out/shot.png --screenshot-after 5 --report /out/report.json /fixtures/$fixture
      grep -E '\"(state|saw_playing|clock_ratio|video_frames|frames_presented|version)\"' /out/report.json
    " ;;
  apt-signed)
    image=${2:-public.ecr.aws/ubuntu/ubuntu:22.04}
    [ -f "$root/target/dist/apt-repo/dists/stable/InRelease" ] || { echo "no signed apt repo: cargo xtask dist apt-repo --sign"; exit 1; }
    "$engine" run --rm --security-opt label=disable -v "$root/target/dist/apt-repo:/repo:ro" "$image" bash -c '
      set -e; export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq >/dev/null 2>&1
      apt-get install -y -qq --no-install-recommends ca-certificates >/dev/null 2>&1
      # The public key is the only trust anchor: it goes where signed-by points, no apt-key.
      install -Dm644 /repo/rvp-release.gpg /usr/share/keyrings/rvp-release.gpg
      echo "deb [signed-by=/usr/share/keyrings/rvp-release.gpg] file:/repo stable main" > /etc/apt/sources.list.d/rvp.list
      echo "== update from the signed repo"; apt-get update 2>&1 | grep -E "rvp|repo|stable|Err|W:|E:" || true
      apt-cache policy rusty-video-player | head -4
      apt-get install -y -qq --no-install-recommends rusty-video-player >/dev/null 2>&1
      rvp --version && echo "installed from the signed repo: ok"
      apt-get remove -y -qq rusty-video-player >/dev/null 2>&1; test ! -e /usr/bin/rvp && echo removed
      echo "== a tampered repo must be refused"
      mkdir -p /tmp/bad && cp -r /repo/. /tmp/bad/
      sed -i "s/^Description: .*/Description: tampered/" /tmp/bad/dists/stable/InRelease
      echo "deb [signed-by=/usr/share/keyrings/rvp-release.gpg] file:/tmp/bad stable main" > /etc/apt/sources.list.d/rvp.list
      if apt-get update 2>&1 | tee /tmp/bad.log | grep -qE "not signed|BAD|invalid|signature"; then echo "tampered repo refused: ok"; else cat /tmp/bad.log; echo "TAMPERED REPO ACCEPTED"; exit 1; fi
    ' ;;
  rpm-signed)
    image=${2:-registry.fedoraproject.org/fedora:44}
    rpm=$(basename "$(ls "$rel"/rusty-video-player-*.x86_64.rpm | head -1)")
    "$engine" run --rm --security-opt label=disable -e RPM="$rpm" -v "$rel:/pkgs:ro" -v "$root/packaging/keys:/keys:ro" "$image" bash -c '
      set -e
      echo "== before the key is imported"; rpm -K "/pkgs/$RPM" || true
      rpm --import /keys/rvp-release.asc
      echo "== after"; rpm -Kv "/pkgs/$RPM"
      rpm -K "/pkgs/$RPM" | grep -q "signatures OK" && echo "signature ok"
      cp "/pkgs/$RPM" /tmp/rvp.rpm
      dnf install -y -q --setopt=install_weak_deps=False --setopt=localpkg_gpgcheck=1 /tmp/rvp.rpm >/dev/null 2>&1
      rvp --version && echo "installed (localpkg_gpgcheck=1)"
      dnf remove -y -q rusty-video-player >/dev/null 2>&1; test ! -e /usr/bin/rvp && echo removed
      echo "== a modified rpm must fail"
      cp /tmp/rvp.rpm /tmp/bad.rpm
      printf x | dd of=/tmp/bad.rpm bs=1 seek=$(( $(stat -c %s /tmp/bad.rpm) - 100 )) conv=notrunc 2>/dev/null
      if rpm -K /tmp/bad.rpm; then echo "TAMPERED RPM ACCEPTED"; exit 1; else echo "tampered rpm refused: ok"; fi
    ' ;;
  *) echo "unknown kind $kind"; exit 2 ;;
esac
echo "== screenshot: $out/shot.png"
