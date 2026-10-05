//! Tags, cover art and chapters read from the container.
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

fn fixture(name: &str) -> PathBuf {
    static ONCE: Once = Once::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir =
        std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"));
    ONCE.call_once(|| {
        let st = Command::new("bash")
            .arg(root.join("tools/gen-fixtures.sh"))
            .arg(&dir)
            .env("RVP_FIXTURE_SET", "m8")
            .status()
            .expect("run gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    dir.join("m8").join(name)
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

async fn demuxer(name: &str) -> rvp_demux::AnyDemuxer<MemSource> {
    open(MemSource::new(std::fs::read(fixture(name)).unwrap())).await.unwrap()
}

#[test]
fn tags_and_cover_art() {
    if skip() {
        return;
    }
    block_on(async {
        let cover = std::fs::read(fixture("cover.jpg")).unwrap();
        for name in ["tagged.m4a", "tagged.webm", "tagged_art.mka"] {
            let d = demuxer(name).await;
            let m = d.metadata();
            assert_eq!(m.title.as_deref(), Some("Sine Song"), "{name}");
            assert_eq!(m.artist.as_deref(), Some("The Tones"), "{name}");
            assert_eq!(m.album.as_deref(), Some("Pure"), "{name}");
            if name != "tagged.webm" {
                let art = m.art.as_ref().unwrap_or_else(|| panic!("{name}: no cover art"));
                assert_eq!(art.mime, "image/jpeg", "{name}");
                assert_eq!(art.data, cover, "{name}: the picture is byte for byte the file");
            } else {
                assert!(m.art.is_none());
            }
        }
        // A file without tags has none.
        let d = demuxer("subs_srt.mkv").await;
        assert_eq!(d.metadata().title, None);
    });
}

#[test]
fn chapters_in_matroska_and_mp4() {
    if skip() {
        return;
    }
    block_on(async {
        for name in ["chapters.mkv", "chapters.mp4"] {
            let d = demuxer(name).await;
            let got: Vec<(i64, &str)> = d.chapters().iter().map(|c| (c.start_us, c.title.as_str())).collect();
            assert_eq!(got, [(0, "Intro"), (2_000_000, "Middle"), (4_000_000, "End")], "{name}");
        }
        assert!(demuxer("subs_srt.mkv").await.chapters().is_empty());
    });
}
