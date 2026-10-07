#!/usr/bin/env bash
# Generate small synthetic test media with ffmpeg into target/fixtures (never committed) and, next to each
# file, the ffprobe packet/stream dump (<name>.probe.json) used as the oracle by the demuxer tests.
#   tools/gen-fixtures.sh [outdir]      (default: <repo>/target/fixtures, or $RVP_FIXTURES)
#   RVP_FIXTURE_SET=basic|core|h264|vp9|m8|audio|library|levels|hevc|perf|all   which set to build (default all, which leaves out `perf`); H.264 goes to <outdir>/h264,
#                                       VP9 to <outdir>/vp9, M8 (subtitles, tracks, gapless) to <outdir>/m8, raw audio files to <outdir>/audio, and the one-minute 1080p30
#                                       speed streams (M9) to <outdir>/perf (minutes of encoding: `cargo xtask perf-fixtures`)
#   RVP_FIXTURE_FORCE=1                 rebuild files that already exist (the H.264 set otherwise skips them)
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-${RVP_FIXTURES:-$root/target/fixtures}}"
mkdir -p "$out"
command -v ffmpeg >/dev/null && command -v ffprobe >/dev/null || { echo "ffmpeg/ffprobe not found" >&2; exit 1; }
ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }

# Just the H.264 + AAC MP4 (needs only libx264 and the native AAC encoder: the desktop smoke tests and the PWA test use it where
# ffmpeg has no AV1 encoder, as in Ubuntu's package).
gen_basic() {
  ff -f lavfi -i "testsrc2=size=320x240:rate=25:duration=6" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=6" \
     -c:v libx264 -preset veryfast -g 12 -bf 2 -pix_fmt yuv420p -c:a aac -b:a 64k -ac 2 -shortest "$out/h264_aac.mp4"
}

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
  enc b_cavlc_spatial 352x288 40 "${main[@]}" -g 40 -bf 3 -refs 3 -x264-params cabac=0:direct=spatial:weightp=1
  enc b_cavlc_temporal 352x288 40 "${main[@]}" -g 40 -bf 3 -refs 3 -x264-params cabac=0:direct=temporal:weightp=1
  enc b_cavlc_pyramid 352x288 40 "${main[@]}" -g 40 -bf 5 -refs 4 -x264-params cabac=0:b-pyramid=normal:weightb=1:weightp=1
  enc b_cavlc_auto 352x288 40 "${main[@]}" -g 40 -bf 2 -refs 2 -x264-params cabac=0:direct=auto:b-adapt=2
  enc b_cavlc_slices 352x288 30 "${main[@]}" -g 30 -bf 3 -refs 3 -x264-params cabac=0:slices=3:weightb=1
  enc b_cavlc_odd 326x246 30 "${main[@]}" -g 30 -bf 3 -refs 3 -x264-params cabac=0:weightb=1
  # --- 6d: CABAC (Main)
  enc c_main_i 352x288 8 "${main[@]}" -g 1
  enc c_main_p 352x288 30 "${main[@]}" -g 30 -bf 0 -refs 3
  enc c_main_b 352x288 40 "${main[@]}" -g 40 -bf 3 -refs 3 -x264-params weightp=1:weightb=1
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
  enc h_high 352x288 40 "${high[@]}" -g 40 -bf 3 -refs 3 -x264-params weightp=1:weightb=1
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
  NOISE=12 enc h_high_720p 1280x720 90 "${high[@]}" -g 60 -bf 3 -refs 3 -x264-params weightp=1:weightb=1
  enc h_high_1080p 1920x1080 20 "${high[@]}" -g 20 -bf 3 -refs 3
  # Typical-bitrate content for the speed numbers (a few Mbit/s instead of the 25+ of the noisy streams above).
  NOISE=2 enc h_high_720p_typ 1280x720 90 "${high[@]}" -g 60 -bf 3 -refs 3 -crf 22 -x264-params weightp=1:weightb=1
  NOISE=2 enc h_high_1080p_typ 1920x1080 60 "${high[@]}" -g 60 -bf 3 -refs 3 -crf 22
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

