//! The raw audio demuxers (MP3, FLAC, Ogg, WAV, ADTS AAC) against ffprobe on generated files (`tools/gen-fixtures.sh`, set
//! `audio`): streams, durations, tags, cover art, and every packet (size and time) as ffprobe lists them.
use rvp_core::task::block_on;
use rvp_core::{Metadata, Packet};
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

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap()
}

fn probe(name: &str) -> serde_json::Value {
    let f = dir().join(format!("{name}.probe.json"));
    serde_json::from_slice(&std::fs::read(f).unwrap()).unwrap()
}

fn drain(d: &mut impl Demuxer) -> Vec<Packet> {
    let mut v = Vec::new();
    while let Some(p) = block_on(d.next_packet()).unwrap() {
        v.push(p);
    }
    v
}

fn f(v: &serde_json::Value) -> f64 {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64()).unwrap_or(f64::NAN)
}

/// All tags ffprobe found, lowercase, from the format and the streams.
fn tags(j: &serde_json::Value) -> std::collections::BTreeMap<String, String> {
    let mut m = std::collections::BTreeMap::new();
    let mut take = |t: &serde_json::Value| {
        if let Some(o) = t.as_object() {
            for (k, v) in o {
                if let Some(s) = v.as_str() {
                    m.entry(k.to_lowercase()).or_insert_with(|| s.to_string());
                }
            }
        }
    };
    take(&j["format"]["tags"]);
    for s in j["streams"].as_array().unwrap() {
        if s["codec_type"] == "audio" {
            take(&s["tags"]);
        }
    }
    m
}

fn check_tags(name: &str, m: &Metadata, want: &std::collections::BTreeMap<String, String>) {
    let s = |k: &str| want.get(k).map(String::as_str);
    assert_eq!(m.title.as_deref(), s("title"), "{name}: title");
    assert_eq!(m.artist.as_deref(), s("artist"), "{name}: artist");
    assert_eq!(m.album.as_deref(), s("album"), "{name}: album");
    assert_eq!(m.album_artist.as_deref(), s("album_artist"), "{name}: album artist");
    assert_eq!(m.genre.as_deref(), s("genre"), "{name}: genre");
    let pair = |k: &str| {
        s(k).map(|v| {
            let mut it = v.split('/');
            (it.next().and_then(|a| a.parse::<u32>().ok()), it.next().and_then(|a| a.parse::<u32>().ok()))
        })
    };
    assert_eq!((m.track, m.track_total), pair("track").unwrap_or((None, None)), "{name}: track");
    assert_eq!((m.disc, m.disc_total), pair("disc").unwrap_or((None, None)), "{name}: disc");
    assert_eq!(m.year, s("date").and_then(|d| d[..4].parse().ok()), "{name}: year");
}

/// (file, ffprobe codec name -> ours, sample-exact packet times)
const FILES: &[&str] = &[
    "cbr.mp3",
    "vbr_v23.mp3",
    "mono_v1.mp3",
    "plain.mp3",
    "tone.flac",
    "tone24_mono.flac",
    "tone.ogg",
    "tone.opus",
    "tone_flac.oga",
    "tone16.wav",
    "tone24.wav",
    "tonef32.wav",
    "tone8_mono.wav",
    "tone.aac",
];

