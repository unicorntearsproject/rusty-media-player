#!/usr/bin/env bash
# Install a Linux package in a clean container and smoke-test the installed program: `rvp --version`, then a headless run on a
# virtual display that opens a fixture, plays it for a few seconds, takes its own screenshot and writes a report.
#
#   tools/packaging/verify-linux.sh deb       [image]   default public.ecr.aws/ubuntu/ubuntu:22.04 (docker.io/library/ubuntu:22.04 in CI)
#   tools/packaging/verify-linux.sh rpm       [image]   default registry.fedoraproject.org/fedora:44
#   tools/packaging/verify-linux.sh appimage  [image]   default public.ecr.aws/ubuntu/ubuntu:22.04
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
  *) echo "unknown kind $kind"; exit 2 ;;
esac
echo "== screenshot: $out/shot.png"