# ---------------------------------------------------------------------------------------------------------
# VP9 conformance matrix: libvpx-encoded streams (profile 0 unless named otherwise), in WebM, video only unless named.
# Same busy synthetic source as the H.264 set. Resize streams are two encodes of different sizes joined at a key
# frame (ffmpeg's libvpx wrapper cannot do in-stream reference scaling).
gen_vp9() {
  local d="$out/vp9"
  mkdir -p "$d"
  # enc <name> <WxH> <frames> <ffmpeg args...>
  enc() {
    local name="$1" size="$2" frames="$3"; shift 3
    local dst="$d/$name.webm"
    if [[ -s "$dst" && -z "${RVP_FIXTURE_FORCE:-}" ]]; then return; fi
    ff -f lavfi -i "testsrc2=size=$size:rate=30" -vf "noise=alls=${NOISE:-9}:allf=t,format=${PIXFMT:-yuv420p}" -frames:v "$frames" \
       -an -c:v libvpx-vp9 -threads 1 -pix_fmt "${PIXFMT:-yuv420p}" "$@" "$dst"
  }
  # sizes
  enc s_64x64 64x64 20 -deadline good -cpu-used 2 -crf 30 -b:v 0 -g 10
  enc s_176x144 176x144 30 -deadline good -cpu-used 2 -crf 30 -b:v 0 -g 15
  enc s_352x288 352x288 30 -deadline good -cpu-used 2 -crf 30 -b:v 0 -g 15
  enc s_8x8 8x8 10 -deadline good -cpu-used 2 -crf 30 -b:v 0 -g 5
  enc s_odd_327x245 327x245 24 -deadline good -cpu-used 2 -crf 30 -b:v 0 -g 12
  enc s_odd_130x66 130x66 24 -deadline good -cpu-used 2 -crf 30 -b:v 0 -g 12
  enc s_odd_17x9 17x9 12 -deadline good -cpu-used 2 -crf 30 -b:v 0 -g 6
  NOISE=14 enc s_720p 1280x720 30 -deadline good -cpu-used 4 -crf 32 -b:v 0 -g 30
  NOISE=2 enc s_720p_typ 1280x720 60 -deadline good -cpu-used 4 -crf 33 -b:v 0 -g 60 -tile-columns 2
  NOISE=2 enc s_1080p_typ 1920x1080 60 -deadline good -cpu-used 4 -crf 33 -b:v 0 -g 60 -tile-columns 2 -tile-rows 1
  # settings (352x288)
  local base=(-deadline good -cpu-used 2 -crf 30 -b:v 0)
  enc t_altref 352x288 40 "${base[@]}" -g 40 -auto-alt-ref 1 -lag-in-frames 16 -arnr-maxframes 5 -arnr-strength 3
  enc t_rt 352x288 40 -deadline realtime -cpu-used 8 -b:v 400k -g 40
  enc t_best 352x288 20 -deadline best -crf 28 -b:v 0 -g 20
  enc t_errres 352x288 30 "${base[@]}" -g 15 -error-resilient default
  enc t_frameparallel 352x288 30 "${base[@]}" -g 15 -frame-parallel 1
  enc t_aq_cyclic 352x288 40 -deadline realtime -cpu-used 7 -aq-mode 3 -b:v 300k -g 40
  enc t_aq_variance 352x288 30 "${base[@]}" -g 30 -aq-mode 1
  enc t_aq_complexity 352x288 30 "${base[@]}" -g 30 -aq-mode 2
  enc t_lossless 352x288 12 -lossless 1 -g 12
  enc t_q4 352x288 12 -deadline good -cpu-used 2 -crf 4 -b:v 0 -g 12
  enc t_q63 352x288 20 -deadline good -cpu-used 2 -crf 63 -b:v 0 -g 20
  enc t_sharp7 352x288 20 "${base[@]}" -g 20 -sharpness 7
  enc t_static 352x288 30 "${base[@]}" -g 30 -static-thresh 800
  enc t_screen 352x288 20 "${base[@]}" -g 20 -tune-content screen
  enc t_gop1 352x288 10 "${base[@]}" -g 1
  enc t_cbr 352x288 40 -deadline realtime -cpu-used 6 -b:v 200k -minrate 200k -maxrate 200k -g 40
  enc t_tiles4 1280x720 20 -deadline realtime -cpu-used 8 -b:v 1500k -g 20 -tile-columns 2 -tile-rows 2
  # colour tags in the key frame: BT.709 full range, and BT.2020 limited
  enc c_bt709_full 352x288 10 -deadline good -cpu-used 4 -crf 30 -b:v 0 -g 10 -colorspace bt709 -color_range pc
  enc c_bt2020 352x288 10 -deadline good -cpu-used 4 -crf 30 -b:v 0 -g 10 -colorspace bt2020nc -color_range tv
  # profile 2 (10-bit 4:2:0) and profile 1 (4:4:4): decoded or rejected cleanly by the wrapper
  PIXFMT=yuv420p10le enc x_profile2_10bit 352x288 12 -profile:v 2 -deadline good -cpu-used 4 -crf 30 -b:v 0 -g 12
  PIXFMT=yuv444p enc x_profile1_444 352x288 12 -profile:v 1 -deadline good -cpu-used 4 -crf 30 -b:v 0 -g 12
  # VP9 + Opus in WebM (A/V playback)
  if [[ ! -s "$d/av_opus.webm" || -n "${RVP_FIXTURE_FORCE:-}" ]]; then
    ff -f lavfi -i "testsrc2=size=320x240:rate=25:duration=6" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=6" \
       -c:v libvpx-vp9 -threads 1 -deadline good -cpu-used 4 -crf 33 -b:v 0 -g 25 -pix_fmt yuv420p -c:a libopus -b:a 64k -ac 2 -shortest "$d/av_opus.webm"
  fi
  # Resize at a key frame: 320x240 then 480x270 then 200x120 (Matroska with a size change mid-stream).
  if [[ ! -s "$d/r_keyframe.ivf" || -n "${RVP_FIXTURE_FORCE:-}" ]]; then
    for part in "a 320x240" "b 480x270" "c 200x120"; do
      set -- $part
      ff -f lavfi -i "testsrc2=size=$2:rate=30" -vf "noise=alls=9:allf=t,format=yuv420p" -frames:v 15 -an -c:v libvpx-vp9 -threads 1 \
         -deadline good -cpu-used 4 -crf 32 -b:v 0 -g 15 -f ivf "$d/.r_$1.ivf"
    done
    python3 - "$d" <<'PY'
import struct, sys
d = sys.argv[1]
out = bytearray(); n = 0; hdr = None
for part in "abc":
    b = open(f"{d}/.r_{part}.ivf", "rb").read()
    if hdr is None: hdr = bytearray(b[:32])
    pos = 32
    while pos < len(b):
        sz = struct.unpack_from("<I", b, pos)[0]
        out += struct.pack("<I", sz) + struct.pack("<Q", n) + b[pos + 12 : pos + 12 + sz]
        pos += 12 + sz; n += 1
struct.pack_into("<I", hdr, 24, n)
open(f"{d}/r_keyframe.ivf", "wb").write(bytes(hdr) + bytes(out))
PY
    rm -f "$d"/.r_*.ivf
    ff -i "$d/r_keyframe.ivf" -c copy "$d/r_keyframe.webm"
  fi
  touch "$out/.vp9.done"
  echo "vp9 fixtures in $d"
}

