//! Demuxer conformance against ffprobe on generated fixtures (see `tools/gen-fixtures.sh`).
//!
//! The fixtures are made on first use with ffmpeg and live in `target/fixtures` (not in git). Set
//! `RVP_SKIP_FIXTURES=1` to skip these tests on a machine without ffmpeg.
use rvp_core::task::block_on;
use rvp_core::{Packet, Rational, StreamKind};
use rvp_demux::{AnyDemuxer, Demuxer, open};
use rvp_host::mock::MemSource;
use std::path::{Path, PathBuf};
use std::sync::Once;

const FIXTURES: &[(&str, bool)] = &[
    // (file, ffprobe reports real dts)
    ("h264_aac.mp4", true),
    ("h264_aac_faststart.mp4", true),
    ("h264_aac_frag.mp4", true),
    ("av1_opus.webm", false),
    ("vp9_vorbis.webm", false),
    ("h264_flac.mkv", false),
];

fn dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"))
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

fn ensure() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let d = dir();
        if d.join(".done").exists() {
            return;
        }
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
        let st = std::process::Command::new("bash")
            .arg(script)
            .arg(&d)
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
}

fn bytes(name: &str) -> Vec<u8> {
    ensure();
    std::fs::read(dir().join(name)).unwrap()
}

fn probe(name: &str) -> serde_json::Value {
    ensure();
    serde_json::from_slice(&std::fs::read(dir().join(format!("{name}.probe.json"))).unwrap()).unwrap()
}

