#!/usr/bin/env bash
# Generate small synthetic test media with ffmpeg into target/fixtures (never committed) and, next to each
# file, the ffprobe packet/stream dump (<name>.probe.json) used as the oracle by the demuxer tests.
#   tools/gen-fixtures.sh [outdir]      (default: <repo>/target/fixtures, or $RVP_FIXTURES)
#   RVP_FIXTURE_SET=core|h264|all       which set to build (default all); the H.264 set goes to <outdir>/h264
#   RVP_FIXTURE_FORCE=1                 rebuild files that already exist (the H.264 set otherwise skips them)
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-${RVP_FIXTURES:-$root/target/fixtures}}"
mkdir -p "$out"
command -v ffmpeg >/dev/null && command -v ffprobe >/dev/null || { echo "ffmpeg/ffprobe not found" >&2; exit 1; }
ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }

gen_core() {
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

  # 10-bit AV1 (video only, 2 s).
  ff -f lavfi -i "testsrc2=size=320x240:rate=25:duration=2" -c:v libsvtav1 -preset 10 -g 12 -pix_fmt yuv420p10le \
     -svtav1-params log=0 "$out/av1_10bit.webm"
  # One minute of AV1+Opus for the A/V sync soak test.
  ff -f lavfi -i "testsrc2=size=320x240:rate=25:duration=60" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=60" \
     -c:v libsvtav1 -preset 12 -g 50 -pix_fmt yuv420p -svtav1-params log=0 -c:a libopus -b:a 64k -ac 2 -shortest "$out/av1_opus_60s.webm"
  # Audio-only MP3 in Matroska.
  ff "${asrc[@]}" -c:a libmp3lame -b:a 128k -ac 2 "$out/mp3.mkv"

  for f in "$out"/*.mp4 "$out"/*.webm "$out"/*.mkv; do
    [[ "$f" == *60s* ]] && continue # the soak file needs no packet dump
    ffprobe -v error -show_format -show_streams -show_packets -of json "$f" > "$f.probe.json"
  done
  touch "$out/.done"
}

# ---------------------------------------------------------------------------------------------------------
# H.264 conformance matrix: x264-encoded streams for every stage of the decoder (rvp-codec-h264), in MP4.
# Synthetic but busy sources (testsrc2 with temporal noise), so all macroblock and partition types occur.
gen_h264() {
  local d="$out/h264"
  mkdir -p "$d"
  # enc <name> <WxH> <frames> <x264 ffmpeg args...>
  enc() {
    local name="$1" size="$2" frames="$3"; shift 3
    local dst="$d/$name.mp4"
    if [[ -s "$dst" && -z "${RVP_FIXTURE_FORCE:-}" ]]; then return; fi
    ff -f lavfi -i "testsrc2=size=$size:rate=30" -vf "noise=alls=${NOISE:-9}:allf=t,format=yuv420p" -frames:v "$frames" \
       -an -c:v libx264 -pix_fmt yuv420p -threads 1 "$@" -movflags +faststart "$dst"
  }
  local base=(-profile:v baseline -preset medium)
  local main=(-profile:v main -preset medium)
  local high=(-profile:v high -preset medium)
  # --- 6a: intra only, CAVLC (Baseline)
  enc i_base_cif 352x288 12 "${base[@]}" -g 1
  enc i_base_nodb 352x288 6 "${base[@]}" -g 1 -x264-params no-deblock=1
  enc i_base_odd 326x246 8 "${base[@]}" -g 1
  enc i_base_slices 352x288 8 "${base[@]}" -g 1 -x264-params slices=4
  enc i_base_q5 352x288 4 "${base[@]}" -g 1 -qp 5
  enc i_base_q45 352x288 4 "${base[@]}" -g 1 -qp 45
  NOISE=14 enc i_base_720p 1280x720 30 "${base[@]}" -g 1
  # --- 6b: P slices (Baseline)
  enc p_base_ref1 352x288 30 "${base[@]}" -g 30 -bf 0 -refs 1
  enc p_base_ref4 352x288 40 "${base[@]}" -g 40 -bf 0 -refs 4
  enc p_base_ref16 352x288 40 "${base[@]}" -g 60 -bf 0 -refs 16
  enc p_base_cip 352x288 30 "${base[@]}" -g 30 -bf 0 -refs 3 -x264-params constrained-intra=1
  enc p_base_slices 352x288 30 "${base[@]}" -g 30 -bf 0 -refs 2 -x264-params slices=4
  enc p_base_odd 326x246 30 "${base[@]}" -g 30 -bf 0 -refs 2
  enc p_base_nodb 352x288 20 "${base[@]}" -g 30 -bf 0 -refs 2 -x264-params no-deblock=1
  enc p_base_720p 1280x720 30 "${base[@]}" -g 30 -bf 0 -refs 2
  # --- 6c: B slices, weighted prediction, direct modes (Main, CAVLC)
  enc b_cavlc_spatial 352x288 40 "${main[@]}" -g 40 -bf 3 -refs 3 -x264-params cabac=0:direct=spatial:weightp=2
  enc b_cavlc_temporal 352x288 40 "${main[@]}" -g 40 -bf 3 -refs 3 -x264-params cabac=0:direct=temporal:weightp=2
  enc b_cavlc_pyramid 352x288 40 "${main[@]}" -g 40 -bf 5 -refs 4 -x264-params cabac=0:b-pyramid=normal:weightb=1:weightp=2
  enc b_cavlc_auto 352x288 40 "${main[@]}" -g 40 -bf 2 -refs 2 -x264-params cabac=0:direct=auto:b-adapt=2
  enc b_cavlc_slices 352x288 30 "${main[@]}" -g 30 -bf 3 -refs 3 -x264-params cabac=0:slices=3:weightb=1
  enc b_cavlc_odd 326x246 30 "${main[@]}" -g 30 -bf 3 -refs 3 -x264-params cabac=0:weightb=1
  # --- 6d: CABAC (Main)
  enc c_main_i 352x288 8 "${main[@]}" -g 1
  enc c_main_p 352x288 30 "${main[@]}" -g 30 -bf 0 -refs 3
  enc c_main_b 352x288 40 "${main[@]}" -g 40 -bf 3 -refs 3 -x264-params weightp=2:weightb=1
  enc c_main_temporal 352x288 40 "${main[@]}" -g 40 -bf 3 -refs 3 -x264-params direct=temporal
  enc c_main_pyramid 352x288 40 "${main[@]}" -g 40 -bf 5 -refs 4 -x264-params b-pyramid=normal
  enc c_main_slices 352x288 30 "${main[@]}" -g 30 -bf 3 -refs 3 -x264-params slices=4
  enc c_main_cip 352x288 30 "${main[@]}" -g 30 -bf 2 -refs 3 -x264-params constrained-intra=1
  enc c_main_odd 326x246 30 "${main[@]}" -g 30 -bf 3 -refs 3
  enc c_main_q8 352x288 12 "${main[@]}" -g 12 -bf 2 -qp 8
  enc c_main_q48 352x288 12 "${main[@]}" -g 12 -bf 2 -qp 48
  enc c_main_1080p 1920x1080 12 "${main[@]}" -g 12 -bf 2 -refs 2
  # --- 6e: High profile (8x8 transform, scaling matrices)
  enc h_high_i 352x288 8 "${high[@]}" -g 1
  enc h_high 352x288 40 "${high[@]}" -g 40 -bf 3 -refs 3 -x264-params weightp=2:weightb=1
  enc h_high_cavlc 352x288 40 "${high[@]}" -g 40 -bf 3 -refs 3 -x264-params cabac=0
  enc h_high_cqm_jvt 352x288 30 "${high[@]}" -g 30 -bf 3 -refs 3 -x264-params cqm=jvt
  enc h_high_cqm_flat 352x288 20 "${high[@]}" -g 30 -bf 2 -refs 2 -x264-params cqm=flat
  enc h_high_cqm_cavlc 352x288 20 "${high[@]}" -g 30 -bf 2 -refs 2 -x264-params cqm=jvt:cabac=0
  enc h_high_slices 352x288 30 "${high[@]}" -g 30 -bf 3 -refs 3 -x264-params slices=4
  enc h_high_odd 326x246 30 "${high[@]}" -g 30 -bf 3 -refs 3
  enc h_high_deblock 352x288 30 "${high[@]}" -g 30 -bf 3 -refs 3 -x264-params deblock=3,-3
  enc h_high_nodb 352x288 20 "${high[@]}" -g 30 -bf 3 -refs 3 -x264-params no-deblock=1
  enc h_high_refs16 352x288 40 "${high[@]}" -g 60 -bf 3 -refs 16 -x264-params b-pyramid=normal
  enc h_high_mbtree 352x288 60 "${high[@]}" -g 60 -bf 3 -refs 4 -x264-params rc-lookahead=30:aq-mode=2
  enc h_high_cip 352x288 30 "${high[@]}" -g 30 -bf 2 -refs 3 -x264-params constrained-intra=1
  NOISE=12 enc h_high_720p 1280x720 90 "${high[@]}" -g 60 -bf 3 -refs 3 -x264-params weightp=2:weightb=1
  enc h_high_1080p 1920x1080 20 "${high[@]}" -g 20 -bf 3 -refs 3
  # --- interlaced (rejected cleanly) and non-4:2:0 / high bit depth (rejected cleanly)
  enc x_interlaced 352x288 10 "${main[@]}" -g 10 -bf 0 -x264-params interlaced=1
  enc x_high10 352x288 6 -profile:v high10 -pix_fmt yuv420p10le -preset veryfast -g 6
  # A raw Annex B stream for the byte-stream path.
  if [[ ! -s "$d/h_high.264" || -n "${RVP_FIXTURE_FORCE:-}" ]]; then
    ff -i "$d/h_high.mp4" -c:v copy -bsf:v h264_mp4toannexb -f h264 "$d/h_high.264"
  fi
  touch "$out/.h264.done"
  echo "h264 fixtures in $d"
}

fixture_set="${RVP_FIXTURE_SET:-all}"
if [[ "$fixture_set" == all || "$fixture_set" == core ]]; then gen_core; fi
if [[ "$fixture_set" == all || "$fixture_set" == h264 ]]; then gen_h264; fi
echo "fixtures in $out"
