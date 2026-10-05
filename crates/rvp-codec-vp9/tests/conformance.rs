//! VP9 decode against ffmpeg (libavcodec's native VP9 decoder): every decoded plane of every frame must be
//! bit-exact, across sizes, odd sizes, encoder settings, hidden alt-ref frames, tiles, segmentation, lossless,
//! 10-bit (profile 2) and a resolution change at a key frame.
mod common;
use common::*;
use rvp_core::PixelFormat;

/// Decode `name` and compare with ffmpeg frame by frame. Returns the frame count.
fn check(name: &str, pix_fmt: &str, format: PixelFormat) -> usize {
    if skip() {
        return 0;
    }
    let path = fixture(name);
    let (info, packets) = read_packets(&path);
    let (frames, errors) = decode_packets(&info, &packets);
    assert_eq!(errors, 0, "{name}: packets that failed");
    let want = reference(&path, pix_fmt);
    assert!(!frames.is_empty(), "{name}: no frames");
    let mut off = 0;
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(f.format, format, "{name}");
        let bytes: usize = f.planes.iter().map(Vec::len).sum();
        assert!(
            off + bytes <= want.len(),
            "{name}: frame {i} runs past ffmpeg's output ({} frames decoded)",
            frames.len()
        );
        let mut got = Vec::with_capacity(bytes);
        for p in &f.planes {
            got.extend_from_slice(p);
        }
        assert_eq!(
            fnv(&got),
            fnv(&want[off..off + bytes]),
            "{name}: frame {i} ({}x{}) differs",
            f.width,
            f.height
        );
        off += bytes;
    }
    assert_eq!(off, want.len(), "{name}: total bytes ({} frames)", frames.len());
    frames.len()
}

fn p0(name: &str) -> usize {
    check(name, "yuv420p", PixelFormat::Yuv420p8)
}

#[test]
fn sizes() {
    for n in ["s_64x64", "s_176x144", "s_352x288", "s_8x8", "s_720p"] {
        p0(&format!("{n}.webm"));
    }
}

#[test]
fn odd_sizes() {
    for n in ["s_odd_327x245", "s_odd_130x66", "s_odd_17x9"] {
        p0(&format!("{n}.webm"));
    }
}

#[test]
fn typical_720p_and_1080p_with_tiles() {
    assert_eq!(p0("s_720p_typ.webm"), if skip() { 0 } else { 60 });
    assert_eq!(p0("s_1080p_typ.webm"), if skip() { 0 } else { 60 });
}

#[test]
fn altref_superframes_and_hidden_frames() {
    // Hidden alt-ref frames ride in superframes with the next shown frame; one packet in, one picture out.
    assert_eq!(p0("t_altref.webm"), if skip() { 0 } else { 40 });
}

#[test]
fn encoder_settings() {
    for n in [
        "t_rt",
        "t_best",
        "t_errres",
        "t_frameparallel",
        "t_aq_cyclic",
        "t_aq_variance",
        "t_aq_complexity",
        "t_lossless",
        "t_q4",
        "t_q63",
        "t_sharp7",
        "t_static",
        "t_screen",
        "t_gop1",
        "t_cbr",
        "t_tiles4",
    ] {
        p0(&format!("{n}.webm"));
    }
}

#[test]
fn profile_2_ten_bit() {
    check("x_profile2_10bit.webm", "yuv420p10le", PixelFormat::Yuv420p10);
}

#[test]
fn resolution_change_at_a_key_frame() {
    if skip() {
        return;
    }
    let n = p0("r_keyframe.webm");
    assert_eq!(n, 45);
    let (info, packets) = read_packets(&fixture("r_keyframe.webm"));
    let (frames, _) = decode_packets(&info, &packets);
    let sizes: Vec<(u32, u32)> = frames.iter().map(|f| (f.width, f.height)).collect();
    assert_eq!(sizes[0], (320, 240));
    assert_eq!(sizes[15], (480, 270));
    assert_eq!(sizes[44], (200, 120));
}

#[test]
fn video_with_audio_track_decodes() {
    assert_eq!(p0("av_opus.webm"), if skip() { 0 } else { 150 });
}

#[test]
fn timestamps_follow_the_packets() {
    if skip() {
        return;
    }
    let (info, packets) = read_packets(&fixture("s_352x288.webm"));
    let (frames, _) = decode_packets(&info, &packets);
    assert_eq!(frames.len(), packets.len());
    for (f, p) in frames.iter().zip(&packets) {
        assert_eq!(f.pts, p.pts);
    }
}

#[test]
fn profile_1_chroma_layouts_are_rejected_cleanly() {
    if skip() {
        return;
    }
    let (info, packets) = read_packets(&fixture("x_profile1_444.webm"));
    let (frames, errors) = decode_packets(&info, &packets);
    assert!(frames.is_empty());
    assert_eq!(errors, packets.len());
}

#[test]
fn colour_comes_from_the_key_frame() {
    if skip() {
        return;
    }
    let (info, packets) = read_packets(&fixture("s_176x144.webm"));
    let (frames, _) = decode_packets(&info, &packets);
    // ffmpeg writes "unknown" for this source, so SD gets BT.601 and limited range.
    assert_eq!(frames[0].matrix, rvp_core::ColorMatrix::Bt601);
    assert_eq!(frames[0].range, rvp_core::ColorRange::Limited);
}

#[test]
fn colour_tags_are_read_from_the_key_frame() {
    if skip() {
        return;
    }
    for (name, matrix, range) in [
        ("c_bt709_full.webm", rvp_core::ColorMatrix::Bt709, rvp_core::ColorRange::Full),
        ("c_bt2020.webm", rvp_core::ColorMatrix::Bt2020, rvp_core::ColorRange::Limited),
    ] {
        p0(name);
        let (info, packets) = read_packets(&fixture(name));
        let (frames, _) = decode_packets(&info, &packets);
        assert_eq!((frames[0].matrix, frames[0].range), (matrix, range), "{name}");
    }
}
