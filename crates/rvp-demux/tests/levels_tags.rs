//! ReplayGain, Opus R128 and gapless-album tags read from real files in every container (`tools/gen-fixtures.sh`, set `levels`):
//! ID3v2.3 and v2.4 `TXXX`, Vorbis comments in FLAC, Ogg Vorbis and Matroska, Opus `R128_*_GAIN`, MP4 in both of its
//! custom-tag styles (QuickTime `mdta` keys and iTunes `----` items with `pgap`).
use rvp_core::Metadata;
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

fn dir() -> PathBuf {
    static ONCE: Once = Once::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let d =
        std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"));
    ONCE.call_once(|| {
        let st = Command::new("bash")
            .arg(root.join("tools/gen-fixtures.sh"))
            .arg(&d)
            .env("RVP_FIXTURE_SET", "levels")
            .status()
            .expect("run gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    d.join("levels")
}

fn meta(name: &str) -> Metadata {
    let bytes = std::fs::read(dir().join(name)).unwrap();
    let d = block_on(open(MemSource::new(bytes))).unwrap_or_else(|e| panic!("{name}: {e}"));
    d.metadata().clone()
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

/// ReplayGain 2: track -6.50 dB and album -3.20 dB mean -11.5 and -14.8 LUFS (the reference is -18).
fn check_replaygain(name: &str, gapless: bool) {
    let m = meta(name);
    let l = m.loudness;
    let near =
        |a: Option<f32>, b: f32| assert!(a.is_some_and(|a| (a - b).abs() < 1e-4), "{name}: {a:?} vs {b}");
    near(l.track_lufs, -11.5);
    near(l.album_lufs, -14.8);
    near(l.track_peak, 0.977);
    near(l.album_peak, 1.0);
    assert_eq!(m.gapless_album, gapless, "{name}: gapless album flag");
    assert_eq!(m.title.as_deref(), Some("Tagged"), "{name}: the ordinary tags still come through");
}

#[test]
fn replaygain_in_every_container() {
    if skip() {
        return;
    }
    check_replaygain("tagged_v23.mp3", true); // ID3v2.3, UTF-16 descriptions, `iTunPGAP`
    check_replaygain("tagged_v24.mp3", false); // ID3v2.4
    check_replaygain("tagged.flac", false);
    check_replaygain("tagged.ogg", false);
    check_replaygain("tagged.mka", false);
    check_replaygain("tagged_mdta.m4a", true); // QuickTime keys, `gapless_playback`
    check_replaygain("tagged_itunes.m4a", true); // `----` items and `pgap`
}

#[test]
fn opus_r128_gains() {
    if skip() {
        return;
    }
    // R128_TRACK_GAIN=-512 is -2 dB to add to reach -23 LUFS: the track is at -21. R128_ALBUM_GAIN=256 (+1 dB): -24.
    let m = meta("tagged.opus");
    assert_eq!(m.loudness.track_lufs, Some(-21.0));
    assert_eq!(m.loudness.album_lufs, Some(-24.0));
    assert_eq!((m.loudness.track_peak, m.loudness.album_peak), (None, None));
    assert!(!m.gapless_album);
}

#[test]
fn files_without_tags_have_no_loudness() {
    if skip() {
        return;
    }
    for name in ["untagged.wav", "music.flac", "music.mp3", "music.m4a", "music.opus", "music.ogg"] {
        let m = meta(name);
        assert!(m.loudness.is_empty() && !m.gapless_album, "{name}");
    }
}