# ---------------------------------------------------------------------------------------------------------
# M8: subtitles (embedded and sidecar), several audio tracks, gapless pieces, chapters.
gen_m8() {
  local d="$out/m8"
  mkdir -p "$d"
  # The marker holds a version, so adding fixtures to this set regenerates it once.
  local version=5
  [[ "$(cat "$d/.done" 2>/dev/null)" == "$version" && -z "${RVP_FIXTURE_FORCE:-}" ]] && return
  # Sidecar subtitle files with known timing (also the source of the embedded ones).
  cat > "$d/sub.srt" <<'SRT'
1
00:00:01,000 --> 00:00:02,000
Hello

2
00:00:03,000 --> 00:00:04,500
<i>World</i>
two lines

3
00:00:05,000 --> 00:00:05,500
Last one
SRT
  cat > "$d/sub_es.srt" <<'SRT'
1
00:00:01,000 --> 00:00:02,000
Hola

2
00:00:03,000 --> 00:00:04,500
Mundo
SRT
  cat > "$d/sub.vtt" <<'VTT'
WEBVTT

NOTE a comment

intro
00:00:01.000 --> 00:00:02.000 align:start
Hello

00:00:03.000 --> 00:00:04.500
<i>World</i>
two lines

00:00:05.000 --> 00:00:05.500
Last one
VTT
  local v=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=6")
  local a=(-f lavfi -i "sine=frequency=440:sample_rate=48000:duration=6")
  local h=(-c:v libx264 -preset veryfast -g 25 -pix_fmt yuv420p)
  # Embedded text subtitles: Matroska SRT (two languages) and WebVTT, MP4 mov_text and wvtt.
  ff "${v[@]}" "${a[@]}" -i "$d/sub.srt" -i "$d/sub_es.srt" -map 0 -map 1 -map 2 -map 3 "${h[@]}" -c:a aac -b:a 64k -c:s srt \
     -metadata:s:s:0 language=eng -metadata:s:s:1 language=spa "$d/subs_srt.mkv"
  ff "${v[@]}" "${a[@]}" -i "$d/sub.vtt" -map 0 -map 1 -map 2 "${h[@]}" -c:a aac -b:a 64k -c:s webvtt -metadata:s:s:0 language=eng "$d/subs_vtt.mkv"
  ff "${v[@]}" "${a[@]}" -i "$d/sub.srt" -map 0 -map 1 -map 2 "${h[@]}" -c:a aac -b:a 64k -c:s mov_text -metadata:s:s:0 language=eng "$d/subs_movtext.mp4"
  # ASS: a script with two styles (the second is bold italic yellow, top) and events with overrides (italic, bold, colour, \an9 and
  # \pos), as a sidecar and embedded in Matroska (S_TEXT/ASS).
  cat > "$d/sub.ass" <<'ASS'
[Script Info]
ScriptType: v4.00+
PlayResX: 320
PlayResY: 240
WrapStyle: 0

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,10,1
Style: Top,Arial,20,&H0000FFFF,&H000000FF,&H00000000,&H00000000,-1,-1,0,0,100,100,0,0,1,2,1,8,10,10,10,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,Hello
Dialogue: 0,0:00:03.00,0:00:04.50,Default,,0,0,0,,{\i1}World{\i0}\Ntwo lines
Dialogue: 1,0:00:03.50,0:00:05.00,Top,,0,0,0,,{\an9\pos(300,20)\c&H00FF00&}Corner
Dialogue: 0,0:00:05.00,0:00:05.50,Default,,0,0,0,,{\b1}Last{\b0} one, with a comma
ASS
  ff "${v[@]}" "${a[@]}" -i "$d/sub.ass" -map 0 -map 1 -map 2 "${h[@]}" -c:a aac -b:a 64k -c:s ass -metadata:s:s:0 language=eng "$d/subs_ass.mkv"
  # A cue that is already on screen when a seek lands: 24 s of video with a keyframe every second; "Long cue" runs from 8 s to 24 s
  # (further ahead than the demuxer reads while playing the first seconds), "Short" from 16 s to 17 s. Plain SRT and ASS, in Matroska.
  cat > "$d/long.srt" <<'SRT'
1
00:00:08,000 --> 00:00:24,000
Long cue

2
00:00:16,000 --> 00:00:17,000
Short
SRT
  # MP4 samples cannot overlap, so its text track gets the long cue alone.
  printf '1\n00:00:08,000 --> 00:00:24,000\nLong cue\n' > "$d/long1.srt"
  sed -e '/^Dialogue:/d' "$d/sub.ass" > "$d/long.ass"
  printf '%s\n' 'Dialogue: 0,0:00:08.00,0:00:24.00,Default,,0,0,0,,{\i1}Long cue' 'Dialogue: 0,0:00:16.00,0:00:17.00,Default,,0,0,0,,Short' >> "$d/long.ass"
  local lv=(-f lavfi -i "testsrc2=size=160x120:rate=25:duration=24") la=(-f lavfi -i "sine=frequency=440:sample_rate=48000:duration=24")
  local lh=(-c:v libx264 -preset veryfast -g 25 -pix_fmt yuv420p)
  ff "${lv[@]}" "${la[@]}" -i "$d/long.srt" -map 0 -map 1 -map 2 "${lh[@]}" -c:a aac -b:a 48k -c:s srt -metadata:s:s:0 language=eng "$d/subs_long.mkv"
  ff "${lv[@]}" "${la[@]}" -i "$d/long.ass" -map 0 -map 1 -map 2 "${lh[@]}" -c:a aac -b:a 48k -c:s ass -metadata:s:s:0 language=eng "$d/subs_long_ass.mkv"
  ff "${lv[@]}" "${la[@]}" -i "$d/long1.srt" -map 0 -map 1 -map 2 "${lh[@]}" -c:a aac -b:a 48k -c:s mov_text -metadata:s:s:0 language=eng "$d/subs_long.mp4"
  # PGS bitmap subtitles (no ffmpeg encoder: tools/gen-pgs.py writes a .sup stream by hand).
  python3 "$root/tools/gen-pgs.py" "$d/sub.sup"
  ff "${v[@]}" "${a[@]}" -f sup -i "$d/sub.sup" -map 0 -map 1 -map 2 "${h[@]}" -c:a aac -b:a 64k -c:s copy -metadata:s:s:0 language=eng "$d/subs_pgs.mkv"
  # Two seconds of picture with sound (a chain item that has both) and one with a picture only, in Matroska.
  ff "${v[@]}" "${a[@]}" -t 2 "${h[@]}" -c:a aac -b:a 64k "$d/av_2s.mkv"
  ff "${v[@]}" -f lavfi -i "sine=frequency=660:sample_rate=48000:duration=6" -t 2 "${h[@]}" -c:a libopus -b:a 64k -ac 2 "$d/av_2s_opus.mkv"
  # A video without audio (the next item of a gapless chain that has no sound).
  ff "${v[@]}" -t 2 "${h[@]}" -an "$d/video_only.mkv"
  ff "${v[@]}" -t 2 "${h[@]}" -an -movflags +faststart "$d/video_only.mp4"
  # Two audio tracks: 440 Hz (English) and 880 Hz (Spanish), Opus in Matroska, with video.
  ff "${v[@]}" "${a[@]}" -f lavfi -i "sine=frequency=880:sample_rate=48000:duration=6" -map 0 -map 1 -map 2 "${h[@]}" -c:a libopus -b:a 64k -ac 2 \
     -metadata:s:a:0 language=eng -metadata:s:a:1 language=spa "$d/two_audio.mkv"
  # Gapless pieces: three 2 s FLAC pieces (and Opus pieces) cut from one continuous 440 Hz sine (sample exact).
  ff -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=6" -c:a pcm_s16le "$d/sine_full.wav"
  for i in 0 1 2; do
    ff -ss $((i * 2)) -i "$d/sine_full.wav" -t 2 -c:a flac "$d/gap_$i.mkv"
    ff -ss $((i * 2)) -i "$d/sine_full.wav" -t 2 -c:a libopus -b:a 96k "$d/gap_opus_$i.webm"
    ff -ss $((i * 2)) -i "$d/sine_full.wav" -t 2 -c:a aac -b:a 96k "$d/gap_aac_$i.mp4"
  done
  # Chapters.
  cat > "$d/chapters.txt" <<'CH'
;FFMETADATA1
[CHAPTER]
TIMEBASE=1/1000
START=0
END=2000
title=Intro
[CHAPTER]
TIMEBASE=1/1000
START=2000
END=4000
title=Middle
[CHAPTER]
TIMEBASE=1/1000
START=4000
END=6000
title=End
CH
  ff "${v[@]}" "${a[@]}" -i "$d/chapters.txt" -map_metadata 2 -map_chapters 2 -map 0 -map 1 "${h[@]}" -c:a aac -b:a 64k "$d/chapters.mkv"
  ff "${v[@]}" "${a[@]}" -i "$d/chapters.txt" -map_metadata 2 -map_chapters 2 -map 0 -map 1 "${h[@]}" -c:a aac -b:a 64k "$d/chapters.mp4"
  # Tags and cover art in audio-only MP4/MKV for the now-playing model.
  ff "${a[@]}" -f lavfi -i "color=c=magenta:size=64x64:d=1,format=yuvj420p" -frames:v 1 "$d/cover.jpg"
  ff "${a[@]}" -i "$d/cover.jpg" -map 0:a -map 1:v -c:a aac -b:a 64k -c:v mjpeg -disposition:v:0 attached_pic \
     -metadata title="Sine Song" -metadata artist="The Tones" -metadata album="Pure" "$d/tagged.m4a"
  ff "${a[@]}" -c:a libopus -b:a 64k -metadata title="Sine Song" -metadata artist="The Tones" -metadata album="Pure" "$d/tagged.webm"
  ff "${a[@]}" -c:a libopus -b:a 64k -metadata title="Sine Song" -metadata artist="The Tones" -metadata album="Pure" \
     -attach "$d/cover.jpg" -metadata:s:t:0 mimetype=image/jpeg -metadata:s:t:0 filename=cover.jpg "$d/tagged_art.mka"
  # Clicks at 120 bpm (4 ms bursts of 2 kHz every 0.5 s, 8 s) for the visualizer's onset and tempo tests.
  ff -f lavfi -i "aevalsrc='if(lt(mod(t,0.5),0.004), 0.8*sin(2*PI*2000*t), 0)':s=48000:d=8" -c:a flac "$d/clicks_120.mkv"
  echo "$version" > "$d/.done"
  echo "m8 fixtures in $d"
}

