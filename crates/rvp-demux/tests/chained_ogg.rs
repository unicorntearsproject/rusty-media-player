//! Chained Ogg (two complete logical streams one after the other): the demuxer plays on through both links, with the
//! timeline continuing across the join, the headers of the second link in the packet flow, a duration that is the sum, and
//! seeks that land in the right link. Fixtures: `tools/gen-fixtures.sh`, set `audio` (`chained.ogg`, `chained.opus`, with
//! the links on their own as `chain_a.*` and `chain_b.*`).
use rvp_core::Packet;
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
            .env("RVP_FIXTURE_SET", "audio")
            .status()
            .expect("run gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    d.join("audio")
}

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap()
}

fn drain(d: &mut impl Demuxer) -> Vec<Packet> {
    let mut v = Vec::new();
    while let Some(p) = block_on(d.next_packet()).unwrap() {
        v.push(p);
    }
    v
}

/// The header packets of a link: a Vorbis header (types 1, 3, 5), an `OpusHead`.
fn is_header(p: &Packet) -> bool {
    p.data.starts_with(b"OpusHead")
        || p.data.starts_with(b"\x7fFLAC")
        || (p.data.len() > 6 && p.data[0] & 1 == 1 && &p.data[1..7] == b"vorbis")
}

/// Packets that carry audio.
fn audio(p: &[Packet]) -> Vec<&Packet> {
    p.iter().filter(|p| !is_header(p)).collect()
}

#[test]
fn both_links_are_played_with_one_timeline() {
    if std::env::var_os("RVP_SKIP_FIXTURES").is_some() {
        return;
    }
    for ext in ["ogg", "opus", "oga"] {
        let mut a = block_on(open(MemSource::new(bytes(&format!("chain_a.{ext}"))))).unwrap();
        let mut b = block_on(open(MemSource::new(bytes(&format!("chain_b.{ext}"))))).unwrap();
        let (da, db) = (a.duration_us().unwrap(), b.duration_us().unwrap());
        let (pa, pb) = (drain(&mut a), drain(&mut b));
        let mut c = block_on(open(MemSource::new(bytes(&format!("chained.{ext}"))))).unwrap();
        let dc = c.duration_us().unwrap();
        assert!((dc - (da + db)).abs() < 1_000, "chained.{ext}: duration {dc} us, the links are {da} + {db}");
        let all = drain(&mut c);
        let au = audio(&all);
        assert_eq!(au.len(), audio(&pa).len() + audio(&pb).len(), "chained.{ext}: audio packet count");
        // The second link's audio follows the first on the same clock: the same packets, shifted by the first link's length.
        let first = audio(&pa).len();
        for (x, y) in audio(&pb).into_iter().zip(&au[first..]) {
            assert_eq!(x.data, y.data, "chained.{ext}: the second link's packets are the file's");
            assert!(
                (y.pts - (x.pts + da)).abs() <= 1_500,
                "chained.{ext}: second link at {} us, expected {} us",
                y.pts,
                x.pts + da
            );
        }
        // Times only go forward over the whole file.
        assert!(au.windows(2).all(|w| w[1].pts >= w[0].pts), "chained.{ext}: times go back");
        // The headers of the second link are in the packet flow, before its audio.
        let join = all.iter().position(|p| is_header(p) && p.pts > 0).expect("header packets of link 2");
        assert!(all[join].pts >= da - 1_000, "chained.{ext}: headers at {}", all[join].pts);
        let head = &all[join].data;
        match ext {
            "ogg" => assert!(head.starts_with(b"\x01vorbis")),
            "oga" => assert!(head.starts_with(b"\x7fFLAC")),
            _ => assert!(head.starts_with(b"OpusHead")),
        }
    }
}

#[test]
fn seeks_land_in_the_right_link_and_bring_its_headers() {
    if std::env::var_os("RVP_SKIP_FIXTURES").is_some() {
        return;
    }
    for ext in ["ogg", "opus", "oga"] {
        let name = format!("chained.{ext}");
        let mut whole = block_on(open(MemSource::new(bytes(&name)))).unwrap();
        let all = drain(&mut whole);
        let dur = whole.duration_us().unwrap();
        // A time in each link (the first is 1.5 s long), the very start, and just before the end.
        for target in [0, 700_000, 1_400_000, 1_600_000, 2_300_000, dur - 100_000] {
            let mut d = block_on(open(MemSource::new(bytes(&name)))).unwrap();
            let landed = block_on(d.seek(target)).unwrap();
            assert!(
                landed <= target + 1_000 && target - landed <= 1_300_000,
                "{name} @{target}: landed {landed}"
            );
            let rest = drain(&mut d);
            let first_audio = audio(&rest)[0];
            assert!(
                all.iter().any(|p| p.data == first_audio.data && (p.pts - first_audio.pts).abs() <= 30_000),
                "{name} @{target}: the first packet after the seek is not in the file"
            );
            assert_eq!(
                rest.last().map(|p| &p.data),
                all.last().map(|p| &p.data),
                "{name} @{target}: ends with the last packet"
            );
            // A seek into a chain brings the headers of the link it landed in.
            if target > 1_600_000 {
                assert!(is_header(&rest[0]) && rest[0].pts > 1_000_000, "{name} @{target}: no headers first");
            } else if target < 1_400_000 {
                assert!(
                    is_header(&rest[0]) && rest[0].pts == 0,
                    "{name} @{target}: no headers of link 1 first"
                );
            }
        }
    }
}

#[test]
fn a_chain_of_one_file_twice_with_the_same_serial_number_still_plays_on() {
    if std::env::var_os("RVP_SKIP_FIXTURES").is_some() {
        return;
    }
    // The same file twice: both links have the same serial number, and a tail that looks like the start of the file.
    for ext in ["ogg", "opus", "oga"] {
        let one = bytes(&format!("chain_a.{ext}"));
        let mut two = one.clone();
        two.extend_from_slice(&one);
        let mut once = block_on(open(MemSource::new(one))).unwrap();
        let single = audio(&drain(&mut once)).len();
        let mut d = block_on(open(MemSource::new(two))).unwrap();
        let n = audio(&drain(&mut d)).len();
        assert_eq!(n, 2 * single, "{ext}: both copies are played");
    }
}
