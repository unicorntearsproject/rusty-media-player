//! Shared helpers for the conformance tests: fixtures, demuxing and the ffmpeg oracle.
#![allow(dead_code)]
pub mod synth;
use rvp_codec_h264::decoder::{Decoder, Frame};
use rvp_core::StreamKind;
use rvp_core::task::block_on;
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

/// Path of an H.264 fixture (`target/fixtures/h264/<name>`), generating the set on first use.
pub fn fixture(name: &str) -> PathBuf {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let d = fixture_dir();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
        let st = Command::new("bash")
            .arg(script)
            .arg(&d)
            .env("RVP_FIXTURE_SET", "h264")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(
            st.success(),
            "fixture generation failed (is ffmpeg with libx264 installed? RVP_SKIP_FIXTURES=1 skips)"
        );
    });
    fixture_dir().join("h264").join(name)
}

/// One decoded picture with the packet timestamp it should carry.
pub struct Decoded {
    pub frames: Vec<Frame>,
    pub packets: usize,
    pub errors: usize,
}

/// Decode every video packet of an MP4/MKV file with our decoder.
pub fn decode_container(path: &Path) -> Decoded {
    let data = std::fs::read(path).unwrap();
    block_on(async {
        let mut d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let mut dec = Decoder::new();
        dec.set_avcc(&info.extra_data).unwrap();
        let mut frames = Vec::new();
        let (mut packets, mut errors) = (0, 0);
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id != info.id {
                continue;
            }
            packets += 1;
            if dec.decode_sample(&p.data, p.pts).is_err() {
                errors += 1;
            }
            while let Some(f) = dec.next_frame() {
                frames.push(f);
            }
        }
        if dec.flush().is_err() {
            errors += 1;
        }
        while let Some(f) = dec.next_frame() {
            frames.push(f);
        }
        Decoded { frames, packets, errors }
    })
}

/// The ffmpeg reference: every decoded frame as raw planar yuv420p.
pub fn reference(path: &Path) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

/// Compare our frames against raw ffmpeg output and describe the first difference.
pub fn compare(name: &str, frames: &[Frame], want: &[u8]) -> Result<(), String> {
    if frames.is_empty() {
        return Err(format!("{name}: no frames decoded"));
    }
    let (w, h) = (frames[0].width, frames[0].height);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let fsz = w * h + 2 * cw * ch;
    if want.len() != frames.len() * fsz {
        return Err(format!(
            "{name}: frame count differs: ours {}, ffmpeg {}",
            frames.len(),
            want.len() / fsz
        ));
    }
    for (i, f) in frames.iter().enumerate() {
        let r = &want[i * fsz..(i + 1) * fsz];
        let planes = [
            (&f.planes[0], &r[..w * h], w, h),
            (&f.planes[1], &r[w * h..w * h + cw * ch], cw, ch),
            (&f.planes[2], &r[w * h + cw * ch..], cw, ch),
        ];
        for (pi, (ours, theirs, pw, _ph)) in planes.iter().enumerate() {
            if ours.as_slice() != *theirs {
                let pos = ours.iter().zip(theirs.iter()).position(|(a, b)| a != b).unwrap();
                let bad = ours.iter().zip(theirs.iter()).filter(|(a, b)| a != b).count();
                let (x, y) = (pos % pw, pos / pw);
                let mb = if pi == 0 { (x / 16, y / 16) } else { (x / 8, y / 8) };
                return Err(format!(
                    "{name}: frame {i} (poc {}, pts {}) plane {pi} differs at ({x},{y}) = MB {mb:?}: ours {} ffmpeg {}; {bad} samples differ",
                    f.poc, f.pts, ours[pos], theirs[pos]
                ));
            }
        }
    }
    Ok(())
}

/// Decode `name` and require a bit-exact match with ffmpeg.
pub fn check(name: &str) {
    if skip() {
        return;
    }
    let path = fixture(name);
    let d = decode_container(&path);
    let want = reference(&path);
    if let Err(e) = compare(name, &d.frames, &want) {
        panic!("{e} ({} packets, {} errors)", d.packets, d.errors);
    }
    assert_eq!(d.errors, 0, "{name}: decoder reported errors");
    // Presentation timestamps must come out strictly increasing (output order).
    assert!(d.frames.windows(2).all(|w| w[0].pts < w[1].pts), "{name}: pts not increasing");
}
