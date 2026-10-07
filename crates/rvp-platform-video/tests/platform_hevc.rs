//! HEVC through the system's own decoder (VA-API on Linux, VideoToolbox on macOS, Media Foundation on Windows). A conformant decoder is
//! bit-exact, so every picture must match ffmpeg's. Where the system has no decoder for it (a Windows machine without the HEVC Video
//! Extensions, a Linux box without a GPU) the test checks that the refusal is a clear sentence instead.
use rvp_core::task::block_on;
use rvp_core::{PixelFormat, PlatformSupport, StreamKind};
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> Option<PathBuf> {
    if std::env::var_os("RVP_SKIP_FIXTURES").is_some()
        || Command::new("ffmpeg").arg("-version").output().is_err()
    {
        return None;
    }
    let dir =
        std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root().join("target/fixtures"));
    let st = Command::new("bash")
        .arg(root().join("tools/gen-fixtures.sh"))
        .arg(&dir)
        .env("RVP_FIXTURE_SET", "hevc")
        .status()
        .ok()?;
    assert!(st.success());
    let p = dir.join("hevc").join(name);
    p.exists().then_some(p)
}

/// ffmpeg's decode of the video as planar 8-bit or 10-bit 4:2:0.
fn reference(path: &Path, ten: bool) -> Vec<u8> {
    let fmt = if ten { "yuv420p10le" } else { "yuv420p" };
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", fmt, "-"])
        .output()
        .expect("ffmpeg");
    assert!(out.status.success());
    out.stdout
}

fn decode(path: &Path) -> Result<(Vec<rvp_core::VideoFrame>, bool), String> {
    let data = std::fs::read(path).unwrap();
    block_on(async {
        let mut d = open(MemSource::new(data)).await.map_err(|e| e.to_string())?;
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let plat = rvp_platform_video::system_video().ok_or("no platform decoder on this system")?;
        match plat.supports(&info) {
            PlatformSupport::Yes => {}
            PlatformSupport::No(why) => return Err(why),
        }
        let mut dec = plat.open(&info).map_err(|e| e.to_string())?;
        let mut frames = Vec::new();
        while let Some(p) = d.next_packet().await.map_err(|e| e.to_string())? {
            if p.stream_id != info.id {
                continue;
            }
            dec.send_packet(&p).map_err(|e| e.to_string())?;
            while let Some(f) = dec.receive_frame().map_err(|e| e.to_string())? {
                frames.push(f);
            }
        }
        dec.drain().map_err(|e| e.to_string())?;
        while let Some(f) = dec.receive_frame().map_err(|e| e.to_string())? {
            frames.push(f);
        }
        Ok((frames, true))
    })
}

#[test]
fn extended_hevc_streams_match_ffmpeg_bit_for_bit_where_there_is_a_decoder() {
    for (name, ten) in [
        ("hevc_aac.mp4", false),
        ("hevc10_aac.mp4", true),
        ("hevc_flac.mkv", false),
        ("hevc_slices.mp4", false),
        ("hevc_weighted.mp4", false),
        ("hevc_opengop.mp4", false),
        ("hevc_ctu32.mp4", false),
        ("hevc_odd.mp4", false),
        ("hevc10_slices.mp4", true),
    ] {
        let Some(path) = fixture(name) else { return };
        match decode(&path) {
            Err(why) => {
                // No VA-API, no GPU or no HEVC: a sentence a person can read, not a code.
                assert!(why.len() > 10 && !why.starts_with("VA-API status"), "{name}: {why}");
                eprintln!("{name}: no VA-API HEVC here: {why}");
            }
            Ok((frames, _)) => {
                let want = reference(&path, ten);
                let (w, h) = (frames[0].width as usize, frames[0].height as usize);
                let frame_bytes = w * h * 3 / 2 * if ten { 2 } else { 1 };
                assert_eq!(frames.len(), want.len() / frame_bytes, "{name}: every picture, once");
                let mut got = Vec::new();
                let mut last_pts = -1;
                for f in &frames {
                    assert!(f.pts > last_pts, "{name}: frames in presentation order");
                    last_pts = f.pts;
                    for p in &f.planes {
                        got.extend_from_slice(p);
                    }
                }
                assert_eq!(got.len(), want.len(), "{name}: size");
                let bad = got.iter().zip(&want).filter(|(a, b)| a != b).count();
                assert_eq!(bad, 0, "{name}: {bad} of {} samples differ from ffmpeg", want.len());
            }
        }
    }
}

