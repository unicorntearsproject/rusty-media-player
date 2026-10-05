//! AV1 decode against ffmpeg: every decoded frame must be bit-exact (8-bit and 10-bit).
use rvp_core::task::block_on;
use rvp_core::{PixelFormat, StreamKind, VideoFrame};
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

fn dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"))
}

fn fixture(name: &str) -> PathBuf {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let d = dir();
        if !d.join(".done").exists() {
            let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
            let st = Command::new("bash").arg(script).arg(&d).status().expect("run tools/gen-fixtures.sh");
            assert!(
                st.success(),
                "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)"
            );
        }
    });
    dir().join(name)
}

fn decode_all(path: &Path) -> Vec<VideoFrame> {
    let data = std::fs::read(path).unwrap();
    block_on(async {
        let mut d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let mut dec = rvp_codec_av1::av1_decoder(&info).unwrap();
        let mut frames = Vec::new();
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id == info.id {
                dec.send_packet(&p).unwrap();
                while let Some(f) = dec.receive_frame().unwrap() {
                    frames.push(f);
                }
            }
        }
        dec.drain().unwrap();
        while let Some(f) = dec.receive_frame().unwrap() {
            frames.push(f);
        }
        frames
    })
}

fn reference(path: &Path, pix_fmt: &str) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", pix_fmt, "-"])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg failed");
    out.stdout
}

fn check(name: &str, pix_fmt: &str, format: PixelFormat) {
    if std::env::var_os("RVP_SKIP_FIXTURES").is_some() {
        return;
    }
    let path = fixture(name);
    let frames = decode_all(&path);
    let want = reference(&path, pix_fmt);
    assert!(!frames.is_empty(), "{name}: no frames");
    let mut got = Vec::new();
    for f in &frames {
        assert_eq!(f.format, format);
        for p in &f.planes {
            got.extend_from_slice(p);
        }
    }
    assert_eq!(got.len(), want.len(), "{name}: total bytes ({} frames)", frames.len());
    if let Some(i) = (0..got.len()).find(|&i| got[i] != want[i]) {
        let fsz = want.len() / frames.len();
        panic!("{name}: first mismatch at byte {i} (frame {}, offset {})", i / fsz, i % fsz);
    }
    // Presentation timestamps strictly increase (the decoder outputs in display order).
    assert!(frames.windows(2).all(|w| w[0].pts < w[1].pts), "{name}: pts not increasing");
}

#[test]
fn eight_bit_av1_is_bit_exact_with_ffmpeg() {
    check("av1_opus.webm", "yuv420p", PixelFormat::Yuv420p8);
}

#[test]
fn ten_bit_av1_is_bit_exact_with_ffmpeg() {
    check("av1_10bit.webm", "yuv420p10le", PixelFormat::Yuv420p10);
}

#[test]
fn flush_resets_the_decoder_so_decoding_can_restart_at_a_keyframe() {
    if std::env::var_os("RVP_SKIP_FIXTURES").is_some() {
        return;
    }
    let path = fixture("av1_opus.webm");
    let data = std::fs::read(&path).unwrap();
    block_on(async {
        let mut d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let mut dec = rvp_codec_av1::av1_decoder(&info).unwrap();
        // Decode a little, seek to a keyframe in the middle, flush, and decode from there.
        for _ in 0..30 {
            let p = d.next_packet().await.unwrap().unwrap();
            if p.stream_id == info.id {
                dec.send_packet(&p).unwrap();
            }
        }
        let landed = d.seek(3_000_000).await.unwrap();
        dec.flush();
        let mut first = None;
        while first.is_none() {
            let p = d.next_packet().await.unwrap().unwrap();
            if p.stream_id == info.id {
                dec.send_packet(&p).unwrap();
                first = dec.receive_frame().unwrap();
            }
        }
        assert_eq!(first.unwrap().pts, landed, "first frame after the seek is the keyframe we landed on");
    });
}