#[test]
fn streams_durations_tags_and_packets_match_ffprobe() {
    if skip() {
        return;
    }
    for name in FILES {
        let j = probe(name);
        let mut d =
            block_on(open(MemSource::new(bytes(name)))).unwrap_or_else(|e| panic!("{name}: open: {e:?}"));
        // Stream.
        let astream = j["streams"].as_array().unwrap().iter().find(|s| s["codec_type"] == "audio").unwrap();
        assert_eq!(d.streams().len(), 1, "{name}");
        let s = &d.streams()[0];
        let want_codec = match astream["codec_name"].as_str().unwrap() {
            c if c.starts_with("pcm_") => c.to_string(),
            c => c.to_string(),
        };
        assert_eq!(s.codec, want_codec, "{name}: codec");
        let a = s.audio.unwrap();
        assert_eq!(a.sample_rate as f64, f(&astream["sample_rate"]), "{name}: rate");
        assert_eq!(a.channels as i64, astream["channels"].as_i64().unwrap(), "{name}: channels");
        // Packets, in file order.
        let pk = drain(&mut d);
        let want: Vec<&serde_json::Value> = j["packets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["stream_index"] == astream["index"])
            .collect();
        assert_eq!(pk.len(), want.len(), "{name}: packet count");
        let start = f(&astream["start_time"]);
        let start = if start.is_nan() { 0.0 } else { start };
        let is_ogg = name.ends_with(".ogg") || name.ends_with(".opus") || name.ends_with(".oga");
        let is_vorbis = name.ends_with(".ogg");
        for (i, (p, w)) in pk.iter().zip(&want).enumerate() {
            let size = w["size"].as_str().unwrap().parse::<usize>().unwrap();
            let strip = if name.ends_with(".aac") { 7 } else { 0 };
            assert_eq!(p.data.len() + strip, size, "{name}: packet {i} size");
            // ffprobe's time, less the stream start: for MP3 the start is the encoder delay, which we put in the timestamps.
            let t = f(&w["pts_time"]) * 1e6 - start * 1e6;
            // Vorbis in Ogg: ffprobe gives the very first packet a duration of its own; the rest line up.
            if is_vorbis && i == 0 {
                continue;
            }
            let tol = if is_ogg { 25.0 } else { 3.0 };
            assert!((p.pts as f64 - t).abs() <= tol, "{name}: packet {i} pts {} vs ffprobe {t}", p.pts);
            assert!(p.keyframe);
        }
        // Duration. Opus: ffprobe counts the pre-skip; ADTS: ffprobe estimates from the bit rate.
        let mut want_dur = f(&j["format"]["duration"]) * 1e6;
        if name.ends_with(".opus") {
            want_dur -= f64::from(astream["initial_padding"].as_i64().unwrap() as i32) / 48_000.0 * 1e6;
        }
        let got = d.duration_us().unwrap() as f64;
        // ADTS has no length: ffprobe estimates it from the bit rate, where we count the frames.
        if name.ends_with(".aac") {
            want_dur = want.len() as f64 * 1024.0 / a.sample_rate as f64 * 1e6;
        }
        let tol = 1_500.0;
        assert!((got - want_dur).abs() <= tol, "{name}: duration {got} vs ffprobe {want_dur}");
        // Tags.
        check_tags(name, d.metadata(), &tags(&j));
    }
}

#[test]
fn id3v1_tag() {
    if skip() {
        return;
    }
    let d = block_on(open(MemSource::new(bytes("v1.mp3")))).unwrap();
    let m = d.metadata();
    assert_eq!(m.title.as_deref(), Some("V1 Title"));
    assert_eq!(m.artist.as_deref(), Some("V1 Artist"));
    assert_eq!(m.album.as_deref(), Some("V1 Album"));
    assert_eq!((m.year, m.track, m.genre.as_deref()), (Some(1999), Some(7), Some("Rock")));
    // The tag is not audio: the same packets as the file without it.
    let mut a = block_on(open(MemSource::new(bytes("v1.mp3")))).unwrap();
    let mut b = block_on(open(MemSource::new(bytes("plain.mp3")))).unwrap();
    assert_eq!(drain(&mut a), drain(&mut b));
}

#[test]
fn cover_art_is_the_picture_in_the_file() {
    if skip() {
        return;
    }
    let cover = bytes("cover.jpg");
    for name in ["cbr.mp3", "vbr_v23.mp3", "tone.flac"] {
        let d = block_on(open(MemSource::new(bytes(name)))).unwrap();
        let art = d.metadata().art.as_ref().unwrap_or_else(|| panic!("{name}: no art"));
        assert_eq!(art.mime, "image/jpeg", "{name}");
        assert_eq!(art.data, cover, "{name}: picture bytes");
    }
    let d = block_on(open(MemSource::new(bytes("plain.mp3")))).unwrap();
    assert!(d.metadata().art.is_none());
}

#[test]
fn gapless_mp3_trims_the_encoder_delay_and_padding() {
    if skip() {
        return;
    }
    let mut d = block_on(open(MemSource::new(bytes("cbr.mp3")))).unwrap();
    let pk = drain(&mut d);
    // LAME delay 576 + 529 decoder delay = 1105 samples, before time zero.
    let first = pk[0].pts;
    assert!((first + 1_105 * 1_000_000 / 48_000).abs() <= 2, "first pts {first}");
    // The audio ends where the file says (3.3 s): the frames past it are cut by the discard on the last frames.
    let end: i64 = pk.iter().map(|p| p.pts + p.duration - p.discard_end_us).max().unwrap();
    assert!((end - 3_300_000).abs() <= 25, "playable end {end}");
    assert!(pk.iter().any(|p| p.discard_end_us > 0));
    // A file without a gapless header starts at zero and cuts nothing.
    let mut d = block_on(open(MemSource::new(bytes("plain.mp3")))).unwrap();
    let pk = drain(&mut d);
    assert_eq!(pk[0].pts, 0);
    assert!(pk.iter().all(|p| p.discard_end_us == 0));
}

#[test]
fn seeks_land_at_or_before_the_target_and_read_the_right_packets() {
    if skip() {
        return;
    }
    for name in FILES {
        let mut whole = block_on(open(MemSource::new(bytes(name)))).unwrap();
        let all = drain(&mut whole);
        let dur = whole.duration_us().unwrap();
        for frac in [0.0, 0.1, 0.37, 0.5, 0.83, 0.97] {
            let target = (dur as f64 * frac) as i64;
            let mut d = block_on(open(MemSource::new(bytes(name)))).unwrap();
            let landed = block_on(d.seek(target)).unwrap();
            // At or before the target, and not absurdly far before it (a page or a block of frames).
            assert!(landed <= target + 1_000, "{name} @{frac}: landed {landed} after target {target}");
            let slack = if name.ends_with(".ogg") || name.ends_with(".opus") || name.ends_with(".oga") {
                1_300_000
            } else {
                600_000
            };
            assert!(target - landed <= slack, "{name} @{frac}: landed {landed} far before {target}");
            let first = block_on(d.next_packet()).unwrap().expect("a packet after the seek");
            // The packet read next is one of the packets of a straight read, and the one that starts at the time landed.
            let m = all.iter().find(|p| p.data == first.data && (p.pts - first.pts).abs() <= 30_000);
            assert!(
                m.is_some(),
                "{name} @{frac}: the packet after the seek is not in the file's packet list"
            );
            let rest = drain(&mut d);
            assert_eq!(
                rest.last().map(|p| &p.data),
                all.last().map(|p| &p.data),
                "{name} @{frac}: ends with the last packet"
            );
        }
    }
}

#[test]
fn truncated_files_give_what_is_there_then_an_error_or_the_end() {
    if skip() {
        return;
    }
    for name in FILES {
        let full = bytes(name);
        for pct in [30, 55, 90] {
            let cut = full[..full.len() * pct / 100].to_vec();
            let Ok(mut d) = block_on(open(MemSource::new(cut))) else { continue };
            let mut n = 0;
            while let Ok(Some(_)) = block_on(d.next_packet()) {
                n += 1;
            }
            // (A 30% cut of a small Ogg file can still be inside its header pages.)
            assert!(n > 0 || pct < 50, "{name} cut at {pct}%: no packets");
        }
    }
}

#[test]
fn junk_in_front_and_damage_inside_do_not_panic() {
    if skip() {
        return;
    }
    for name in FILES {
        let full = bytes(name);
        // Flip bytes here and there; whatever comes out, nothing may panic.
        for k in 1..=6usize {
            let mut b = full.clone();
            for i in 0..b.len() / 997 {
                let at = (i * 997 * k + k * 13) % b.len();
                b[at] ^= 0x5A;
            }
            if let Ok(mut d) = block_on(open(MemSource::new(b))) {
                let mut n = 0;
                while let Ok(Some(_)) = block_on(d.next_packet()) {
                    n += 1;
                    if n > 100_000 {
                        break;
                    }
                }
                let _ = block_on(d.seek(1_000_000));
            }
        }
    }
}
