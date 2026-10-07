//! The software decoder against ffmpeg: every stream of the conformance matrix (`tools/gen-fixtures.sh`, set `hevcconf`) is decoded
//! with ours and with ffmpeg, and every sample of every picture must be the same. A mismatch is reported with where it is.
//! `RVP_HEVC_ONLY=name` runs the streams whose name contains it.
use rvp_codec_hevc::sw::hevc_decoder;
use rvp_core::task::block_on;
use rvp_core::{PixelFormat, StreamKind};
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> Option<PathBuf> {
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
        .env("RVP_FIXTURE_SET", "hevcconf")
        .status()
        .ok()?;
    assert!(st.success());
    Some(dir.join("hevcconf"))
}

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

/// Decode a file with the software decoder: the frames as planar bytes, and the first error if any.
fn decode(path: &Path) -> (Vec<rvp_core::VideoFrame>, Option<String>, bool) {
    let data = std::fs::read(path).unwrap();
    block_on(async {
        let mut d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let mut dec = hevc_decoder(&info).unwrap();
        let mut frames = Vec::new();
        let mut err = None;
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id != info.id {
                continue;
            }
            if let Err(e) = dec.send_packet(&p) {
                err.get_or_insert(format!("{e}"));
            }
            while let Ok(Some(f)) = dec.receive_frame() {
                frames.push(f);
            }
        }
        let _ = dec.drain();
        while let Ok(Some(f)) = dec.receive_frame() {
            frames.push(f);
        }
        let ten = frames.first().is_some_and(|f| f.format == PixelFormat::Yuv420p10);
        (frames, err, ten)
    })
}

fn compare(name: &str, path: &Path) -> Result<(), String> {
    let (frames, err, ten) = decode(path);
    if let Some(e) = err {
        return Err(format!("{name}: decode error after {} frames: {e}", frames.len()));
    }
    let want = reference(path, ten);
    let (w, h) =
        (frames.first().ok_or(format!("{name}: no frames"))?.width as usize, frames[0].height as usize);
    let bps = if ten { 2 } else { 1 };
    let frame_bytes = w * h * 3 / 2 * bps;
    if frames.len() != want.len() / frame_bytes {
        return Err(format!("{name}: {} frames, ffmpeg has {}", frames.len(), want.len() / frame_bytes));
    }
    for (n, f) in frames.iter().enumerate() {
        let mut off = n * frame_bytes;
        for (c, plane) in f.planes.iter().enumerate() {
            let (pw, ph) = if c == 0 { (w, h) } else { (w / 2, h / 2) };
            let row = pw * bps;
            for y in 0..ph {
                let got = &plane[y * f.strides[c]..y * f.strides[c] + row];
                let exp = &want[off + y * row..off + (y + 1) * row];
                if got != exp {
                    let x = (0..row).find(|i| got[*i] != exp[*i]).unwrap() / bps;
                    let bad: usize = (0..ph)
                        .map(|yy| {
                            (0..row)
                                .filter(|i| plane[yy * f.strides[c] + i] != want[off + yy * row + i])
                                .count()
                        })
                        .sum();
                    return Err(format!(
                        "{name}: frame {n} plane {c} first differs at ({x},{y}): got {} want {} ({bad} bytes differ in the plane)",
                        got[x * bps],
                        exp[x * bps]
                    ));
                }
            }
            off += row * ph;
        }
    }
    Ok(())
}

#[test]
fn the_software_decoder_matches_ffmpeg_on_the_conformance_matrix() {
    let Some(dir) = fixtures() else { return };
    let only = std::env::var("RVP_HEVC_ONLY").ok();
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mp4"))
        .collect();
    names.sort();
    let mut failures = Vec::new();
    let mut ran = 0;
    for p in names {
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
            continue;
        }
        ran += 1;
        match compare(&name, &p) {
            Ok(()) => eprintln!("ok   {name}"),
            Err(e) => {
                eprintln!("FAIL {e}");
                failures.push(e);
            }
        }
    }
    assert!(ran > 0);
    assert!(failures.is_empty(), "{} of {ran} streams differ:\n{}", failures.len(), failures.join("\n"));
}
