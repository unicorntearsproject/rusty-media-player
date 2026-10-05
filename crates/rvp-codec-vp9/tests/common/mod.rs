//! Shared helpers: fixtures, demuxing and the ffmpeg oracle.
#![allow(dead_code)]
use rvp_core::task::block_on;
use rvp_core::{Packet, StreamInfo, StreamKind, VideoFrame};
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

pub fn fixture_dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"))
}

pub fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

/// Path of a VP9 fixture (`target/fixtures/vp9/<name>`), generating the set on first use.
pub fn fixture(name: &str) -> PathBuf {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let d = fixture_dir();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
        let st = Command::new("bash")
            .arg(script)
            .arg(&d)
            .env("RVP_FIXTURE_SET", "vp9")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(
            st.success(),
            "fixture generation failed (is ffmpeg with libvpx-vp9 installed? RVP_SKIP_FIXTURES=1 skips)"
        );
    });
    fixture_dir().join("vp9").join(name)
}

/// The stream info and all video packets of a WebM/MKV file.
pub fn read_packets(path: &Path) -> (StreamInfo, Vec<Packet>) {
    let data = std::fs::read(path).unwrap();
    block_on(async {
        let mut d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let mut pk = Vec::new();
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id == info.id {
                pk.push(p);
            }
        }
        (info, pk)
    })
}

/// Decode every packet; returns the frames and the number of packets that failed.
pub fn decode_packets(info: &StreamInfo, packets: &[Packet]) -> (Vec<VideoFrame>, usize) {
    let mut dec = rvp_codec_vp9::vp9_decoder(info).unwrap();
    let mut frames = Vec::new();
    let mut errors = 0;
    for p in packets {
        if dec.send_packet(p).is_err() {
            errors += 1;
        }
        while let Some(f) = dec.receive_frame().unwrap() {
            frames.push(f);
        }
    }
    dec.drain().unwrap();
    while let Some(f) = dec.receive_frame().unwrap() {
        frames.push(f);
    }
    (frames, errors)
}

/// ffmpeg's decode of the first video stream as raw planar frames at their own sizes.
pub fn reference(path: &Path, pix_fmt: &str) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-noautoscale", "-f", "rawvideo", "-pix_fmt", pix_fmt, "-"])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

/// FNV-1a over a plane set, for the framemd5-style per-frame comparison messages.
pub fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}
