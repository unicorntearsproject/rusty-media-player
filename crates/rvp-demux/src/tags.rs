//! Tag formats shared by the raw audio demuxers: ID3v1 and ID3v2 (MP3, AAC, sometimes FLAC and WAV), Vorbis comments
//! (FLAC, Ogg), the FLAC `PICTURE` block, and the base64 that Ogg uses to carry it.
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_core::{Art, Metadata};

/// The largest cover picture kept.
pub(crate) const MAX_ART: usize = 16 << 20;
/// The largest ID3v2 tag read.
pub(crate) const MAX_ID3: u64 = 32 << 20;

/// ID3v1 genre names (index 0..=191, the original 80 and the Winamp extensions).
const GENRES: [&str; 192] = [
    "Blues",
    "Classic Rock",
    "Country",
    "Dance",
    "Disco",
    "Funk",
    "Grunge",
    "Hip-Hop",
    "Jazz",
    "Metal",
    "New Age",
    "Oldies",
    "Other",
    "Pop",
    "R&B",
    "Rap",
    "Reggae",
    "Rock",
    "Techno",
    "Industrial",
    "Alternative",
    "Ska",
    "Death Metal",
    "Pranks",
    "Soundtrack",
    "Euro-Techno",
    "Ambient",
    "Trip-Hop",
    "Vocal",
    "Jazz+Funk",
    "Fusion",
    "Trance",
    "Classical",
    "Instrumental",
    "Acid",
    "House",
    "Game",
    "Sound Clip",
    "Gospel",
    "Noise",
    "Alternative Rock",
    "Bass",
    "Soul",
    "Punk",
    "Space",
    "Meditative",
    "Instrumental Pop",
    "Instrumental Rock",
    "Ethnic",
    "Gothic",
    "Darkwave",
    "Techno-Industrial",
    "Electronic",
    "Pop-Folk",
    "Eurodance",
    "Dream",
    "Southern Rock",
    "Comedy",
    "Cult",
    "Gangsta",
    "Top 40",
    "Christian Rap",
    "Pop/Funk",
    "Jungle",
    "Native American",
    "Cabaret",
    "New Wave",
    "Psychedelic",
    "Rave",
    "Showtunes",
    "Trailer",
    "Lo-Fi",
    "Tribal",
    "Acid Punk",
    "Acid Jazz",
    "Polka",
    "Retro",
    "Musical",
    "Rock & Roll",
    "Hard Rock",
    "Folk",
    "Folk-Rock",
    "National Folk",
    "Swing",
    "Fast Fusion",
    "Bebop",
    "Latin",
    "Revival",
    "Celtic",
    "Bluegrass",
    "Avantgarde",
    "Gothic Rock",
    "Progressive Rock",
    "Psychedelic Rock",
    "Symphonic Rock",
    "Slow Rock",
    "Big Band",
    "Chorus",
    "Easy Listening",
    "Acoustic",
    "Humour",
    "Speech",
    "Chanson",
    "Opera",
    "Chamber Music",
    "Sonata",
    "Symphony",
    "Booty Bass",
    "Primus",
    "Porn Groove",
    "Satire",
    "Slow Jam",
    "Club",
    "Tango",
    "Samba",
    "Folklore",
    "Ballad",
    "Power Ballad",
    "Rhythmic Soul",
    "Freestyle",
    "Duet",
    "Punk Rock",
    "Drum Solo",
    "A cappella",
    "Euro-House",
    "Dance Hall",
    "Goa",
    "Drum & Bass",
    "Club-House",
    "Hardcore",
    "Terror",
    "Indie",
    "BritPop",
    "Negerpunk",
    "Polsk Punk",
    "Beat",
    "Christian Gangsta Rap",
    "Heavy Metal",
    "Black Metal",
    "Crossover",
    "Contemporary Christian",
    "Christian Rock",
    "Merengue",
    "Salsa",
    "Thrash Metal",
    "Anime",
    "JPop",
    "Synthpop",
    "Abstract",
    "Art Rock",
    "Baroque",
    "Bhangra",
    "Big Beat",
    "Breakbeat",
    "Chillout",
    "Downtempo",
    "Dub",
    "EBM",
    "Eclectic",
    "Electro",
    "Electroclash",
    "Emo",
    "Experimental",
    "Garage",
    "Global",
    "IDM",
    "Illbient",
    "Industro-Goth",
    "Jam Band",
    "Krautrock",
    "Leftfield",
    "Lounge",
    "Math Rock",
    "New Romantic",
    "Nu-Breakz",
    "Post-Punk",
    "Post-Rock",
    "Psytrance",
    "Shoegaze",
    "Space Rock",
    "Trop Rock",
    "World Music",
    "Neoclassical",
    "Audiobook",
    "Audio Theatre",
    "Neue Deutsche Welle",
    "Podcast",
    "Indie Rock",
    "G-Funk",
    "Dubstep",
    "Garage Rock",
    "Psybient",
];

