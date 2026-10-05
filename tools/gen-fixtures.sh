#!/usr/bin/env bash
# Generate small synthetic test media with ffmpeg into target/fixtures (never committed) and, next to each
# file, the ffprobe packet/stream dump (<name>.probe.json) used as the oracle by the demuxer tests.
#   tools/gen-fixtures.sh [outdir]      (default: <repo>/target/fixtures, or $RVP_FIXTURES)
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-${RVP_FIXTURES:-$root/target/fixtures}}"
mkdir -p "$out"
command -v ffmpeg >/dev/null && command -v ffprobe >/dev/null || { echo "ffmpeg/ffprobe not found" >&2; exit 1; }
ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }

dur=6
vsrc=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=$dur")
asrc=(-f lavfi -i "sine=frequency=440:sample_rate=48000:duration=$dur")
# 25 fps, keyframe every 12 frames, 2 B-frames where the encoder supports them.
h264=(-c:v libx264 -preset veryfast -g 12 -bf 2 -pix_fmt yuv420p)
aac=(-c:a aac -b:a 64k -ac 2)

ff "${vsrc[@]}" "${asrc[@]}" "${h264[@]}" "${aac[@]}" -shortest "$out/h264_aac.mp4"
ff "${vsrc[@]}" "${asrc[@]}" "${h264[@]}" "${aac[@]}" -shortest -movflags +faststart "$out/h264_aac_faststart.mp4"
ff "${vsrc[@]}" "${asrc[@]}" "${h264[@]}" "${aac[@]}" -shortest \
   -movflags frag_keyframe+empty_moov+default_base_moof "$out/h264_aac_frag.mp4"
ff "${vsrc[@]}" "${asrc[@]}" -c:v libsvtav1 -preset 10 -g 12 -pix_fmt yuv420p -svtav1-params log=0 \
   -c:a libopus -b:a 64k -ac 2 -shortest "$out/av1_opus.webm"
ff "${vsrc[@]}" "${asrc[@]}" -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -g 12 -b:v 300k -pix_fmt yuv420p \
   -c:a libvorbis -b:a 64k -ac 2 -shortest "$out/vp9_vorbis.webm"
ff "${vsrc[@]}" "${asrc[@]}" "${h264[@]}" -c:a flac -ac 2 -shortest "$out/h264_flac.mkv"

# Audio-only MP3 in Matroska.
ff "${asrc[@]}" -c:a libmp3lame -b:a 128k -ac 2 "$out/mp3.mkv"

for f in "$out"/*.mp4 "$out"/*.webm "$out"/*.mkv; do
  ffprobe -v error -show_format -show_streams -show_packets -of json "$f" > "$f.probe.json"
done
touch "$out/.done"
echo "fixtures in $out"
