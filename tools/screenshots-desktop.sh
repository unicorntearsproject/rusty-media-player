#!/usr/bin/env bash
# Regenerates docs/screenshots/desktop/*.png headless: the real desktop app (release build) on a virtual display, over the generated
# showcase library (tools/gen-library.py --showcase, made by `cargo xtask fixtures`) and a test video. One run per picture.
#   tools/screenshots-desktop.sh [path-to-rusty-wave]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bin="${1:-${CARGO_TARGET_DIR:-$root/target}/release/rusty-wave}"
fx="${RVP_FIXTURES:-$root/target/fixtures}"
out="$root/docs/screenshots/desktop"
scratch="${RVP_SCRATCH:-/mnt/scratch/rvp-scratch}/shots-$$"
mkdir -p "$out" "$scratch/xdg" "$scratch/videos"
trap 'gpgconf --kill all >/dev/null 2>&1 || true' EXIT
printf 'XDG_MUSIC_DIR="%s"\nXDG_VIDEOS_DIR="%s"\n' "$fx/showcase/music" "$scratch/videos" > "$scratch/xdg/user-dirs.dirs"

shot() { # name after-seconds exit-seconds [press...] -- [file...]
  local name="$1" after="$2" exit="$3"; shift 3
  local presses=() files=()
  while [[ $# -gt 0 && "$1" != "--" ]]; do presses+=(--press "$1"); shift; done
  [[ "${1:-}" == "--" ]] && shift
  files=("$@")
  local data="$scratch/data-$name"; mkdir -p "$data"
  env -u WAYLAND_DISPLAY -u DISPLAY RVP_NO_OFFERS=1 XDG_CONFIG_HOME="$scratch/xdg" GDK_BACKEND=x11 \
    nice -n 10 xvfb-run -a -s "-screen 0 1280x800x24" "$bin" --data-dir "$data" --no-audio --no-media-keys --window 1280x720 \
      "${presses[@]}" --exit-after "$exit" --screenshot "$out/$name.png" --screenshot-after "$after" "${files[@]}" > /dev/null
  echo "wrote $out/$name.png"
}

shot 01-library-albums 9 11 3:1
shot 02-now-playing 12 14 5:3 6:Down 7:Down 8:Enter 9:6
shot 03-visualizer 13 15 5:3 6:Down 7:Enter 8:7
shot 04-video 5 7 -- "$fx/h264_aac.mp4"
shot 05-help 10 12 3:1 8:h