/// Decode text in an ID3 encoding (0 Latin-1, 1 UTF-16 with BOM, 2 UTF-16BE, 3 UTF-8), up to the first terminator.
/// Returns the text and the bytes consumed including the terminator.
fn id3_text(enc: u8, b: &[u8]) -> (String, usize) {
    match enc {
        1 | 2 => {
            // UTF-16: units of two bytes, terminated by a zero unit.
            let end = b.chunks_exact(2).position(|u| u == [0, 0]).map_or(b.len() & !1, |i| i * 2);
            let used = (end + 2).min(b.len());
            let mut body = &b[..end];
            let mut be = enc == 2;
            if enc == 1 && body.len() >= 2 {
                match (body[0], body[1]) {
                    (0xFF, 0xFE) => body = &body[2..],
                    (0xFE, 0xFF) => {
                        be = true;
                        body = &body[2..];
                    }
                    _ => {}
                }
            }
            let units = body.chunks_exact(2).map(|u| {
                if be { u16::from_be_bytes([u[0], u[1]]) } else { u16::from_le_bytes([u[0], u[1]]) }
            });
            (char::decode_utf16(units).map(|r| r.unwrap_or('\u{fffd}')).collect(), used)
        }
        _ => {
            let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            let used = (end + 1).min(b.len());
            let s = if enc == 3 {
                String::from_utf8_lossy(&b[..end]).into_owned()
            } else {
                b[..end].iter().map(|&c| c as char).collect()
            };
            (s, used)
        }
    }
}

/// The first value of a text frame body (`enc` byte then one or more terminated strings).
fn text_frame(body: &[u8]) -> Option<String> {
    let (&enc, rest) = body.split_first()?;
    let (s, _) = id3_text(enc, rest);
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
    (!s.is_empty()).then_some(s)
}

/// The number at the start of a gain or peak value: `"-6.54 dB"`, `"+0.20"`, `"0.988"`, with a comma or a typographic minus
/// tolerated (taggers write all of these).
fn leading_number(v: &str) -> Option<f32> {
    let v = v.trim().replace('\u{2212}', "-").replace(',', ".");
    let end = v
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(v.len(), |(i, _)| i);
    v[..end].parse::<f32>().ok().filter(|x| x.is_finite())
}

/// Take a loudness or gapless tag (`REPLAYGAIN_TRACK_GAIN`, `R128_TRACK_GAIN`, `ITUNPGAP`, ...) into `meta`. `key` is
/// matched without regard to case; the first value seen for a field wins (a file may carry both formats: the one met first
/// is the one its tagger meant as primary). Returns true if the key was one of these.
///
/// ReplayGain 2 gains are decibels to add to reach -18 LUFS, so the loudness is -18 minus the gain; Opus `R128_*_GAIN` values
/// are a signed integer in 1/256 dB to add (on top of the header's output gain, which the decoder applies) to reach -23 LUFS.
/// Peaks are linear.
pub(crate) fn apply_loudness_tag(meta: &mut Metadata, key: &str, value: &str) -> bool {
    use rvp_core::media::{R128_REFERENCE_LUFS, REPLAYGAIN_REFERENCE_LUFS};
    let rg = |g: f32| REPLAYGAIN_REFERENCE_LUFS - g;
    let r128 = |q: i32| R128_REFERENCE_LUFS - q as f32 / 256.0;
    let key = key.trim().to_ascii_uppercase();
    match key.as_str() {
        "REPLAYGAIN_TRACK_GAIN" => {
            meta.loudness.track_lufs = meta.loudness.track_lufs.or(leading_number(value).map(rg))
        }
        "REPLAYGAIN_ALBUM_GAIN" => {
            meta.loudness.album_lufs = meta.loudness.album_lufs.or(leading_number(value).map(rg))
        }
        "REPLAYGAIN_TRACK_PEAK" => {
            meta.loudness.track_peak = meta.loudness.track_peak.or(leading_number(value).filter(|p| *p > 0.0))
        }
        "REPLAYGAIN_ALBUM_PEAK" => {
            meta.loudness.album_peak = meta.loudness.album_peak.or(leading_number(value).filter(|p| *p > 0.0))
        }
        "R128_TRACK_GAIN" => {
            meta.loudness.track_lufs = meta.loudness.track_lufs.or(value.trim().parse::<i32>().ok().map(r128))
        }
        "R128_ALBUM_GAIN" => {
            meta.loudness.album_lufs = meta.loudness.album_lufs.or(value.trim().parse::<i32>().ok().map(r128))
        }
        // iTunes' "part of a gapless album" flag, under the names taggers give it.
        "ITUNPGAP" | "PGAP" | "GAPLESS_ALBUM" | "GAPLESS" | "GAPLESS_PLAYBACK" => {
            if matches!(value.trim(), "1" | "true" | "TRUE" | "True" | "yes") {
                meta.gapless_album = true;
            }
        }
        _ => return false,
    }
    true
}

