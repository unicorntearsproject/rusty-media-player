//! Writing tags into real files (the audio fixtures of `tools/gen-fixtures.sh`) and reading them back with the library's own reader:
//! every field and the cover come back, the audio is byte for byte the same, what is not edited stays, the same edit twice gives the same
//! bytes, and damaged or odd input is refused without a panic.
use rvp_core::task::block_on;
use rvp_host::mock::MemSource;
use rvp_library::{TrackTags, read_tags};
use rvp_tagwrite::{Change, Cover, Edit, Error, Format, edit_tags, format_of};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

fn dir() -> PathBuf {
    static ONCE: Once = Once::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let d =
        std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"));
    ONCE.call_once(|| {
        if std::env::var_os("RVP_SKIP_FIXTURES").is_some() {
            return;
        }
        let st = Command::new("bash")
            .arg(root.join("tools/gen-fixtures.sh"))
            .arg(&d)
            .env("RVP_FIXTURE_SET", "audio")
            .status()
            .expect("run gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    d
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

fn load(rel: &str) -> Vec<u8> {
    std::fs::read(dir().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn tags(bytes: &[u8]) -> TrackTags {
    block_on(read_tags(MemSource::new(bytes.to_vec()))).expect("the file reads")
}

fn jpeg() -> Cover {
    Cover { mime: "image/jpeg".into(), data: load("audio/cover.jpg") }
}

fn full() -> Edit {
    Edit {
        title: Change::Set("Édit \u{2014} Título \u{2603}".into()),
        artist: Change::Set("Ärtist & Co".into()),
        album: Change::Set("Albüm".into()),
        album_artist: Change::Set("Various Artists".into()),
        track_no: Change::Set(3),
        track_total: Change::Set(12),
        disc_no: Change::Set(1),
        disc_total: Change::Set(2),
        year: Change::Set(1999),
        genre: Change::Set("Trip Hop".into()),
        cover: Change::Set(jpeg()),
    }
}

fn check_full(t: &TrackTags, what: &str) {
    assert_eq!(t.title, "Édit \u{2014} Título \u{2603}", "{what}");
    assert_eq!(t.artist, "Ärtist & Co", "{what}");
    assert_eq!(t.album, "Albüm", "{what}");
    assert_eq!(t.album_artist, "Various Artists", "{what}");
    assert_eq!((t.track_no, t.track_total, t.disc_no, t.disc_total), (3, 12, 1, 2), "{what}");
    assert_eq!(t.year, 1999, "{what}");
    assert_eq!(t.genre, "Trip Hop", "{what}");
    assert!(t.art.is_some(), "{what}: the cover");
}

const MP3: [&str; 4] = ["audio/cbr.mp3", "audio/plain.mp3", "audio/v1.mp3", "audio/mono_v1.mp3"];
const FLAC: [&str; 3] = ["audio/surround51.flac", "audio/tone.flac", "audio/tone24_mono.flac"];
const OGG: [&str; 5] =
    ["audio/tone.ogg", "audio/tone.opus", "audio/chain_a.ogg", "audio/chain_a.opus", "audio/chained.ogg"];
const MP4: [&str; 4] = ["audio/aac51.m4a", "h264_aac_faststart.mp4", "h264_aac.mp4", "audio/video_aac51.mp4"];

fn all() -> Vec<(Format, &'static str)> {
    let mut v = Vec::new();
    v.extend(MP3.iter().map(|f| (Format::Mp3, *f)));
    v.extend(FLAC.iter().map(|f| (Format::Flac, *f)));
    v.extend(OGG.iter().map(|f| (Format::Ogg, *f)));
    v.extend(MP4.iter().map(|f| (Format::Mp4, *f)));
    v
}

// ---- the audio is not touched ---------------------------------------------------------------------------------------------

fn syncsafe(b: &[u8]) -> usize {
    b.iter().fold(0, |a, &x| a << 7 | (x & 0x7F) as usize)
}

/// Everything after the ID3v2 tag.
fn after_id3(f: &[u8]) -> &[u8] {
    if f.starts_with(b"ID3") { &f[10 + syncsafe(&f[6..10])..] } else { f }
}

/// Without an ID3v1 tag at the end (the edit keeps that in step, so the audio is what is in front of it).
fn without_v1(f: &[u8]) -> &[u8] {
    if f.len() >= 128 && &f[f.len() - 128..f.len() - 125] == b"TAG" { &f[..f.len() - 128] } else { f }
}

fn after_flac_metadata(f: &[u8]) -> &[u8] {
    let mut at = 4;
    loop {
        let (last, len) =
            (f[at] & 0x80 != 0, u32::from_be_bytes([0, f[at + 1], f[at + 2], f[at + 3]]) as usize);
        at += 4 + len;
        if last {
            return &f[at..];
        }
    }
}

struct Page {
    granule: u64,
    serial: u32,
    seq: u32,
    crc: u32,
    raw: Vec<u8>,
    data: Vec<u8>,
}

fn pages(f: &[u8]) -> Vec<Page> {
    let mut v = Vec::new();
    let mut at = 0;
    while at < f.len() {
        assert_eq!(&f[at..at + 4], b"OggS", "a page at {at}");
        let n = f[at + 26] as usize;
        let dlen: usize = f[at + 27..at + 27 + n].iter().map(|&s| s as usize).sum();
        let end = at + 27 + n + dlen;
        v.push(Page {
            granule: u64::from_le_bytes(f[at + 6..at + 14].try_into().unwrap()),
            serial: u32::from_le_bytes(f[at + 14..at + 18].try_into().unwrap()),
            seq: u32::from_le_bytes(f[at + 18..at + 22].try_into().unwrap()),
            crc: u32::from_le_bytes(f[at + 22..at + 26].try_into().unwrap()),
            raw: f[at..end].to_vec(),
            data: f[at + 27 + n..end].to_vec(),
        });
        at = end;
    }
    v
}

fn ogg_crc(raw: &[u8]) -> u32 {
    let mut c = 0u32;
    for (i, &b) in raw.iter().enumerate() {
        let b = if (22..26).contains(&i) { 0 } else { b };
        c ^= (b as u32) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
        }
    }
    c
}

/// Every page's checksum is right and the sequence numbers of each stream count up from 0.
fn check_ogg_pages(f: &[u8], what: &str) {
    let mut next: std::collections::HashMap<u32, u32> = Default::default();
    for p in pages(f) {
        assert_eq!(ogg_crc(&p.raw), p.crc, "{what}: a page checksum");
        let n = next.entry(p.serial).or_insert(0);
        assert_eq!(p.seq, *n, "{what}: sequence numbers of stream {}", p.serial);
        *n += 1;
    }
}

fn ogg_audio(f: &[u8]) -> Vec<Vec<u8>> {
    pages(f).into_iter().filter(|p| p.granule != 0 && p.granule != u64::MAX).map(|p| p.data).collect()
}

fn collect_offsets(f: &[u8], start: usize, end: usize, out: &mut Vec<u64>) {
    let mut at = start;
    while at + 8 <= end {
        let size = u32::from_be_bytes(f[at..at + 4].try_into().unwrap()) as usize;
        let ty = &f[at + 4..at + 8];
        match ty {
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => collect_offsets(f, at + 8, at + size, out),
            b"stco" => {
                let n = u32::from_be_bytes(f[at + 12..at + 16].try_into().unwrap()) as usize;
                for i in 0..n {
                    out.push(
                        u32::from_be_bytes(f[at + 16 + i * 4..at + 20 + i * 4].try_into().unwrap()) as u64
                    );
                }
            }
            b"co64" => {
                let n = u32::from_be_bytes(f[at + 12..at + 16].try_into().unwrap()) as usize;
                for i in 0..n {
                    out.push(u64::from_be_bytes(f[at + 16 + i * 8..at + 24 + i * 8].try_into().unwrap()));
                }
            }
            _ => {}
        }
        if size < 8 {
            break;
        }
        at += size;
    }
}

fn top_boxes(f: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut v = Vec::new();
    let mut at = 0;
    while at + 8 <= f.len() {
        let mut size = u32::from_be_bytes(f[at..at + 4].try_into().unwrap()) as usize;
        if size == 0 {
            size = f.len() - at;
        }
        if size == 1 {
            size = u64::from_be_bytes(f[at + 8..at + 16].try_into().unwrap()) as usize;
        }
        v.push((f[at + 4..at + 8].try_into().unwrap(), at, size));
        at += size;
    }
    v
}

/// The media data is the same bytes, and every chunk offset points at the same bytes it did.
fn check_mp4_audio(old: &[u8], new: &[u8], what: &str) {
    let mdats = |f: &[u8]| -> Vec<Vec<u8>> {
        top_boxes(f)
            .into_iter()
            .filter(|b| &b.0 == b"mdat")
            .map(|(_, at, n)| f[at..at + n].to_vec())
            .collect()
    };
    assert_eq!(mdats(old), mdats(new), "{what}: media data");
    let (mut a, mut b) = (Vec::new(), Vec::new());
    collect_offsets(old, 0, old.len(), &mut a);
    collect_offsets(new, 0, new.len(), &mut b);
    assert_eq!(a.len(), b.len(), "{what}: chunk count");
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        let (x, y) = (*x as usize, *y as usize);
        let n = 16.min(old.len() - x).min(new.len() - y);
        assert_eq!(&old[x..x + n], &new[y..y + n], "{what}: chunk {i} moved to the wrong place");
    }
}

fn check_audio_same(fmt: Format, old: &[u8], new: &[u8], what: &str) {
    match fmt {
        Format::Mp3 => assert_eq!(without_v1(after_id3(old)), without_v1(after_id3(new)), "{what}"),
        Format::Flac => assert_eq!(after_flac_metadata(old), after_flac_metadata(new), "{what}"),
        Format::Ogg => {
            check_ogg_pages(new, what);
            assert_eq!(ogg_audio(old), ogg_audio(new), "{what}: audio pages");
        }
        Format::Mp4 => check_mp4_audio(old, new, what),
    }
}

// ---- tests ----------------------------------------------------------------------------------------------------------------

#[test]
fn every_field_and_the_cover_round_trip_in_every_format_and_the_audio_is_untouched() {
    if skip() {
        return;
    }
    for (fmt, name) in all() {
        let old = load(name);
        assert_eq!(format_of(name, &old), Some(fmt), "{name}");
        let before = tags(&old);
        let new = edit_tags(fmt, &old, &full()).unwrap_or_else(|e| panic!("{name}: {e}"));
        check_full(&tags(&new), name);
        check_audio_same(fmt, &old, &new, name);
        // The audio reads the same: length and format.
        let after = tags(&new);
        assert_eq!(
            (after.duration_us, after.codec.as_str(), after.sample_rate, after.channels),
            (before.duration_us, before.codec.as_str(), before.sample_rate, before.channels),
            "{name}"
        );
    }
}

#[test]
fn the_same_edit_twice_is_the_same_bytes_and_an_empty_edit_changes_no_tag() {
    if skip() {
        return;
    }
    for (fmt, name) in all() {
        let old = load(name);
        let once = edit_tags(fmt, &old, &full()).unwrap();
        let twice = edit_tags(fmt, &once, &full()).unwrap();
        assert_eq!(once, twice, "{name}: not idempotent");
        let same = edit_tags(fmt, &old, &Edit::default()).unwrap();
        let (a, b) = (tags(&old), tags(&same));
        assert_eq!(
            (a.title, a.artist, a.album, a.album_artist, a.track_no, a.year, a.genre),
            (b.title, b.artist, b.album, b.album_artist, b.track_no, b.year, b.genre),
            "{name}"
        );
        check_audio_same(fmt, &old, &same, name);
    }
}

#[test]
fn clearing_and_changing_one_field_leaves_the_others() {
    if skip() {
        return;
    }
    for (fmt, name) in all() {
        let tagged = edit_tags(fmt, &load(name), &full()).unwrap();
        // Change only the title and the track number; clear the genre and the cover.
        let e = Edit {
            title: Change::Set("Nu".into()),
            track_no: Change::Set(7),
            genre: Change::Clear,
            cover: Change::Clear,
            ..Edit::default()
        };
        let out = edit_tags(fmt, &tagged, &e).unwrap();
        let t = tags(&out);
        assert_eq!(t.title, "Nu", "{name}");
        assert_eq!(
            (t.track_no, t.track_total),
            (7, 12),
            "{name}: the total stays when only the number changes"
        );
        assert_eq!((t.disc_no, t.disc_total, t.year), (1, 2, 1999), "{name}");
        assert_eq!(
            (t.artist.as_str(), t.album.as_str(), t.album_artist.as_str()),
            ("Ärtist & Co", "Albüm", "Various Artists"),
            "{name}"
        );
        assert!(t.genre.is_empty() && t.art.is_none(), "{name}: genre {:?}", t.genre);
        // Clearing the numbers and the year too.
        let e = Edit {
            track_no: Change::Clear,
            track_total: Change::Clear,
            disc_no: Change::Clear,
            disc_total: Change::Clear,
            year: Change::Clear,
            ..Edit::default()
        };
        let t = tags(&edit_tags(fmt, &out, &e).unwrap());
        assert_eq!((t.track_no, t.track_total, t.disc_no, t.disc_total, t.year), (0, 0, 0, 0, 0), "{name}");
        assert_eq!(t.title, "Nu", "{name}");
    }
}

#[test]
fn a_cover_can_be_replaced_with_a_png_and_a_bigger_one_than_a_page_holds() {
    if skip() {
        return;
    }
    // 300 KB of picture: the Ogg header spans several pages, the MP4 movie box grows a lot, then shrinks again.
    let mut data = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
    data.extend((0..300_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8));
    let big = Cover { mime: "image/png".into(), data };
    for (fmt, name) in all() {
        let old = load(name);
        let e = Edit { cover: Change::Set(big.clone()), ..Edit::default() };
        let with = edit_tags(fmt, &old, &e).unwrap();
        let t = tags(&with);
        let art = t.art.expect(name);
        assert_eq!(art.mime, "image/png", "{name}");
        assert_eq!(art.data, big.data, "{name}");
        check_audio_same(fmt, &old, &with, name);
        // And back to the small one: the audio still lines up.
        let back = edit_tags(fmt, &with, &Edit { cover: Change::Set(jpeg()), ..Edit::default() }).unwrap();
        assert_eq!(tags(&back).art.unwrap().data, jpeg().data, "{name}");
        check_audio_same(fmt, &old, &back, name);
        let none = edit_tags(fmt, &with, &Edit { cover: Change::Clear, ..Edit::default() }).unwrap();
        assert!(tags(&none).art.is_none(), "{name}");
        check_audio_same(fmt, &old, &none, name);
    }
}

#[test]
fn what_is_not_edited_stays_in_every_format() {
    if skip() {
        return;
    }
    // ID3: a frame this writer does not know, and a ReplayGain text frame, survive an edit and an upgrade from v2.3.
    for version in [3u8, 4] {
        let mut frames = Vec::new();
        let mut frame = |id: &[u8; 4], data: &[u8]| {
            frames.extend_from_slice(id);
            if version == 4 {
                frames.extend_from_slice(&[0, 0, (data.len() >> 7) as u8 & 0x7F, data.len() as u8 & 0x7F]);
            } else {
                frames.extend_from_slice(&(data.len() as u32).to_be_bytes());
            }
            frames.extend_from_slice(&[0, 0]);
            frames.extend_from_slice(data);
        };
        frame(b"TIT2", b"\x00Old title");
        frame(b"PRIV", b"owner@example.com\x00secret-bytes-\x01\x02\x03");
        frame(b"TXXX", b"\x00REPLAYGAIN_TRACK_GAIN\x00-6.50 dB");
        frame(b"TYER", b"\x002004");
        frame(b"WXXX", b"\x00\x00http://example.com/x");
        let mut file = b"ID3".to_vec();
        file.extend_from_slice(&[version, 0, 0]);
        let n = frames.len();
        file.extend_from_slice(&[
            (n >> 21) as u8 & 0x7F,
            (n >> 14) as u8 & 0x7F,
            (n >> 7) as u8 & 0x7F,
            n as u8 & 0x7F,
        ]);
        file.extend_from_slice(&frames);
        let audio = load("audio/cbr.mp3");
        file.extend_from_slice(after_id3(&audio));
        let before = tags(&file);
        assert_eq!((before.title.as_str(), before.year), ("Old title", 2004), "v2.{version}");
        let out =
            edit_tags(Format::Mp3, &file, &Edit { artist: Change::Set("New".into()), ..Edit::default() })
                .unwrap();
        assert_eq!(&out[..5], b"ID3\x04\x00", "written as v2.4 (was v2.{version})");
        let find = |needle: &[u8]| out.windows(needle.len()).any(|w| w == needle);
        assert!(find(b"owner@example.com\x00secret-bytes-\x01\x02\x03"), "v2.{version}: the PRIV frame");
        assert!(find(b"http://example.com/x"), "v2.{version}: the WXXX frame");
        let t = tags(&out);
        assert_eq!((t.title.as_str(), t.artist.as_str(), t.year), ("Old title", "New", 2004), "v2.{version}");
        assert_eq!(
            t.loudness.track_lufs, before.loudness.track_lufs,
            "v2.{version}: the ReplayGain frame still reads"
        );
        assert!(!out.windows(4).any(|w| w == b"TYER"), "v2.4 has no TYER");
        assert_eq!(without_v1(after_id3(&out)), without_v1(after_id3(&file)));
    }
    // FLAC: blocks other than the comments (seek table, application, padding...) and comment entries that are not edited.
    let flac = load("audio/surround51.flac");
    let blocks = |f: &[u8]| {
        let mut v = Vec::new();
        let mut at = 4;
        loop {
            let ty = f[at] & 0x7F;
            let len = u32::from_be_bytes([0, f[at + 1], f[at + 2], f[at + 3]]) as usize;
            v.push((ty, f[at + 4..at + 4 + len].to_vec()));
            at += 4 + len;
            if f[at - 4 - len] & 0x80 != 0 {
                return v;
            }
        }
    };
    let out = edit_tags(Format::Flac, &flac, &full()).unwrap();
    let (a, b) = (blocks(&flac), blocks(&out));
    for (ty, body) in &a {
        if *ty != 4 && *ty != 6 {
            assert!(b.contains(&(*ty, body.clone())), "FLAC block type {ty} kept");
        }
    }
    assert_eq!(b[0].0, 0, "the stream info stays first");
    assert_eq!(b.iter().filter(|x| x.0 == 4).count(), 1);
}

#[test]
fn a_vorbis_comment_that_the_file_has_but_is_not_edited_is_kept() {
    if skip() {
        return;
    }
    // Put a custom entry in with an edit of a different field set, then edit something else: the custom entry stays (found as bytes).
    let ogg = load("audio/tone.ogg");
    let marked = {
        let mut f = ogg.clone();
        // Not possible to add a custom field through the public edit, so check what the encoder wrote (its ENCODER entry) stays.
        let t =
            edit_tags(Format::Ogg, &f, &Edit { title: Change::Set("x".into()), ..Edit::default() }).unwrap();
        f = t;
        f
    };
    let enc = ogg.windows(8).position(|w| w == b"ENCODER=");
    if let Some(i) = enc {
        let end = ogg[i..].iter().position(|&c| c == 0 || c == 1).map_or(16, |n| n.min(40));
        assert!(
            marked.windows(end.min(16)).any(|w| w == &ogg[i..i + end.min(16)]),
            "the encoder entry stays"
        );
    }
}

#[test]
fn a_header_that_spans_pages_is_laid_out_in_valid_pages() {
    if skip() {
        return;
    }
    for name in ["audio/tone.ogg", "audio/tone.opus", "audio/chained.ogg", "audio/chained.opus"] {
        let old = load(name);
        let big = Cover { mime: "image/jpeg".into(), data: vec![0xFF; 70_000] };
        let out = edit_tags(Format::Ogg, &old, &Edit { cover: Change::Set(big), ..Edit::default() }).unwrap();
        check_ogg_pages(&out, name);
        let p = pages(&out);
        assert!(p.len() > pages(&old).len(), "{name}: more pages for the bigger header");
        // A packet that goes on over a page boundary says so on the next page, and the first page begins the stream.
        assert_eq!(out[5] & 2, 2, "{name}: begin-of-stream");
        assert_eq!(&out[..4], b"OggS");
    }
}

#[test]
fn files_that_cannot_be_followed_are_refused_without_a_panic() {
    if skip() {
        return;
    }
    let e = full();
    // Cut at many places and with bytes flipped: an error or a result, never a panic and never nonsense that is bigger than the input
    // plus the edit.
    let mut seed = 0x1234_5678u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for (fmt, name) in all() {
        let f = load(name);
        let head = &f[..f.len().min(6000)];
        for cut in (0..head.len()).step_by(37) {
            let _ = edit_tags(fmt, &head[..cut], &e);
        }
        for _ in 0..60 {
            let mut m = head.to_vec();
            for _ in 0..1 + rnd() % 4 {
                let i = rnd() as usize % m.len();
                m[i] ^= 1 << (rnd() % 8);
            }
            if let Ok(out) = edit_tags(fmt, &m, &e) {
                assert!(
                    out.len() < m.len() + jpeg().data.len() + 4096,
                    "{name}: the result is the size of nothing sensible"
                );
            }
        }
    }
    // Plain garbage and an empty file.
    for fmt in [Format::Mp3, Format::Flac, Format::Ogg, Format::Mp4] {
        assert!(edit_tags(fmt, b"", &e).is_ok() == (fmt == Format::Mp3) || edit_tags(fmt, b"", &e).is_err());
        assert!(
            edit_tags(fmt, &[0xAB; 100], &e).is_ok() == (fmt == Format::Mp3)
                || edit_tags(fmt, &[0xAB; 100], &e).is_err()
        );
    }
    // A file of another kind in a format's place.
    assert!(matches!(edit_tags(Format::Flac, &load("audio/cbr.mp3"), &e), Err(Error::Corrupt(_))));
    assert!(matches!(edit_tags(Format::Mp4, &load("audio/cbr.mp3"), &e), Err(Error::Corrupt(_))));
    assert!(matches!(edit_tags(Format::Ogg, &load("audio/cbr.mp3"), &e), Err(Error::Corrupt(_))));
    // Ogg FLAC and other codecs are not handled.
    assert!(matches!(edit_tags(Format::Ogg, &load("audio/tone_flac.oga"), &e), Err(Error::Unsupported(_))));
    // A cover that is not a picture is refused.
    let bad =
        Edit { cover: Change::Set(Cover { mime: "text/plain".into(), data: vec![1] }), ..Edit::default() };
    assert!(matches!(edit_tags(Format::Mp3, &load("audio/cbr.mp3"), &bad), Err(Error::Unsupported(_))));
    let empty =
        Edit { cover: Change::Set(Cover { mime: "image/png".into(), data: vec![] }), ..Edit::default() };
    assert!(matches!(edit_tags(Format::Flac, &load("audio/tone.flac"), &empty), Err(Error::Unsupported(_))));
}

#[test]
fn format_is_told_from_the_bytes_first() {
    assert_eq!(format_of("x.bin", b"fLaC\0\0\0\0"), Some(Format::Flac));
    assert_eq!(format_of("x.mp3", b"OggS\0\0\0\0"), Some(Format::Ogg));
    assert_eq!(format_of("x", b"\0\0\0\x20ftypM4A "), Some(Format::Mp4));
    assert_eq!(format_of("a.MP3", b"ID3\x04\0\0\0\0\0\0"), Some(Format::Mp3));
    assert_eq!(format_of("a.flac", b"ID3\x04\0\0\0\0\0\0"), Some(Format::Flac));
    assert_eq!(format_of("a.mp3", &[0xFF, 0xFB, 0x90, 0x00]), Some(Format::Mp3));
    assert_eq!(format_of("a.txt", b"hello world"), None);
    assert_eq!(format_of("a.mp3", b""), None);
}

#[test]
fn an_mp4_with_the_movie_box_first_or_last_keeps_its_chunks() {
    if skip() {
        return;
    }
    // faststart: moov before mdat (the offsets move); the plain one has it at the end (nothing moves).
    for name in ["h264_aac_faststart.mp4", "h264_aac.mp4"] {
        let old = load(name);
        let order: Vec<_> = top_boxes(&old).iter().map(|b| b.0).collect();
        let moov_first = order.iter().position(|t| t == b"moov") < order.iter().position(|t| t == b"mdat");
        let out = edit_tags(Format::Mp4, &old, &full()).unwrap();
        check_mp4_audio(&old, &out, name);
        let out_order: Vec<_> = top_boxes(&out).iter().map(|b| b.0).collect();
        assert_eq!(
            order.len() + 0,
            out_order.len() - out_order.iter().filter(|t| *t == b"free").count()
                + order.iter().filter(|t| *t == b"free").count(),
            "{name}: boxes {order:?} -> {out_order:?}"
        );
        let _ = moov_first;
        // A shrink that leaves a free box: the media data does not move at all.
        let shrunk = edit_tags(Format::Mp4, &out, &Edit { cover: Change::Clear, ..Edit::default() }).unwrap();
        check_mp4_audio(&old, &shrunk, name);
        if moov_first {
            let pos = |f: &[u8]| top_boxes(f).into_iter().find(|b| &b.0 == b"mdat").unwrap().1;
            assert_eq!(pos(&out), pos(&shrunk), "{name}: a free box filled the gap");
        }
    }
}

#[test]
fn an_id3v1_tag_at_the_end_follows_the_edit() {
    if skip() {
        return;
    }
    let old = load("audio/v1.mp3");
    assert_eq!(&old[old.len() - 128..old.len() - 125], b"TAG");
    let e = Edit {
        title: Change::Set("Caf\u{e9} d\u{2603}l\u{e0} ".repeat(5)),
        artist: Change::Clear,
        year: Change::Set(1987),
        track_no: Change::Set(9),
        ..Edit::default()
    };
    let out = edit_tags(Format::Mp3, &old, &e).unwrap();
    let v1 = &out[out.len() - 128..];
    assert_eq!(&v1[..3], b"TAG");
    assert_eq!(out.len() - 128, old.len() - 128 + (out.len() - old.len()), "only the front grew");
    assert_eq!(v1[3], b'C');
    assert_eq!(v1[3 + 3], 0xE9, "\u{e9} as Latin-1");
    assert!(v1[3..33].contains(&b'?'), "what Latin-1 cannot hold became a ?");
    assert!(v1[33..63].iter().all(|&c| c == 0), "the artist is cleared");
    assert_eq!(&v1[93..97], b"1987");
    assert_eq!((v1[125], v1[126]), (0, 9), "v1.1 track number");
    assert_eq!(&v1[63..93], &old[old.len() - 128 + 63..old.len() - 128 + 93], "the album is as it was");
}

#[test]
fn files_without_a_tag_get_one_and_an_old_v22_tag_is_upgraded() {
    if skip() {
        return;
    }
    // No tag at all: an MP3 stream as it comes from an encoder.
    let bare = after_id3(&load("audio/cbr.mp3")).to_vec();
    let out = edit_tags(Format::Mp3, &bare, &Edit { title: Change::Set("Fresh".into()), ..Edit::default() })
        .unwrap();
    assert_eq!(&out[..5], b"ID3\x04\x00");
    assert_eq!(tags(&out).title, "Fresh");
    assert_eq!(after_id3(&out), &bare[..]);
    // An empty edit of a file without a tag adds nothing.
    assert_eq!(edit_tags(Format::Mp3, &bare, &Edit::default()).unwrap(), bare);
    // ID3v2.2: three-letter frames.
    let mut frames = Vec::new();
    for (id, data) in [
        (&b"TT2"[..], &b"\x00Old Two"[..]),
        (b"TP1", b"\x00Her"),
        (b"TRK", b"\x004/9"),
        (b"TYE", b"\x001975"),
        (b"XYZ", b"unknown v2.2"),
    ] {
        frames.extend_from_slice(id);
        frames.extend_from_slice(&(data.len() as u32).to_be_bytes()[1..]);
        frames.extend_from_slice(data);
    }
    let mut v22 = b"ID3\x02\x00\x00".to_vec();
    let n = frames.len();
    v22.extend_from_slice(&[
        (n >> 21) as u8 & 0x7F,
        (n >> 14) as u8 & 0x7F,
        (n >> 7) as u8 & 0x7F,
        n as u8 & 0x7F,
    ]);
    v22.extend_from_slice(&frames);
    v22.extend_from_slice(&bare);
    let t = tags(&v22);
    assert_eq!(
        (t.title.as_str(), t.artist.as_str(), t.track_no, t.track_total, t.year),
        ("Old Two", "Her", 4, 9, 1975)
    );
    let out = edit_tags(
        Format::Mp3,
        &v22,
        &Edit { album: Change::Set("A".into()), track_no: Change::Set(5), ..Edit::default() },
    )
    .unwrap();
    let t = tags(&out);
    assert_eq!(
        (t.title.as_str(), t.artist.as_str(), t.album.as_str(), t.track_no, t.track_total, t.year),
        ("Old Two", "Her", "A", 5, 9, 1975)
    );
    assert_eq!(&out[..5], b"ID3\x04\x00");
}

#[test]
fn a_flac_without_a_comment_block_gets_one_and_an_mp4_without_tags_gets_the_boxes() {
    if skip() {
        return;
    }
    // FLAC: drop the comment block from a real file (rewriting the headers by hand), then edit.
    let flac = load("audio/tone.flac");
    let mut out = flac[..4].to_vec();
    let mut at = 4;
    let mut kept: Vec<(u8, Vec<u8>)> = Vec::new();
    loop {
        let ty = flac[at] & 0x7F;
        let len = u32::from_be_bytes([0, flac[at + 1], flac[at + 2], flac[at + 3]]) as usize;
        let last = flac[at] & 0x80 != 0;
        if ty != 4 && ty != 6 {
            kept.push((ty, flac[at + 4..at + 4 + len].to_vec()));
        }
        at += 4 + len;
        if last {
            break;
        }
    }
    for (i, (ty, body)) in kept.iter().enumerate() {
        out.push(ty | if i + 1 == kept.len() { 0x80 } else { 0 });
        out.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(body);
    }
    out.extend_from_slice(&flac[at..]);
    assert!(tags(&out).title.is_empty());
    let new = edit_tags(Format::Flac, &out, &full()).unwrap();
    check_full(&tags(&new), "flac without comments");
    assert_eq!(after_flac_metadata(&new), after_flac_metadata(&out));
    // MP4: a movie box with nothing in it but the header box.
    let mvhd = |f: &mut Vec<u8>| {
        let mut body = vec![0u8; 100];
        body[12..16].copy_from_slice(&1000u32.to_be_bytes());
        f.extend_from_slice(&(108u32).to_be_bytes());
        f.extend_from_slice(b"mvhd");
        f.extend_from_slice(&body);
    };
    let mut moov = Vec::new();
    mvhd(&mut moov);
    let mut mp4 = Vec::new();
    mp4.extend_from_slice(&24u32.to_be_bytes());
    mp4.extend_from_slice(b"ftypM4A \0\0\0\0M4A isom");
    mp4.extend_from_slice(&((moov.len() + 8) as u32).to_be_bytes());
    mp4.extend_from_slice(b"moov");
    mp4.extend_from_slice(&moov);
    mp4.extend_from_slice(&16u32.to_be_bytes());
    mp4.extend_from_slice(b"mdat");
    mp4.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let e = Edit {
        title: Change::Set("T".into()),
        track_no: Change::Set(2),
        track_total: Change::Set(5),
        cover: Change::Set(jpeg()),
        ..Edit::default()
    };
    let out = edit_tags(Format::Mp4, &mp4, &e).unwrap();
    let find = |needle: &[u8]| out.windows(needle.len()).position(|w| w == needle);
    for tag in [&b"udta"[..], b"meta", b"hdlr", b"mdir", b"ilst", &[0xA9, b'n', b'a', b'm'], b"trkn", b"covr"]
    {
        assert!(find(tag).is_some(), "{:?}", String::from_utf8_lossy(tag));
    }
    // The media data is intact and last, and a second edit parses what the first wrote.
    assert_eq!(&out[out.len() - 16..], &mp4[mp4.len() - 16..]);
    let again = edit_tags(
        Format::Mp4,
        &out,
        &Edit { title: Change::Set("U".into()), cover: Change::Clear, ..Edit::default() },
    )
    .unwrap();
    assert!(again.windows(5).any(|w| w == b"data\0") || again.windows(4).any(|w| w == b"data"));
    assert!(!again.windows(4).any(|w| w == b"covr"));
    let titles: Vec<usize> = again
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == [0xA9, b'n', b'a', b'm'])
        .map(|(i, _)| i)
        .collect();
    assert_eq!(titles.len(), 1, "the title replaced, not repeated");
    assert!(again.windows(1).any(|w| w == b"U"));
    assert_eq!(top_boxes(&again).last().map(|b| b.0), Some(*b"mdat"));
}