fn num(v: &serde_json::Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

struct Demuxed {
    streams: Vec<rvp_core::StreamInfo>,
    duration: Option<i64>,
    packets: Vec<Packet>,
}

fn demux(src: MemSource) -> Demuxed {
    block_on(async {
        let mut d = open(src).await.expect("open");
        let mut packets = Vec::new();
        while let Some(p) = d.next_packet().await.expect("next_packet") {
            packets.push(p);
        }
        Demuxed { streams: d.streams().to_vec(), duration: d.duration_us(), packets }
    })
}

fn parse_tb(s: &str) -> Rational {
    let (n, d) = s.split_once('/').unwrap();
    Rational::new(n.parse().unwrap(), d.parse().unwrap())
}

#[test]
fn streams_and_packets_match_ffprobe() {
    if skip() {
        return;
    }
    for &(name, has_dts) in FIXTURES {
        let want = probe(name);
        let got = demux(MemSource::new(bytes(name)));
        let ws = want["streams"].as_array().unwrap();
        assert_eq!(got.streams.len(), ws.len(), "{name}: stream count");

        for (i, (g, w)) in got.streams.iter().zip(ws).enumerate() {
            let ctx = format!("{name} stream {i}");
            assert_eq!(g.codec, w["codec_name"].as_str().unwrap(), "{ctx}: codec");
            let kind = match w["codec_type"].as_str().unwrap() {
                "video" => StreamKind::Video,
                "audio" => StreamKind::Audio,
                _ => StreamKind::Subtitle,
            };
            assert_eq!(g.kind, kind, "{ctx}: kind");
            let tb = parse_tb(w["time_base"].as_str().unwrap());
            assert_eq!(
                g.time_base.num as u64 * tb.den as u64,
                tb.num as u64 * g.time_base.den as u64,
                "{ctx}: time base"
            );
            if let Some(v) = g.video {
                assert_eq!(
                    (v.width as i64, v.height as i64),
                    (num(&w["width"]).unwrap(), num(&w["height"]).unwrap()),
                    "{ctx}"
                );
            }
            if let Some(a) = g.audio {
                assert_eq!(a.sample_rate as i64, num(&w["sample_rate"]).unwrap(), "{ctx}: rate");
                assert_eq!(a.channels as i64, num(&w["channels"]).unwrap(), "{ctx}: channels");
            }
            assert!(!g.extra_data.is_empty() || g.codec == "vp9", "{ctx}: codec config present");

            // Packets of this stream, in order, against ffprobe.
            let mine: Vec<&Packet> = got.packets.iter().filter(|p| p.stream_id == g.id).collect();
            let theirs: Vec<&serde_json::Value> =
                want["packets"].as_array().unwrap().iter().filter(|p| p["stream_index"] == i).collect();
            assert_eq!(mine.len(), theirs.len(), "{ctx}: packet count");
            for (n, (m, t)) in mine.iter().zip(&theirs).enumerate() {
                let pc = format!("{ctx} packet {n}");
                assert_eq!(m.pts, tb.ticks_to_us(num(&t["pts"]).unwrap()), "{pc}: pts");
                if has_dts {
                    assert_eq!(m.dts, tb.ticks_to_us(num(&t["dts"]).unwrap()), "{pc}: dts");
                }
                assert_eq!(m.data.len() as i64, num(&t["size"]).unwrap(), "{pc}: size");
                assert_eq!(m.keyframe, t["flags"].as_str().unwrap().starts_with('K'), "{pc}: keyframe");
            }
        }

        // Container duration: ffprobe's format duration, to within 1 ms (fragmented MP4 has no mvhd
        // duration and ffprobe estimates it differently from us, so 100 ms there).
        let want_dur =
            (want["format"]["duration"].as_str().unwrap().parse::<f64>().unwrap() * 1e6).round() as i64;
        let tol = if name.contains("frag") { 100_000 } else { 1_000 };
        let d = got.duration.expect("duration");
        assert!((d - want_dur).abs() <= tol, "{name}: duration {d} vs ffprobe {want_dur}");
    }
}

#[test]
fn packet_order_is_file_order_and_payloads_are_intact() {
    if skip() {
        return;
    }
    // Reading the same file through two different source behaviours must give byte-identical packets.
    for name in ["h264_aac.mp4", "h264_aac_frag.mp4", "av1_opus.webm", "h264_flac.mkv"] {
        let data = bytes(name);
        let a = demux(MemSource::new(data.clone()));
        let b = demux(MemSource::new(data.clone()).with_max_read(1).with_pending_polls(3));
        let c = demux(MemSource::new(data).with_max_read(7));
        assert_eq!(a.packets, b.packets, "{name}: 1-byte reads + Pending polls");
        assert_eq!(a.packets, c.packets, "{name}: 7-byte reads");
        assert_eq!(a.streams, b.streams, "{name}");
        assert_eq!(a.duration, b.duration, "{name}");
    }
}

#[test]
fn random_seeks_land_on_a_keyframe_at_or_before_the_target() {
    if skip() {
        return;
    }
    for &(name, _) in FIXTURES {
        let want = probe(name);
        let tb = parse_tb(want["streams"][0]["time_base"].as_str().unwrap());
        // Keyframe pts of stream 0 (the video stream), ascending.
        let mut keys: Vec<i64> = want["packets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["stream_index"] == 0 && p["flags"].as_str().unwrap().starts_with('K'))
            .map(|p| tb.ticks_to_us(num(&p["pts"]).unwrap()))
            .collect();
        keys.sort();
        assert!(keys.len() >= 10, "{name}: fixture should have many keyframes");
        let video_id = {
            let d = demux(MemSource::new(bytes(name)));
            d.streams.iter().find(|s| s.kind == StreamKind::Video).unwrap().id
        };

        let mut d: AnyDemuxer<MemSource> = block_on(open(MemSource::new(bytes(name)))).unwrap();
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let dur = d.duration_us().unwrap();
        for i in 0..1000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            // Mostly inside the file, plus out-of-range targets at both ends.
            let target = match i {
                0 => -5_000_000,
                1 => dur + 5_000_000,
                _ => (seed % (dur as u64 + 2_000_000)) as i64 - 1_000_000,
            };
            let want_land = keys.iter().rev().find(|&&k| k <= target).copied().unwrap_or(keys[0]);
            block_on(async {
                let landed = d.seek(target).await.unwrap();
                assert_eq!(landed, want_land, "{name}: target {target}");
                // The next video packet must be that keyframe.
                loop {
                    let p = d.next_packet().await.unwrap().expect("video packet after seek");
                    if p.stream_id == video_id {
                        assert!(p.keyframe, "{name}: first video packet after seek is a keyframe");
                        assert_eq!(
                            p.pts, landed,
                            "{name}: first video packet after seek is the landed keyframe"
                        );
                        break;
                    }
                }
            });
        }
    }
}

#[test]
fn truncated_and_corrupted_files_error_instead_of_panicking() {
    if skip() {
        return;
    }
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for &(name, _) in FIXTURES {
        let data = bytes(name);
        for _ in 0..40 {
            // Truncate at a random point, and flip a few random bytes in the head (where the metadata lives).
            let cut = (rnd() as usize) % data.len();
            let mut d = data[..cut].to_vec();
            for _ in 0..4 {
                let i = (rnd() as usize) % d.len().clamp(1, 4096);
                if i < d.len() {
                    d[i] ^= (rnd() & 0xFF) as u8;
                }
            }
            block_on(async {
                if let Ok(mut dm) = open(MemSource::new(d)).await {
                    let mut n = 0;
                    while let Ok(Some(_)) = dm.next_packet().await {
                        n += 1;
                        if n > 10_000 {
                            break;
                        }
                    }
                    let _ = dm.seek(1_000_000).await;
                }
            });
        }
    }
}