#[test]
fn hdr_streams_come_out_tone_mapped_to_sdr_rgba() {
    for name in ["hevc10_hdr.mp4", "hevc10_hlg.mp4"] {
        let Some(path) = fixture(name) else { return };
        let Ok((frames, _)) = decode(&path) else { return };
        assert_eq!(frames.len(), 125, "{name}");
        let f = &frames[60];
        assert_eq!(f.format, PixelFormat::Rgba8, "{name}: mapped to SDR");
        let px = &f.planes[0];
        let mean = px.as_chunks::<4>().0.iter().map(|p| p[0] as u32 + p[1] as u32 + p[2] as u32).sum::<u32>()
            as f64
            / (px.len() / 4) as f64
            / 3.0;
        assert!((15.0..245.0).contains(&mean), "{name}: mean level {mean}");
        assert!(px.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }
}

/// The small streams committed beside this test, with the FNV-1a 64 hash and size of ffmpeg's decode of each (no ffmpeg needed to run).
fn committed(name: &str) -> (PathBuf, u64, usize) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let h = std::fs::read_to_string(dir.join(format!("{name}.yuv.fnv"))).unwrap();
    let mut it = h.split_whitespace();
    let hash = u64::from_str_radix(it.next().unwrap(), 16).unwrap();
    let len = it.next().unwrap().parse().unwrap();
    (dir.join(format!("{name}.mp4")), hash, len)
}

fn fnv(data: &[&[u8]]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for d in data {
        for b in *d {
            h = (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// What a refusal has to be: a sentence for a person, naming what is missing.
fn assert_clear(name: &str, why: &str) {
    assert!(why.len() > 20 && why.chars().any(|c| c.is_alphabetic()), "{name}: {why:?}");
    eprintln!("{name}: this system has no HEVC decoder for it: {why}");
}

#[test]
fn the_committed_hevc_streams_decode_exactly_on_this_system_or_the_refusal_is_clear() {
    for name in ["hevc_main", "hevc_main10"] {
        let (path, want_hash, want_len) = committed(name);
        match decode(&path) {
            Err(why) => assert_clear(name, &why),
            Ok((frames, _)) => {
                assert_eq!(frames.len(), 40, "{name}");
                let mut last = -1;
                let mut planes: Vec<&[u8]> = Vec::new();
                for f in &frames {
                    assert!(f.pts > last, "{name}: presentation order");
                    last = f.pts;
                    assert_ne!(f.format, PixelFormat::Rgba8, "{name}: SDR stays YUV");
                    for p in &f.planes {
                        planes.push(p);
                    }
                }
                let total: usize = planes.iter().map(|p| p.len()).sum();
                assert_eq!(total, want_len, "{name}: size");
                assert_eq!(fnv(&planes), want_hash, "{name}: differs from ffmpeg's decode");
            }
        }
    }
}

#[test]
fn the_committed_hdr10_stream_is_tone_mapped_or_refused_clearly() {
    let (path, _, _) = committed("hevc_hdr10");
    match decode(&path) {
        Err(why) => assert_clear("hevc_hdr10", &why),
        Ok((frames, _)) => {
            assert_eq!(frames.len(), 40);
            let f = &frames[20];
            assert_eq!(f.format, PixelFormat::Rgba8, "mapped to SDR");
            let px = &f.planes[0];
            let mean =
                px.as_chunks::<4>().0.iter().map(|p| p[0] as u32 + p[1] as u32 + p[2] as u32).sum::<u32>()
                    as f64
                    / (px.len() / 4) as f64
                    / 3.0;
            assert!((15.0..245.0).contains(&mean), "mean level {mean}");
        }
    }
}