/// `"3/12"` or `"3"` as (number, total).
fn number_pair(s: &str) -> (Option<u32>, Option<u32>) {
    let mut it = s.split('/');
    let n = it.next().and_then(|v| v.trim().parse().ok());
    let t = it.next().and_then(|v| v.trim().parse().ok());
    (n, t)
}

/// The year in a date such as `"2004"`, `"2004-05-06"` or `"2004-05-06T10:00"`.
pub(crate) fn year_of(s: &str) -> Option<i32> {
    let d: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    (d.len() == 4).then(|| d.parse().ok()).flatten()
}

/// A genre as ID3v2 writes it: `"(13)Pop"`, `"(13)"`, `"13"` or free text.
fn genre_text(s: &str) -> Option<String> {
    let mut t = s.trim();
    if let Some(rest) = t.strip_prefix('(') {
        if let Some((num, tail)) = rest.split_once(')') {
            if let Ok(i) = num.parse::<usize>() {
                if tail.trim().is_empty() {
                    return GENRES.get(i).map(|g| (*g).to_string());
                }
                t = tail.trim();
            }
        }
    }
    if let Ok(i) = t.parse::<usize>() {
        return GENRES.get(i).map(|g| (*g).to_string());
    }
    (!t.is_empty()).then(|| t.to_string())
}

fn syncsafe(b: &[u8]) -> u32 {
    b.iter().take(4).fold(0u32, |a, &x| (a << 7) | (x & 0x7F) as u32)
}

/// Total length of the ID3v2 tag that starts with this 10-byte header (header, body, footer), if it is one.
pub(crate) fn id3v2_len(h: &[u8]) -> Option<u64> {
    if h.len() < 10
        || &h[..3] != b"ID3"
        || h[3] == 0xFF
        || h[4] == 0xFF
        || h[6..10].iter().any(|&b| b & 0x80 != 0)
    {
        return None;
    }
    let footer = if h[3] >= 4 && h[5] & 0x10 != 0 { 10 } else { 0 };
    Some(10 + syncsafe(&h[6..10]) as u64 + footer)
}

/// Undo unsynchronisation (`FF 00` becomes `FF`).
fn deunsync(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        out.push(b[i]);
        if b[i] == 0xFF && b.get(i + 1) == Some(&0) {
            i += 1;
        }
        i += 1;
    }
    out
}

