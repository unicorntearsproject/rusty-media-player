#!/usr/bin/env bash
# Runs an AppImage for a test or an acceptance check without leaving a FUSE mount behind.
#   tools/run-appimage.sh <file.AppImage> [args...]
# It always passes --appimage-extract-and-run (the image is unpacked into $TMPDIR, no FUSE mount at all), stops the app with SIGTERM and
# waits when this script is told to stop, and on every way out lazily unmounts any /tmp/.mount_Rusty* that appeared meanwhile.
set -euo pipefail
img="${1:?usage: run-appimage.sh <file.AppImage> [args...]}"; shift
before="$(ls -d /tmp/.mount_Rusty* 2>/dev/null || true)"
pid=""
cleanup() {
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  fi
  for m in $(ls -d /tmp/.mount_Rusty* 2>/dev/null || true); do
    grep -qxF "$m" <<<"$before" || fusermount -uz "$m" 2>/dev/null || true
  done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
"$img" --appimage-extract-and-run "$@" &
pid=$!
rc=0
wait "$pid" || rc=$?
pid=""
exit "$rc"