# ---------------------------------------------------------------------------------------------------------
# M9: one minute of 1080p30 per codec for the real-time tests in the browser (video plus a sine on AAC/Opus).
gen_perf() {
  local d="$out/perf"
  mkdir -p "$d"
  local version=1
  [[ "$(cat "$d/.done" 2>/dev/null)" == "$version" && -z "${RVP_FIXTURE_FORCE:-}" ]] && return
  local frames=1800 a=(-f lavfi -i "sine=frequency=440:sample_rate=48000:duration=60")
  # src <noise>: busy synthetic picture (testsrc2 plus temporal noise); noise 2 gives a few Mbit/s, 9 about 25 Mbit/s.
  # The audio is input 0 and the picture input 1; the filter and frame count are output options.
  src() { echo -vf "noise=alls=$1:allf=t,format=yuv420p" -frames:v "$frames" -map 1:v -map 0:a; }
  local vin=(-f lavfi -i "testsrc2=size=1920x1080:rate=30")
  local high=(-c:v libx264 -profile:v high -preset medium -g 60 -bf 3 -refs 3 -pix_fmt yuv420p -threads 4)
  ff "${a[@]}" "${vin[@]}" $(src 2) "${high[@]}" -crf 22 -c:a aac -b:a 96k -shortest -movflags +faststart "$d/h264_1080p30_typ.mp4"
  ff "${a[@]}" "${vin[@]}" $(src 9) "${high[@]}" -crf 18 -x264-params vbv-maxrate=25000:vbv-bufsize=25000 -c:a aac -b:a 96k -shortest -movflags +faststart "$d/h264_1080p30_stress.mp4"
  ff "${a[@]}" "${vin[@]}" $(src 2) -c:v libvpx-vp9 -deadline good -cpu-used 4 -crf 33 -b:v 0 -g 60 -tile-columns 2 -row-mt 1 -threads 8 \
     -c:a libopus -b:a 96k -shortest "$d/vp9_1080p30.webm"
  ff "${a[@]}" "${vin[@]}" $(src 2) -c:v libsvtav1 -preset 10 -crf 36 -g 60 -svtav1-params log=0 -c:a libopus -b:a 96k -shortest "$d/av1_1080p30.webm"
  echo "$version" > "$d/.done"
  echo "perf fixtures in $d"
}