/// Parse an ID3v2 tag (the whole of it, header included) into `meta`; fields the file already has are kept.
pub(crate) fn parse_id3v2(tag: &[u8], meta: &mut Metadata) {
    if tag.len() < 10 {
        return;
    }
    let (major, flags) = (tag[3], tag[5]);
    let mut body_end = tag.len();
    if major >= 4 && flags & 0x10 != 0 {
        body_end = body_end.saturating_sub(10);
    }
    let mut body: Vec<u8> = tag[10..body_end.max(10)].to_vec();
    // Tag-level unsynchronisation (v2.2, v2.3; v2.4 marks it per frame).
    if flags & 0x80 != 0 && major < 4 {
        body = deunsync(&body);
    }
    let mut pos = 0usize;
    // An extended header precedes the frames in v2.3 and v2.4.
    if flags & 0x40 != 0 && major >= 3 {
        let skip = if major == 4 {
            syncsafe(body.get(0..4).unwrap_or(&[0; 4])) as usize
        } else {
            4 + u32::from_be_bytes(body.get(0..4).and_then(|s| s.try_into().ok()).unwrap_or([0; 4])) as usize
        };
        pos = skip.min(body.len());
    }
    let (idlen, hdrlen) = if major == 2 { (3, 6) } else { (4, 10) };
    let mut best_art: Option<(u8, Art)> = None;
    while pos + hdrlen <= body.len() {
        let id = &body[pos..pos + idlen];
        if id[0] == 0 {
            break; // padding
        }
        let size = match major {
            2 => u32::from_be_bytes([0, body[pos + 3], body[pos + 4], body[pos + 5]]) as usize,
            4 => syncsafe(&body[pos + 4..pos + 8]) as usize,
            _ => u32::from_be_bytes([body[pos + 4], body[pos + 5], body[pos + 6], body[pos + 7]]) as usize,
        };
        let fflags = if major >= 3 { u16::from_be_bytes([body[pos + 8], body[pos + 9]]) } else { 0 };
        let start = pos + hdrlen;
        let Some(end) = start.checked_add(size).filter(|&e| e <= body.len()) else { break };
        pos = end;
        let mut data: Vec<u8> = body[start..end].to_vec();
        if major == 4 {
            // v2.4 frame flags: 0x01 data length indicator, 0x02 unsynchronisation, 0x08 compression, 0x04 encryption.
            if fflags & 0x0C != 0 {
                continue;
            }
            if fflags & 0x02 != 0 {
                data = deunsync(&data);
            }
            if fflags & 0x01 != 0 && data.len() >= 4 {
                data.drain(..4);
            }
        } else if major == 3 && fflags & 0x00E0 != 0 {
            continue; // compressed, encrypted or grouped
        }
        let id = core::str::from_utf8(id).unwrap_or("");
        match id {
            "TIT2" | "TT2" => meta.title = meta.title.take().or_else(|| text_frame(&data)),
            "TPE1" | "TP1" => meta.artist = meta.artist.take().or_else(|| text_frame(&data)),
            "TALB" | "TAL" => meta.album = meta.album.take().or_else(|| text_frame(&data)),
            "TPE2" | "TP2" => meta.album_artist = meta.album_artist.take().or_else(|| text_frame(&data)),
            "TRCK" | "TRK" => {
                if let Some((n, t)) = text_frame(&data).map(|s| number_pair(&s)) {
                    meta.track = meta.track.or(n);
                    meta.track_total = meta.track_total.or(t);
                }
            }
            "TPOS" | "TPA" => {
                if let Some((n, t)) = text_frame(&data).map(|s| number_pair(&s)) {
                    meta.disc = meta.disc.or(n);
                    meta.disc_total = meta.disc_total.or(t);
                }
            }
            "TDRC" | "TYER" | "TYE" | "TDRL" => {
                meta.year = meta.year.or_else(|| text_frame(&data).and_then(|s| year_of(&s)))
            }
            "TCON" | "TCO" => {
                meta.genre = meta.genre.take().or_else(|| text_frame(&data).and_then(|s| genre_text(&s)))
            }
            // User-defined text (`TXXX`: encoding, description, value) and comments (`COMM`: encoding, language, description,
            // text): where ReplayGain and the gapless-album flag live.
            "TXXX" | "TXX" | "COMM" | "COM" => {
                let Some((&enc, rest)) = data.split_first() else { continue };
                let rest = if id.starts_with("COM") { rest.get(3..).unwrap_or(&[]) } else { rest };
                let (desc, used) = id3_text(enc, rest);
                let (value, _) = id3_text(enc, rest.get(used..).unwrap_or(&[]));
                apply_loudness_tag(meta, &desc, &value);
            }
            "APIC" | "PIC" => {
                let Some((&enc, rest)) = data.split_first() else { continue };
                let (mime, rest) = if id == "PIC" {
                    let Some((fmt, rest)) = rest.split_at_checked(3) else { continue };
                    let m = match fmt {
                        b"PNG" => "image/png",
                        b"JPG" => "image/jpeg",
                        _ => "image/jpeg",
                    };
                    (m.to_string(), rest)
                } else {
                    let end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
                    let m = String::from_utf8_lossy(&rest[..end]).to_ascii_lowercase();
                    (
                        if m.is_empty() || m == "jpg" { "image/jpeg".to_string() } else { m },
                        rest.get(end + 1..).unwrap_or(&[]),
                    )
                };
                let Some((&ptype, rest)) = rest.split_first() else { continue };
                let (_, used) = id3_text(enc, rest);
                let img = rest.get(used..).unwrap_or(&[]);
                if img.is_empty() || img.len() > MAX_ART {
                    continue;
                }
                // Prefer the front cover (type 3); otherwise the first picture.
                let better = match &best_art {
                    None => true,
                    Some((t, _)) => ptype == 3 && *t != 3,
                };
                if better {
                    best_art = Some((ptype, Art { mime, data: img.to_vec() }));
                }
            }
            _ => {}
        }
    }
    if meta.art.is_none() {
        meta.art = best_art.map(|(_, a)| a);
    }
}

