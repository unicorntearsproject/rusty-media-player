//! M9 hardening: truncated and corrupt files play to the end of what is usable and stop with `Ended` or `Failed`; they
//! never panic, hang or run away.
use rvp_host_headless::{PlayOptions, SessionState, play_file};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"))
}

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/hardening");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// Fixtures whose structure lets playback start even when the end is cut off (header and index first, or self-contained
/// fragments / clusters).
const STREAMABLE: &[&str] =
    &["h264_aac_faststart.mp4", "h264_aac_frag.mp4", "h264_flac.mkv", "vp9_vorbis.webm", "av1_opus.webm"];
/// The same plus files whose index sits at the end: cut those and the open fails, which must be clean.
const ALL: &[&str] = &[
    "h264_aac.mp4",
    "h264_aac_faststart.mp4",
    "h264_aac_frag.mp4",
    "h264_flac.mkv",
    "vp9_vorbis.webm",
    "av1_opus.webm",
];

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some() || !fixtures().join("h264_aac.mp4").exists()
}

#[test]
fn truncated_files_play_what_is_there_and_stop() {
    if skip() {
        return;
    }
    for name in ALL {
        let full = std::fs::read(fixtures().join(name)).unwrap();
        let whole = play_file(fixtures().join(name).to_str().unwrap(), &PlayOptions::default()).unwrap();
        assert_eq!(whole.state, SessionState::Ended, "{name}");
        let total_video = whole.video_frames.len();
        for pct in [0u64, 1, 5, 10, 25, 50, 75, 90, 99] {
            let cut = (full.len() as u64 * pct / 100) as usize;
            let path = scratch(&format!("t{pct}_{name}"));
            std::fs::write(&path, &full[..cut]).unwrap();
            let r = play_file(path.to_str().unwrap(), &PlayOptions::default());
            let Ok(r) = r else { continue }; // opening may fail with an error: that is a clean stop
            assert!(
                matches!(r.state, SessionState::Ended | SessionState::Failed),
                "{name} cut at {pct}%: state {:?}",
                r.state
            );
            // Never more than the whole file gives, and for the streamable files a cut late in the file keeps most of it.
            assert!(r.video_frames.len() <= total_video, "{name} cut at {pct}%");
            if STREAMABLE.contains(name) && pct >= 75 && total_video > 0 {
                assert!(
                    r.video_frames.len() * 2 >= total_video * pct as usize / 100,
                    "{name} cut at {pct}%: {} of {total_video} frames",
                    r.video_frames.len()
                );
                assert_eq!(r.state, SessionState::Ended, "{name} cut at {pct}%: {:?}", r.error);
            }
            std::fs::remove_file(&path).ok();
        }
    }
}

/// A tiny deterministic generator (the tests need no `rand`).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[test]
fn corrupt_files_never_panic_or_hang() {
    if skip() {
        return;
    }
    let mut rng = Lcg(0x5eed);
    for name in ALL {
        let full = std::fs::read(fixtures().join(name)).unwrap();
        for variant in 0..6 {
            let mut data = full.clone();
            match variant {
                // A few flipped bytes anywhere.
                0..=2 => {
                    for _ in 0..(3 + variant * 20) {
                        let at = rng.next() as usize % data.len();
                        data[at] ^= 1 << (rng.next() % 8);
                    }
                }
                // Garbage over a run of bytes.
                3 => {
                    let at = rng.next() as usize % data.len();
                    for b in data[at..(at + 400).min(full.len())].iter_mut() {
                        *b = rng.next() as u8;
                    }
                }
                // The head damaged.
                4 => {
                    for b in data[..64.min(full.len())].iter_mut().skip(8) {
                        *b ^= 0x5a;
                    }
                }
                // A hole of zeros.
                _ => {
                    let at = rng.next() as usize % data.len();
                    data[at..(at + 2000).min(full.len())].fill(0);
                }
            }
            let path = scratch(&format!("c{variant}_{name}"));
            std::fs::write(&path, &data).unwrap();
            let opts = PlayOptions { max_virtual_us: Some(30_000_000), ..Default::default() };
            if let Ok(r) = play_file(path.to_str().unwrap(), &opts) {
                assert!(
                    matches!(r.state, SessionState::Ended | SessionState::Failed | SessionState::Playing),
                    "{name} variant {variant}: {:?}",
                    r.state
                );
            }
            std::fs::remove_file(&path).ok();
        }
    }
}