# ---------------------------------------------------------------------------------------------------------
# M9: raw audio files (MP3, FLAC, Ogg, Opus, WAV, ADTS AAC) with tags and cover art, plus the ffprobe dump of each.
gen_audio() {
  local d="$out/audio"
  mkdir -p "$d"
  local version=4
  [[ "$(cat "$d/.done" 2>/dev/null)" == "$version" && -z "${RVP_FIXTURE_FORCE:-}" ]] && return
  # 3.3 s at 48 kHz (the player's output rate, so no resampling gets in the way of comparing samples): not a whole number of
  # frames in any codec, so the end padding matters. A tone that changes pitch makes
  # a wrong seek or a wrong gapless trim audible in the samples.
  local a=(-f lavfi -i "aevalsrc=0.5*sin(2*PI*(300+100*t)*t)|0.5*sin(2*PI*(500+60*t)*t):s=48000:d=3.3")
  # A 64x64 cover: a gradient, as JPEG.
  ff -f lavfi -i "gradients=size=64x64:duration=1:rate=1" -frames:v 1 "$d/cover.jpg"
  local tags=(-metadata title="Chirp Étude" -metadata artist="The Tones" -metadata album="Pure" -metadata album_artist="Various" \
              -metadata track=3/12 -metadata disc=1/2 -metadata date=2004-05-06 -metadata genre=Electronic)
  local art=(-i "$d/cover.jpg" -map 0:a -map 1:v -disposition:v attached_pic -c:v copy)
  # MP3: CBR with Xing/LAME header and ID3v2.4 (UTF-8) tags and art; VBR; ID3v2.3; ID3v1 only; no tags and no Xing header.
  ff "${a[@]}" "${art[@]}" -c:a libmp3lame -b:a 128k "${tags[@]}" -id3v2_version 4 "$d/cbr.mp3"
  ff "${a[@]}" "${art[@]}" -c:a libmp3lame -q:a 4 "${tags[@]}" -id3v2_version 3 "$d/vbr_v23.mp3"
  ff "${a[@]}" -c:a libmp3lame -b:a 96k -ac 1 -ar 22050 "${tags[@]}" -id3v2_version 0 -write_id3v1 1 "$d/mono_v1.mp3"
  ff "${a[@]}" -c:a libmp3lame -b:a 192k -write_xing 0 -map_metadata -1 "$d/plain.mp3"
  # ID3v1 only: the plain file with a 128-byte tag appended (ffmpeg does not write them): title, artist, album, year, track 7, genre 17.
  { cat "$d/plain.mp3"; printf 'TAG'; printf '%-30s%-30s%-30s%-4s%-28s\0\007\021' "V1 Title" "V1 Artist" "V1 Album" 1999 ""; } > "$d/v1.mp3"
  # FLAC (tags and picture), Ogg Vorbis (tags and picture), Opus (tags), FLAC in Ogg.
  ff "${a[@]}" "${art[@]}" -c:a flac "${tags[@]}" "$d/tone.flac"
  ff "${a[@]}" -c:a flac -sample_fmt s32 -bits_per_raw_sample 24 -ac 1 "$d/tone24_mono.flac"
  ff "${a[@]}" -c:a libvorbis -q:a 4 "${tags[@]}" "$d/tone.ogg"
  ff "${a[@]}" -c:a libopus -b:a 96k "${tags[@]}" "$d/tone.opus"
  ff "${a[@]}" -c:a flac "${tags[@]}" -f ogg "$d/tone_flac.oga"
  # WAV: 16-bit (with LIST/INFO tags), 24-bit, float, 8-bit mono.
  ff "${a[@]}" -c:a pcm_s16le "${tags[@]}" "$d/tone16.wav"
  ff "${a[@]}" -c:a pcm_s24le "$d/tone24.wav"
  ff "${a[@]}" -c:a pcm_f32le "$d/tonef32.wav"
  ff "${a[@]}" -c:a pcm_u8 -ac 1 "$d/tone8_mono.wav"
  # ADTS AAC.
  ff "${a[@]}" -c:a aac -b:a 96k -f adts "$d/tone.aac"
  # MPEG audio layer II: MPEG 1 stereo, and MPEG 2 (LSF) mono at 24 kHz. (ffmpeg has no layer I encoder.)
  ff "${a[@]}" -c:a mp2 -b:a 192k "$d/tone.mp2"
  ff "${a[@]}" -c:a mp2 -b:a 64k -ac 1 -ar 24000 "$d/tone_lsf_mono.mp2"
  # Multichannel: 5.1 and 7.1, one tone per channel (the order is FL FR FC LFE BL BR [SL SR]), as FLAC and PCM.
  local s51=(-f lavfi -i "aevalsrc=0.3*sin(2*PI*440*t)|0.3*sin(2*PI*554*t)|0.3*sin(2*PI*660*t)|0.3*sin(2*PI*80*t)|0.3*sin(2*PI*880*t)|0.3*sin(2*PI*990*t):s=48000:d=2:c=5.1")
  local s71=(-f lavfi -i "aevalsrc=0.2*sin(2*PI*440*t)|0.2*sin(2*PI*554*t)|0.2*sin(2*PI*660*t)|0.2*sin(2*PI*80*t)|0.2*sin(2*PI*880*t)|0.2*sin(2*PI*990*t)|0.2*sin(2*PI*1200*t)|0.2*sin(2*PI*1500*t):s=48000:d=2:c=7.1")
  ff "${s51[@]}" -c:a flac "$d/surround51.flac"
  ff "${s51[@]}" -c:a pcm_s16le "$d/surround51.wav"
  ff "${s51[@]}" -c:a pcm_s24le "$d/surround51_24.wav"
  ff "${s71[@]}" -c:a flac "$d/surround71.flac"
  ff "${s71[@]}" -c:a pcm_f32le "$d/surround71_f32.wav"
  ff "${s51[@]}" -c:a aac -b:a 384k "$d/aac51.m4a"
  # AAC 5.1 next to a picture: symphonia 0.6 cannot decode it, so the video must still play (without sound) and say why.
  ff -f lavfi -i "testsrc2=size=160x120:rate=25:duration=2" "${s51[@]}" -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 384k -shortest "$d/video_aac51.mp4"
  # Chained Ogg: two complete streams one after the other (different tones), Vorbis and Opus.
  local c1=(-f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1.5") c2=(-f lavfi -i "sine=frequency=880:sample_rate=48000:duration=1.2")
  ff "${c1[@]}" -c:a libvorbis -q:a 4 -ac 2 "$d/chain_a.ogg"
  ff "${c2[@]}" -c:a libvorbis -q:a 4 -ac 2 "$d/chain_b.ogg"
  cat "$d/chain_a.ogg" "$d/chain_b.ogg" > "$d/chained.ogg"
  ff "${c1[@]}" -c:a libopus -b:a 64k -ac 2 "$d/chain_a.opus"
  ff "${c2[@]}" -c:a libopus -b:a 64k -ac 2 "$d/chain_b.opus"
  cat "$d/chain_a.opus" "$d/chain_b.opus" > "$d/chained.opus"
  ff "${c1[@]}" -c:a flac -ac 2 -f ogg "$d/chain_a.oga"
  ff "${c2[@]}" -c:a flac -ac 2 -f ogg "$d/chain_b.oga"
  cat "$d/chain_a.oga" "$d/chain_b.oga" > "$d/chained.oga"
  for f in "$d"/*.mp3 "$d"/*.mp2 "$d"/*.flac "$d"/*.ogg "$d"/*.opus "$d"/*.oga "$d"/*.wav "$d"/*.aac; do
    ffprobe -v error -show_format -show_streams -show_packets -of json "$f" > "$f.probe.json"
  done
  echo "$version" > "$d/.done"
}

# M10: a 200-track library in mixed formats with art, Unicode and odd tags, plus expected.json (the tree a scan must find).
gen_library() {
  python3 "$root/tools/gen-library.py" "$out/library"
  # A small library with real lengths, colourful covers and music with a beat, for the screenshots.
  python3 "$root/tools/gen-library.py" --showcase "$out/showcase"
}

# Loudness (automatic level, crossfade): programme-like signals at known levels with ffmpeg's EBU R128 reading of each in
# <name>.ebur128 (integrated loudness, true peak), files that carry ReplayGain and Opus R128 tags in every container, and a small
# library (an album of tracks 8 dB apart, one tagged track) for the scan.
gen_levels() {
  local d="$out/levels"
  mkdir -p "$d/lib/quiet-loud" "$d/lib/tagged"
  local version=6
  [[ "$(cat "$d/.done" 2>/dev/null)" == "$version" && -z "${RVP_FIXTURE_FORCE:-}" ]] && return
  # ffmpeg's reading of the whole file as `lavfi.r128.I=<LUFS>` and `lavfi.r128.true_peak=<linear>` lines (the last values the
  # filter reports, three decimals).
  ebur() {
    local raw; raw="$(mktemp)"
    ffmpeg -hide_banner -nostats -loglevel error -i "$1" \
      -af "${3:-anull},ebur128=metadata=1:peak=true,ametadata=mode=print:file=$raw" -f null -
    tail -n 12 "$raw" | grep -E '^lavfi.r128.(I|true_peak)=' > "$2"
    rm -f "$raw"
  }
  # 1 kHz sine, both channels, -23 LUFS (0.0708 peak): EBU Tech 3341 test 1; and -14 LUFS.
  ff -f lavfi -i "aevalsrc=0.07079*sin(2*PI*1000*t)|0.07079*sin(2*PI*1000*t):s=48000:d=20" -c:a pcm_s24le "$d/tone_m23.wav"
  ff -f lavfi -i "aevalsrc=0.19953*sin(2*PI*1000*t)|0.19953*sin(2*PI*1000*t):s=48000:d=10" -c:a pcm_s24le "$d/tone_m14.wav"
  # Programme-like: tones with a pulsing envelope and a click track, different in the two channels.
  local music="0.35*sin(2*PI*220*t)*(0.6+0.4*sin(2*PI*1.5*t))+0.15*sin(2*PI*1760*t)*lt(mod(t\,0.5)\,0.1)+0.1*sin(2*PI*55*t)|0.3*sin(2*PI*330*t)*(0.6+0.4*sin(2*PI*1.2*t))+0.15*sin(2*PI*2093*t)*lt(mod(t\,0.4)\,0.1)+0.1*sin(2*PI*82*t)"
  ff -f lavfi -i "aevalsrc=$music:s=48000:d=30" -c:a flac "$d/music.flac"
  # Loud for ten seconds, quiet for ten (the gate must follow the loud part), 44.1 kHz.
  ff -f lavfi -i "aevalsrc=0.5*sin(2*PI*300*t)*if(lt(mod(t\,20)\,10)\,1\,0.04)|0.4*sin(2*PI*500*t)*if(lt(mod(t\,20)\,10)\,1\,0.04):s=44100:d=30" -c:a flac "$d/dynamic_44k.flac"
  # Mono at 22.05 kHz (played to both speakers).
  ff -f lavfi -i "aevalsrc=0.2*sin(2*PI*440*t)*(0.5+0.5*sin(2*PI*2*t)):s=22050:d=12" -c:a flac "$d/mono_22k.flac"
  # The same programme through lossy codecs: the readings of two decoders must agree.
  ff -i "$d/music.flac" -c:a libmp3lame -b:a 160k "$d/music.mp3"
  ff -i "$d/music.flac" -c:a aac -b:a 128k "$d/music.m4a"
  ff -i "$d/music.flac" -c:a libopus -b:a 96k "$d/music.opus"
  ff -i "$d/music.flac" -c:a libvorbis -q:a 4 "$d/music.ogg"
  for f in tone_m23.wav tone_m14.wav music.flac dynamic_44k.flac music.mp3 music.m4a music.opus music.ogg; do ebur "$d/$f" "$d/$f.ebur128"; done
  ebur "$d/mono_22k.flac" "$d/mono_22k.flac.ebur128" "pan=stereo|c0=c0|c1=c0"
  # Tags: ReplayGain (track -6.50 dB, album -3.20 dB) in every container; Opus gets R128 gains instead; the gapless-album flag.
  local t=(-f lavfi -i "sine=frequency=440:sample_rate=48000:duration=2")
  local rg=(-metadata REPLAYGAIN_TRACK_GAIN="-6.50 dB" -metadata REPLAYGAIN_TRACK_PEAK=0.977000 \
            -metadata REPLAYGAIN_ALBUM_GAIN="-3.20 dB" -metadata REPLAYGAIN_ALBUM_PEAK=1.000000 -metadata title=Tagged -metadata album=Tags)
  ff "${t[@]}" -c:a libmp3lame -b:a 128k "${rg[@]}" -metadata iTunPGAP=1 -id3v2_version 3 "$d/tagged_v23.mp3"
  ff "${t[@]}" -c:a libmp3lame -b:a 128k "${rg[@]}" -id3v2_version 4 "$d/tagged_v24.mp3"
  ff "${t[@]}" -c:a flac "${rg[@]}" "$d/tagged.flac"
  ff "${t[@]}" -c:a libvorbis -q:a 3 "${rg[@]}" "$d/tagged.ogg"
  ff "${t[@]}" -c:a libopus -b:a 64k -metadata R128_TRACK_GAIN=-512 -metadata R128_ALBUM_GAIN=256 -metadata title=Tagged -metadata album=Tags "$d/tagged.opus"
  # M4A twice: QuickTime `mdta` keys (what ffmpeg writes), and iTunes-style free-form `----` items plus `pgap` (what iTunes, foobar2000
  # and MP3Tag write, put in by tools/m4a-freeform.py).
  ff "${t[@]}" -c:a aac -b:a 64k "${rg[@]}" -metadata gapless_playback=1 -movflags use_metadata_tags "$d/tagged_mdta.m4a"
  ff "${t[@]}" -c:a aac -b:a 64k -metadata title=Tagged -metadata album=Tags "$d/plain_for_tags.m4a"
  python3 "$root/tools/m4a-freeform.py" "$d/plain_for_tags.m4a" "$d/tagged_itunes.m4a" replaygain_track_gain="-6.50 dB" \
    replaygain_track_peak=0.977000 replaygain_album_gain="-3.20 dB" replaygain_album_peak=1.000000 --pgap
  rm -f "$d/plain_for_tags.m4a"
  ff "${t[@]}" -c:a flac "${rg[@]}" "$d/tagged.mka"
  ff "${t[@]}" -c:a pcm_s16le "$d/untagged.wav"
  # The same tags on a track long enough to change settings while it plays (the browser tests).
  ff -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=40" -c:a flac "${rg[@]}" "$d/tagged_long.flac"
  # A small library: an album whose tracks sit 8 LU apart (no tags: the scan measures them), and one tagged track.
  local alb=(-metadata album="Quiet and Loud" -metadata artist="Levels")
  local n=1
  for g in 0.02 0.0504 0.1267; do
    ff -f lavfi -i "aevalsrc=$g*sin(2*PI*$((200+n*110))*t)*(0.7+0.3*sin(2*PI*2*t))|$g*sin(2*PI*$((300+n*90))*t)*(0.7+0.3*sin(2*PI*1.7*t)):s=44100:d=12" \
       -c:a flac "${alb[@]}" -metadata title="Track $n" -metadata track=$n "$d/lib/quiet-loud/0$n.flac"
    ebur "$d/lib/quiet-loud/0$n.flac" "$d/lib/quiet-loud/0$n.flac.ebur128"
    n=$((n+1))
  done
  ff "${t[@]}" -c:a libmp3lame -b:a 128k "${rg[@]}" -metadata artist=Tagger -id3v2_version 4 "$d/lib/tagged/01.mp3"
  # Crossfade: tones of known pitch in FLAC (the decode is exact), a short one and a tiny one, three tracks of an album flagged
  # as gapless (the first two consecutive), and a video with sound.
  tone() { local f=$1 hz=$2 amp=$3 dur=$4; shift 4
    ff -f lavfi -i "aevalsrc=$amp*sin(2*PI*$hz*t)|$amp*sin(2*PI*$hz*t):s=48000:d=$dur" -c:a flac "$@" "$d/$f"; }
  tone xf_a.flac 440 0.4 8
  tone xf_b.flac 880 0.4 8
  tone xf_c.flac 1320 0.4 8
  tone xf_short.flac 600 0.4 1.5
  tone xf_tiny.flac 700 0.4 0.6
  tone xf_long_a.flac 440 0.4 40
  tone xf_long_b.flac 880 0.4 40
  tone gl_1.flac 440 0.4 6 -metadata album=Live -metadata track=1 -metadata ITUNPGAP=1
  tone gl_2.flac 880 0.4 6 -metadata album=Live -metadata track=2 -metadata ITUNPGAP=1
  tone gl_5.flac 1320 0.4 6 -metadata album=Live -metadata track=5 -metadata ITUNPGAP=1
  ff -f lavfi -i "testsrc2=size=160x120:rate=25:duration=5" -f lavfi -i "sine=frequency=1000:sample_rate=48000:duration=5" \
     -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 64k -shortest "$d/xf_video.mp4"
  echo "$version" > "$d/.done"
}

# HEVC (Main and Main10) and 10-bit H.264, which no decoder of ours takes: the platform decoder handoff and its messages are tested on
# them (needs libx265; without it the set is skipped and the tests that use it skip).
gen_hevc() {
  local d="$out/hevc"; mkdir -p "$d"
  [[ -f "$d/.done" && -z "${RVP_FIXTURE_FORCE:-}" ]] && return 0
  local v=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=4") a=(-f lavfi -i "sine=frequency=440:sample_rate=48000:duration=4")
  local enc; enc="$(ffmpeg -hide_banner -encoders 2>/dev/null || true)"
  [[ "$enc" == *libx265* ]] || { echo "libx265 missing: no HEVC fixtures"; return 0; }
  ff "${v[@]}" "${a[@]}" -c:v libx265 -preset veryfast -x265-params log-level=error:keyint=12 -pix_fmt yuv420p -tag:v hvc1 -c:a aac -b:a 64k -ac 2 -shortest "$d/hevc_aac.mp4"
  ff "${v[@]}" "${a[@]}" -c:v libx265 -preset veryfast -x265-params log-level=error:keyint=12 -pix_fmt yuv420p10le -tag:v hvc1 -c:a aac -b:a 64k -ac 2 -shortest "$d/hevc10_aac.mp4"
  ff "${v[@]}" "${a[@]}" -c:v libx265 -preset veryfast -x265-params log-level=error:keyint=12 -pix_fmt yuv420p -c:a flac -ac 2 -shortest "$d/hevc_flac.mkv"
  ff "${v[@]}" "${a[@]}" -c:v libx264 -preset veryfast -g 12 -pix_fmt yuv420p10le -c:a aac -b:a 64k -ac 2 -shortest "$d/h264_10bit.mp4"
  # Streams that use more of the standard: several slices, weighted prediction and scaling lists, open GOP with RASL pictures, many
  # reference pictures with long B pyramids, a larger CTB, tiny and odd sizes (cropping).
  local n=(-f lavfi -i "testsrc2=size=640x360:rate=25:duration=5,noise=alls=12:allf=t")
  local x=(-c:v libx265 -preset veryfast -pix_fmt yuv420p -tag:v hvc1 -an)
  ff "${n[@]}" "${x[@]}" -x265-params log-level=error:keyint=25:slices=4 "$d/hevc_slices.mp4"
  ff "${n[@]}" "${x[@]}" -x265-params log-level=error:keyint=25:weightp=1:weightb=1:scaling-list=default "$d/hevc_weighted.mp4"
  ff "${n[@]}" "${x[@]}" -x265-params log-level=error:keyint=30:min-keyint=10:open-gop=1:bframes=8:ref=5:b-pyramid=1 "$d/hevc_opengop.mp4"
  ff "${n[@]}" "${x[@]}" -x265-params log-level=error:keyint=50:ctu=32:rd=3:cu-lossless=0:lookahead-slices=0 "$d/hevc_ctu32.mp4"
  ff -f lavfi -i "testsrc2=size=202x118:rate=25:duration=3,noise=alls=12:allf=t" "${x[@]}" -x265-params log-level=error:keyint=12 "$d/hevc_odd.mp4"
  ff "${n[@]}" -c:v libx265 -preset veryfast -pix_fmt yuv420p10le -tag:v hvc1 -an -x265-params log-level=error:keyint=25:slices=3:weightp=1 "$d/hevc10_slices.mp4"
  # HDR10 (PQ, BT.2020) and HLG, Main 10.
  ff "${n[@]}" -c:v libx265 -preset veryfast -pix_fmt yuv420p10le -tag:v hvc1 -an -x265-params "log-level=error:keyint=25:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited" "$d/hevc10_hdr.mp4"
  ff "${n[@]}" -c:v libx265 -preset veryfast -pix_fmt yuv420p10le -tag:v hvc1 -an -x265-params "log-level=error:keyint=25:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc:range=limited" "$d/hevc10_hlg.mp4"
  touch "$out/hevc/.done"
}

# HEVC conformance matrix for the software decoder: small x265 streams that switch on one family of tools at a time, checked frame by
# frame against ffmpeg's decode (crates/rvp-codec-hevc/tests/conformance.rs).
gen_hevcconf() {
  local d="$out/hevcconf"; mkdir -p "$d"
  [[ -f "$d/.done" && -z "${RVP_FIXTURE_FORCE:-}" ]] && return 0
  local enc; enc="$(ffmpeg -hide_banner -encoders 2>/dev/null || true)"
  [[ "$enc" == *libx265* ]] || { echo "libx265 missing: no HEVC conformance fixtures"; return 0; }
  # name | size | frames | pix_fmt | x265 params
  local src8="testsrc2=size=%s:rate=25:duration=%s,noise=alls=10:allf=t"
  e() { # name size frames fmt params
    local name=$1 size=$2 frames=$3 fmt=$4 params=$5
    ff -f lavfi -i "$(printf "$src8" "$size" "$(python3 -c "print($frames/25)")")" -c:v libx265 -preset veryfast -pix_fmt "$fmt" -tag:v hvc1 -an \
       -x265-params "log-level=error:$params" "$d/$name.mp4"
  }
  local base="no-deblock=1:no-sao=1:keyint=1:bframes=0:ref=1:rc-lookahead=0:scenecut=0"
  e i_ctu64 200x136 6 yuv420p "$base:ctu=64"
  e i_ctu32 200x136 6 yuv420p "$base:ctu=32"
  e i_ctu16 200x136 6 yuv420p "$base:ctu=16"
  e i_ctu32_mincu16 208x144 4 yuv420p "$base:ctu=32:min-cu-size=16"
  e i_tskip 200x136 4 yuv420p "$base:ctu=32:tskip=1:tskip-fast=0"
  e i_nosign 200x136 4 yuv420p "$base:ctu=32:no-signhide=1"
  e i_qp_low 200x136 4 yuv420p "$base:ctu=32:qp=8:rc-lookahead=0:crf=8"
  e i_qp_high 200x136 4 yuv420p "$base:ctu=32:crf=40"
  e i_cuqp 200x136 4 yuv420p "$base:ctu=32:aq-mode=3:cutree=0"
  e i_10bit 200x136 4 yuv420p10le "$base:ctu=32"
  e i_constrained 200x136 4 yuv420p "$base:ctu=32:constrained-intra=1"
  e i_nostrong 200x136 4 yuv420p "$base:ctu=32:no-strong-intra-smoothing=1"
  e i_scaling 200x136 4 yuv420p "$base:ctu=32:scaling-list=default"
  e i_lossless 96x64 3 yuv420p "$base:ctu=16:lossless=1"
  e i_cu_lossless 200x136 4 yuv420p "$base:ctu=32:cu-lossless=1"
  e i_slices 200x136 4 yuv420p "$base:ctu=32:slices=4"
  e i_wpp 200x136 4 yuv420p "$base:ctu=32:wpp=1"
  e i_nowpp 200x136 4 yuv420p "$base:ctu=32:wpp=0"
  # Loop filters (intra pictures first).
  local nof="no-sao=1:keyint=1:bframes=0:ref=1:rc-lookahead=0:scenecut=0"
  e f_dbk 200x136 4 yuv420p "$nof:ctu=32"
  e f_dbk_ctu64 200x136 4 yuv420p "$nof:ctu=64"
  e f_dbk_ctu16 200x136 4 yuv420p "$nof:ctu=16"
  e f_dbk_offsets 200x136 4 yuv420p "$nof:ctu=32:deblock=-3,2"
  e f_dbk_slices 200x136 4 yuv420p "$nof:ctu=32:slices=4"
  e f_dbk_10bit 200x136 4 yuv420p10le "$nof:ctu=32"
  e f_dbk_lossless 200x136 4 yuv420p "$nof:ctu=32:cu-lossless=1"
  e f_dbk_qp 200x136 4 yuv420p "$nof:ctu=32:crf=36:aq-mode=3"
  local nof2="no-deblock=1:keyint=1:bframes=0:ref=1:rc-lookahead=0:scenecut=0"
  e s_sao 200x136 4 yuv420p "$nof2:ctu=32"
  e s_sao_ctu64 200x136 4 yuv420p "$nof2:ctu=64"
  e s_sao_ctu16 200x136 4 yuv420p "$nof2:ctu=16"
  e s_sao_10bit 200x136 4 yuv420p10le "$nof2:ctu=32"
  e s_sao_both 200x136 4 yuv420p "keyint=1:bframes=0:ref=1:rc-lookahead=0:scenecut=0:ctu=32"
  e s_sao_both_10bit 200x136 4 yuv420p10le "keyint=1:bframes=0:ref=1:rc-lookahead=0:scenecut=0:ctu=32:limit-sao=0"
  # Inter pictures: P and B, references, partitions, weighting, with and without the loop filters.
  local nl="no-deblock=1:no-sao=1:scenecut=0:rc-lookahead=0"
  local nlb="no-deblock=1:no-sao=1:scenecut=0:rc-lookahead=24"
  e p_ref1 200x136 12 yuv420p "$nl:keyint=60:bframes=0:ref=1:ctu=32"
  e p_ref4 200x136 12 yuv420p "$nl:keyint=60:bframes=0:ref=4:ctu=32"
  e p_ctu64 200x136 12 yuv420p "$nl:keyint=60:bframes=0:ref=2:ctu=64"
  e p_ctu16 200x136 12 yuv420p "$nl:keyint=60:bframes=0:ref=2:ctu=16"
  e p_amp 200x136 12 yuv420p "$nl:keyint=60:bframes=0:ref=2:ctu=32:rect=1:amp=1"
  e p_10bit 200x136 12 yuv420p10le "$nl:keyint=60:bframes=0:ref=2:ctu=32"
  e b_basic 200x136 16 yuv420p "$nlb:keyint=60:bframes=3:ref=2:ctu=32:b-pyramid=0"
  e b_pyramid 200x136 20 yuv420p "$nlb:keyint=60:bframes=8:ref=4:ctu=32:b-pyramid=1"
  e b_amp 200x136 16 yuv420p "$nlb:keyint=60:bframes=4:ref=3:ctu=32:rect=1:amp=1"
  e b_10bit 200x136 16 yuv420p10le "$nlb:keyint=60:bframes=4:ref=3:ctu=32"
  e b_ctu64 200x136 16 yuv420p "$nlb:keyint=60:bframes=4:ref=3:ctu=64"
  e b_ctu16 200x136 16 yuv420p "$nlb:keyint=60:bframes=4:ref=3:ctu=16"
  e b_nomerge 200x136 16 yuv420p "$nlb:keyint=60:bframes=4:ref=3:ctu=32:max-merge=1"
  e b_merge3 200x136 16 yuv420p "$nlb:keyint=60:bframes=4:ref=3:ctu=32:max-merge=3"
  e b_openGop 200x136 24 yuv420p "$nlb:keyint=12:min-keyint=6:bframes=4:ref=3:ctu=32:open-gop=1"
  e b_cutree 200x136 20 yuv420p "$nlb:keyint=60:bframes=6:ref=3:ctu=32:cutree=1:rc-lookahead=10"
  e p_dbk_sao 200x136 12 yuv420p "scenecut=0:rc-lookahead=0:keyint=60:bframes=0:ref=2:ctu=32"
  e b_dbk_sao 200x136 16 yuv420p "scenecut=0:rc-lookahead=24:keyint=60:bframes=4:ref=3:ctu=32"
  e b_dbk_sao_10bit 200x136 16 yuv420p10le "scenecut=0:rc-lookahead=24:keyint=60:bframes=4:ref=3:ctu=32"
  e b_dbk_sao_amp 200x136 16 yuv420p "scenecut=0:rc-lookahead=24:keyint=60:bframes=4:ref=3:ctu=32:rect=1:amp=1"
  # Weighted prediction needs brightness changes: a fade in, then a fade out, over the moving test picture.
  w() { # name frames fmt params
    local name=$1 frames=$2 fmt=$3 params=$4
    ff -f lavfi -i "testsrc2=size=200x136:rate=25:duration=$(python3 -c "print($frames/25)"),noise=alls=8:allf=t,fade=t=in:st=0:d=0.4,fade=t=out:st=0.5:d=0.4" -c:v libx265 -preset veryfast -pix_fmt "$fmt" -tag:v hvc1 -an \
       -x265-params "log-level=error:$params" "$d/$name.mp4"
  }
  w w_p 24 yuv420p "$nl:keyint=60:bframes=0:ref=3:ctu=32:weightp=1"
  w w_b 24 yuv420p "$nlb:keyint=60:bframes=4:ref=3:ctu=32:weightp=1:weightb=1"
  w w_b_10bit 24 yuv420p10le "$nlb:keyint=60:bframes=4:ref=3:ctu=32:weightp=1:weightb=1"
  w w_b_full 24 yuv420p "scenecut=0:rc-lookahead=24:keyint=60:bframes=4:ref=3:ctu=32:weightp=1:weightb=1"
  touch "$d/.done"
}

fixture_set="${RVP_FIXTURE_SET:-all}"
if [[ "$fixture_set" == basic ]]; then gen_basic; fi
if [[ "$fixture_set" == all || "$fixture_set" == core ]]; then gen_core; fi
if [[ "$fixture_set" == all || "$fixture_set" == h264 ]]; then gen_h264; fi
if [[ "$fixture_set" == all || "$fixture_set" == vp9 ]]; then gen_vp9; fi
if [[ "$fixture_set" == all || "$fixture_set" == m8 ]]; then gen_m8; fi
if [[ "$fixture_set" == all || "$fixture_set" == audio ]]; then gen_audio; fi
if [[ "$fixture_set" == all || "$fixture_set" == library ]]; then gen_library; fi
if [[ "$fixture_set" == all || "$fixture_set" == levels ]]; then gen_levels; fi
if [[ "$fixture_set" == all || "$fixture_set" == hevc ]]; then gen_hevc; fi
if [[ "$fixture_set" == hevcconf ]]; then gen_hevcconf; fi
if [[ "$fixture_set" == perf ]]; then gen_perf; fi
echo "fixtures in $out"