/// Parse the 128-byte ID3v1 tag at the end of a file (`b` is exactly those bytes) into `meta`.
pub(crate) fn parse_id3v1(b: &[u8], meta: &mut Metadata) {
    if b.len() < 128 || &b[..3] != b"TAG" {
        return;
    }
    let field = |r: core::ops::Range<usize>| -> Option<String> {
        let s: String = b[r].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
        let s = s.trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    meta.title = meta.title.take().or_else(|| field(3..33));
    meta.artist = meta.artist.take().or_else(|| field(33..63));
    meta.album = meta.album.take().or_else(|| field(63..93));
    meta.year = meta.year.or_else(|| field(93..97).and_then(|s| year_of(&s)));
    // ID3v1.1: a zero byte before the track number.
    if b[125] == 0 && b[126] != 0 {
        meta.track = meta.track.or(Some(b[126] as u32));
    }
    meta.genre = meta.genre.take().or_else(|| GENRES.get(b[127] as usize).map(|g| (*g).to_string()));
}

/// Decode standard base64 (padding optional; whitespace and unknown characters are skipped).
pub(crate) fn base64(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => continue,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
        if out.len() > MAX_ART + 4096 {
            break;
        }
    }
    out
}

/// A FLAC `PICTURE` block body (also what Ogg carries base64-coded in `METADATA_BLOCK_PICTURE`): (picture type, art).
pub(crate) fn parse_flac_picture(b: &[u8]) -> Option<(u32, Art)> {
    let u32at = |o: usize| -> Option<u32> { Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?)) };
    let ptype = u32at(0)?;
    let mlen = u32at(4)? as usize;
    let mime = String::from_utf8_lossy(b.get(8..8 + mlen)?).into_owned();
    let mut o = 8 + mlen;
    let dlen = u32at(o)? as usize;
    o = o.checked_add(4 + dlen)?;
    // width, height, depth, colours (16 bytes), then the data.
    let len = u32at(o + 16)? as usize;
    let data = b.get(o + 20..o + 20 + len)?;
    (!data.is_empty() && data.len() <= MAX_ART).then(|| {
        (
            ptype,
            Art {
                mime: if mime.is_empty() || mime == "-->" { "image/jpeg".to_string() } else { mime },
                data: data.to_vec(),
            },
        )
    })
}

/// Parse a Vorbis comment block (`vendor`, then `KEY=value` entries) into `meta`.
pub(crate) fn parse_vorbis_comments(b: &[u8], meta: &mut Metadata) {
    let u32at =
        |o: usize| -> Option<usize> { Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize) };
    let Some(vlen) = u32at(0) else { return };
    let Some(mut o) = 4usize.checked_add(vlen) else { return };
    let Some(n) = u32at(o) else { return };
    o += 4;
    let mut best_art: Option<(u32, Art)> = None;
    for _ in 0..n.min(10_000) {
        let Some(len) = u32at(o) else { break };
        let Some(e) = o.checked_add(4).and_then(|s| s.checked_add(len)).filter(|&e| e <= b.len()) else {
            break;
        };
        let entry = &b[o + 4..e];
        o = e;
        let Some(eq) = entry.iter().position(|&c| c == b'=') else { continue };
        let key = String::from_utf8_lossy(&entry[..eq]).to_ascii_uppercase();
        let value = &entry[eq + 1..];
        if key == "METADATA_BLOCK_PICTURE" {
            if let Some(pic) =
                core::str::from_utf8(value).ok().map(base64).and_then(|d| parse_flac_picture(&d))
            {
                let better = match &best_art {
                    None => true,
                    Some((t, _)) => pic.0 == 3 && *t != 3,
                };
                if better {
                    best_art = Some(pic);
                }
            }
            continue;
        }
        let text = String::from_utf8_lossy(value).trim().to_string();
        if text.is_empty() {
            continue;
        }
        match key.as_str() {
            "TITLE" => meta.title = meta.title.take().or(Some(text)),
            "ARTIST" => meta.artist = meta.artist.take().or(Some(text)),
            "ALBUM" => meta.album = meta.album.take().or(Some(text)),
            "ALBUMARTIST" | "ALBUM ARTIST" | "ALBUM_ARTIST" => {
                meta.album_artist = meta.album_artist.take().or(Some(text))
            }
            "TRACKNUMBER" => {
                let (n, t) = number_pair(&text);
                meta.track = meta.track.or(n);
                meta.track_total = meta.track_total.or(t);
            }
            "TRACKTOTAL" | "TOTALTRACKS" => meta.track_total = meta.track_total.or(text.parse().ok()),
            "DISCNUMBER" => {
                let (n, t) = number_pair(&text);
                meta.disc = meta.disc.or(n);
                meta.disc_total = meta.disc_total.or(t);
            }
            "DISCTOTAL" | "TOTALDISCS" => meta.disc_total = meta.disc_total.or(text.parse().ok()),
            "DATE" | "YEAR" => meta.year = meta.year.or_else(|| year_of(&text)),
            "GENRE" => meta.genre = meta.genre.take().or(Some(text)),
            other => {
                apply_loudness_tag(meta, other, &text);
            }
        }
    }
    if meta.art.is_none() {
        meta.art = best_art.map(|(_, a)| a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn frame(id: &[u8; 4], body: &[u8], syncsafe_size: bool) -> Vec<u8> {
        let mut v = id.to_vec();
        let n = body.len() as u32;
        if syncsafe_size {
            v.extend_from_slice(&[
                (n >> 21) as u8 & 0x7F,
                (n >> 14) as u8 & 0x7F,
                (n >> 7) as u8 & 0x7F,
                n as u8 & 0x7F,
            ]);
        } else {
            v.extend_from_slice(&n.to_be_bytes());
        }
        v.extend_from_slice(&[0, 0]);
        v.extend_from_slice(body);
        v
    }

    fn tag(major: u8, frames: &[u8]) -> Vec<u8> {
        let n = frames.len() as u32;
        let mut v = b"ID3".to_vec();
        v.extend_from_slice(&[
            major,
            0,
            0,
            (n >> 21) as u8 & 0x7F,
            (n >> 14) as u8 & 0x7F,
            (n >> 7) as u8 & 0x7F,
            n as u8 & 0x7F,
        ]);
        v.extend_from_slice(frames);
        v
    }

    #[test]
    fn id3v24_text_frames_in_every_encoding() {
        let mut f = Vec::new();
        f.extend(frame(b"TIT2", &[&[3u8][..], "Tïtle ✓".as_bytes()].concat(), true));
        // UTF-16 with BOM, UTF-16BE, Latin-1.
        let utf16: Vec<u8> =
            [0xFF, 0xFE].into_iter().chain("Åna".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        f.extend(frame(b"TPE1", &[&[1u8][..], &utf16].concat(), true));
        f.extend(frame(
            b"TALB",
            &[&[2u8][..], &"Álbum".encode_utf16().flat_map(|u| u.to_be_bytes()).collect::<Vec<_>>()].concat(),
            true,
        ));
        f.extend(frame(
            b"TPE2",
            &[1u8, b'x'][..].iter().copied().take(0).chain([0u8, 0xC9, 0x72, 0x69]).collect::<Vec<_>>(),
            true,
        ));
        f.extend(frame(b"TRCK", &[0, b'3', b'/', b'1', b'2'], true));
        f.extend(frame(b"TPOS", &[0, b'1', b'/', b'2'], true));
        f.extend(frame(b"TDRC", &[3, b'2', b'0', b'0', b'4', b'-', b'0', b'5'], true));
        f.extend(frame(b"TCON", &[0, b'(', b'1', b'3', b')'], true));
        let mut m = Metadata::default();
        parse_id3v2(&tag(4, &f), &mut m);
        assert_eq!(m.title.as_deref(), Some("Tïtle ✓"));
        assert_eq!(m.artist.as_deref(), Some("Åna"));
        assert_eq!(m.album.as_deref(), Some("Álbum"));
        assert_eq!(m.album_artist.as_deref(), Some("Éri"));
        assert_eq!(
            (m.track, m.track_total, m.disc, m.disc_total, m.year),
            (Some(3), Some(12), Some(1), Some(2), Some(2004))
        );
        assert_eq!(m.genre.as_deref(), Some("Pop"));
    }

    #[test]
    fn id3v23_picture_prefers_the_front_cover_and_survives_unsync() {
        let apic = |ptype: u8, data: &[u8]| {
            frame(b"APIC", &[&[0u8][..], b"image/png\0", &[ptype], b"d\0", data].concat(), false)
        };
        let mut f = apic(0, b"OTHER");
        f.extend(apic(3, b"FRONT\xFFx"));
        // Tag-level unsynchronisation (v2.3): a zero byte goes after every FF; frame sizes are those of the original data.
        let unsynced: Vec<u8> =
            f.iter().flat_map(|&b| if b == 0xFF { vec![0xFF, 0] } else { vec![b] }).collect();
        let mut t = tag(3, &unsynced);
        t[5] |= 0x80;
        let mut m = Metadata::default();
        parse_id3v2(&t, &mut m);
        let art = m.art.expect("art");
        assert_eq!(art.mime, "image/png");
        assert_eq!(art.data, b"FRONT\xFFx");
    }

    #[test]
    fn id3v22_and_v1() {
        let f22 = [
            b"TT2".to_vec(),
            vec![0, 0, 3, 0, b'H', b'i'],
            b"PIC".to_vec(),
            vec![0, 0, 9, 0, b'J', b'P', b'G', 3, 0, 1, 2, 3],
        ]
        .concat();
        let mut m = Metadata::default();
        parse_id3v2(&tag(2, &f22), &mut m);
        assert_eq!(m.title.as_deref(), Some("Hi"));
        assert_eq!(m.art.map(|a| (a.mime, a.data)), Some(("image/jpeg".into(), vec![1, 2, 3])));
        let mut v1 = vec![0u8; 128];
        v1[..3].copy_from_slice(b"TAG");
        v1[3..8].copy_from_slice(b"Title");
        v1[33..38].copy_from_slice(b"Artst");
        v1[93..97].copy_from_slice(b"1999");
        v1[125] = 0;
        v1[126] = 7;
        v1[127] = 17;
        let mut m = Metadata::default();
        parse_id3v1(&v1, &mut m);
        assert_eq!(
            (m.title.as_deref(), m.artist.as_deref(), m.year, m.track, m.genre.as_deref()),
            (Some("Title"), Some("Artst"), Some(1999), Some(7), Some("Rock"))
        );
    }

    #[test]
    fn vorbis_comments_and_flac_pictures() {
        let mut pic = Vec::new();
        for v in [3u32, 9] {
            pic.extend(v.to_be_bytes());
        }
        pic.extend(b"image/png");
        pic.extend(0u32.to_be_bytes());
        for _ in 0..4 {
            pic.extend(0u32.to_be_bytes());
        }
        pic.extend(4u32.to_be_bytes());
        pic.extend(b"\x89PNG");
        // base64 of `pic`, written by hand with the alphabet below.
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut b64 = String::new();
        for c in pic.chunks(3) {
            let n =
                (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for k in 0..4 {
                if k <= c.len() {
                    b64.push(alphabet[(n >> (18 - 6 * k) & 63) as usize] as char);
                } else {
                    b64.push('=');
                }
            }
        }
        let mut block = Vec::new();
        block.extend(3u32.to_le_bytes());
        block.extend(b"vnd");
        let entries = [
            "title=Song".to_string(),
            "ARTIST=Band".to_string(),
            "TRACKNUMBER=4/9".to_string(),
            "DATE=2010-01-01".to_string(),
            alloc::format!("METADATA_BLOCK_PICTURE={b64}"),
        ];
        block.extend((entries.len() as u32).to_le_bytes());
        for e in &entries {
            block.extend((e.len() as u32).to_le_bytes());
            block.extend(e.as_bytes());
        }
        let mut m = Metadata::default();
        parse_vorbis_comments(&block, &mut m);
        assert_eq!(
            (m.title.as_deref(), m.artist.as_deref(), m.track, m.track_total, m.year),
            (Some("Song"), Some("Band"), Some(4), Some(9), Some(2010))
        );
        assert_eq!(m.art.map(|a| (a.mime, a.data)), Some(("image/png".into(), b"\x89PNG".to_vec())));
        assert_eq!(base64("TWFu"), b"Man");
    }

    fn vorbis_block(entries: &[&str]) -> Vec<u8> {
        let mut block = Vec::new();
        block.extend(3u32.to_le_bytes());
        block.extend(b"vnd");
        block.extend((entries.len() as u32).to_le_bytes());
        for e in entries {
            block.extend((e.len() as u32).to_le_bytes());
            block.extend(e.as_bytes());
        }
        block
    }

    #[test]
    fn gain_values_as_taggers_write_them() {
        assert_eq!(leading_number("-6.54 dB"), Some(-6.54));
        assert_eq!(leading_number("+0.20 dB"), Some(0.2));
        assert_eq!(leading_number("  1,25 DB "), Some(1.25));
        assert_eq!(leading_number("\u{2212}3.5 dB"), Some(-3.5));
        assert_eq!(leading_number("0.988553"), Some(0.988_553));
        assert_eq!(leading_number("n/a"), None);
        assert_eq!(leading_number(""), None);
        assert_eq!(leading_number("-"), None);
    }

    #[test]
    fn replaygain_in_vorbis_comments_flac_and_ogg() {
        let block = vorbis_block(&[
            "TITLE=Song",
            "replaygain_track_gain=-6.50 dB",
            "REPLAYGAIN_TRACK_PEAK=0.988553",
            "REPLAYGAIN_ALBUM_GAIN=+2.25 dB",
            "REPLAYGAIN_ALBUM_PEAK=1.000000",
            "ITUNPGAP=1",
        ]);
        let mut m = Metadata::default();
        parse_vorbis_comments(&block, &mut m);
        // Gains are decibels to reach -18 LUFS: -6.5 dB to turn down means the track is at -11.5 LUFS.
        assert_eq!(m.loudness.track_lufs, Some(-11.5));
        assert_eq!(m.loudness.album_lufs, Some(-20.25));
        assert_eq!((m.loudness.track_peak, m.loudness.album_peak), (Some(0.988_553), Some(1.0)));
        assert!(m.gapless_album);
        assert_eq!(m.title.as_deref(), Some("Song"));
    }

    #[test]
    fn opus_r128_gains_are_q7_8_against_minus_23() {
        // -512 is -2 dB to add: the track is at -21 LUFS; +256 is one dB to add: the album is at -24.
        let block = vorbis_block(&["R128_TRACK_GAIN=-512", "R128_ALBUM_GAIN=256"]);
        let mut m = Metadata::default();
        parse_vorbis_comments(&block, &mut m);
        assert_eq!((m.loudness.track_lufs, m.loudness.album_lufs), (Some(-21.0), Some(-24.0)));
        assert!(!m.gapless_album);
        // Nothing to go on, or nonsense: no loudness.
        let mut m = Metadata::default();
        parse_vorbis_comments(
            &vorbis_block(&["R128_TRACK_GAIN=loud", "REPLAYGAIN_TRACK_GAIN=x", "REPLAYGAIN_TRACK_PEAK=-1"]),
            &mut m,
        );
        assert!(m.loudness.is_empty());
        // The first value of a field wins.
        let mut m = Metadata::default();
        parse_vorbis_comments(
            &vorbis_block(&["REPLAYGAIN_TRACK_GAIN=-1 dB", "REPLAYGAIN_TRACK_GAIN=-9 dB"]),
            &mut m,
        );
        assert_eq!(m.loudness.track_lufs, Some(-17.0));
    }

    #[test]
    fn replaygain_and_gapless_flag_in_id3_txxx_and_comm() {
        let txxx = |enc: u8, desc: &str, value: &str| -> Vec<u8> {
            let mut b = vec![enc];
            match enc {
                1 => {
                    for s in [desc, value] {
                        b.extend([0xFF, 0xFE]);
                        b.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
                        b.extend([0, 0]);
                    }
                }
                _ => {
                    b.extend(desc.as_bytes());
                    b.push(0);
                    b.extend(value.as_bytes());
                }
            }
            b
        };
        // v2.3 with UTF-16 descriptions (what ffmpeg and foobar2000 write) and a v2.4 UTF-8 frame, and iTunes' comment frame.
        let mut f = Vec::new();
        f.extend(frame(b"TXXX", &txxx(1, "REPLAYGAIN_TRACK_GAIN", "-7.00 dB"), false));
        f.extend(frame(b"TXXX", &txxx(0, "replaygain_track_peak", "0.9"), false));
        f.extend(frame(b"TXXX", &txxx(1, "REPLAYGAIN_ALBUM_GAIN", "-3.00 dB"), false));
        let mut comm = vec![0u8];
        comm.extend(b"eng");
        comm.extend(b"iTunPGAP\0");
        comm.extend(b"1");
        f.extend(frame(b"COMM", &comm, false));
        let mut m = Metadata::default();
        parse_id3v2(&tag(3, &f), &mut m);
        assert_eq!(m.loudness.track_lufs, Some(-11.0));
        assert_eq!(m.loudness.album_lufs, Some(-15.0));
        assert_eq!(m.loudness.track_peak, Some(0.9));
        assert!(m.gapless_album);

        let mut f = Vec::new();
        f.extend(frame(b"TXXX", &txxx(3, "REPLAYGAIN_TRACK_GAIN", "+1.50 dB"), true));
        f.extend(frame(b"TXXX", &txxx(3, "iTunPGAP", "0"), true));
        let mut m = Metadata::default();
        parse_id3v2(&tag(4, &f), &mut m);
        assert_eq!(m.loudness.track_lufs, Some(-19.5));
        assert!(!m.gapless_album, "0 is not the flag");
        // A frame cut short does not take the parser down.
        let mut m = Metadata::default();
        parse_id3v2(&tag(4, &frame(b"TXXX", &[3, b'R'], true)), &mut m);
        assert!(m.loudness.is_empty());
    }
}
