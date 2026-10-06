//! MP3: an ID3v2.4 tag in front of the audio. An existing v2.2 or v2.3 tag is read and rewritten as v2.4 (frames this writer does not
//! know are carried over as they are); an ID3v1 tag or anything else at the end of the file is not touched.
use crate::{Change, Cover, Edit, Error, merged_pair, pair_text, parse_pair};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

struct Frame {
    id: [u8; 4],
    /// Format flags in their v2.4 positions.
    flags: u16,
    data: Vec<u8>,
}

fn syncsafe(b: &[u8]) -> usize {
    b.iter().fold(0usize, |a, &x| a << 7 | (x & 0x7F) as usize)
}

fn put_syncsafe(v: usize) -> Result<[u8; 4], Error> {
    if v >= 1 << 28 {
        return Err(Error::Unsupported("the tag would be bigger than ID3 allows"));
    }
    Ok([(v >> 21) as u8 & 0x7F, (v >> 14) as u8 & 0x7F, (v >> 7) as u8 & 0x7F, v as u8 & 0x7F])
}

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

/// The text of a text frame body.
fn text_of(body: &[u8]) -> String {
    let Some((&enc, rest)) = body.split_first() else { return String::new() };
    let s = match enc {
        1 | 2 => {
            let (mut be, mut r) = (enc == 2, rest);
            if enc == 1 && r.len() >= 2 {
                match (r[0], r[1]) {
                    (0xFF, 0xFE) => r = &r[2..],
                    (0xFE, 0xFF) => {
                        be = true;
                        r = &r[2..];
                    }
                    _ => {}
                }
            }
            let units = r.chunks_exact(2).map(|u| {
                if be { u16::from_be_bytes([u[0], u[1]]) } else { u16::from_le_bytes([u[0], u[1]]) }
            });
            char::decode_utf16(units).map(|c| c.unwrap_or('\u{fffd}')).take_while(|&c| c != '\0').collect()
        }
        3 => String::from_utf8_lossy(rest.split(|&c| c == 0).next().unwrap_or(&[])).into_owned(),
        _ => rest.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect(),
    };
    s.trim().to_string()
}

fn text_frame(id: &[u8; 4], s: &str) -> Frame {
    let mut data = Vec::with_capacity(1 + s.len());
    data.push(3); // UTF-8
    data.extend_from_slice(s.as_bytes());
    Frame { id: *id, flags: 0, data }
}

fn picture_frame(c: &Cover) -> Frame {
    let mut data = Vec::with_capacity(c.data.len() + 32);
    data.push(0); // the text below is Latin-1 (and plain ASCII)
    data.extend_from_slice(c.mime.as_bytes());
    data.push(0);
    data.push(3); // front cover
    data.push(0); // no description
    data.extend_from_slice(&c.data);
    Frame { id: *b"APIC", flags: 0, data }
}

/// A v2.2 frame name as its v2.3/v2.4 name (only the ones worth carrying over).
fn upgrade_id(id: &[u8]) -> Option<[u8; 4]> {
    let name: &[u8; 4] = match id {
        b"TT1" => b"TIT1",
        b"TT2" => b"TIT2",
        b"TT3" => b"TIT3",
        b"TP1" => b"TPE1",
        b"TP2" => b"TPE2",
        b"TP3" => b"TPE3",
        b"TAL" => b"TALB",
        b"TRK" => b"TRCK",
        b"TPA" => b"TPOS",
        b"TYE" => b"TYER",
        b"TCO" => b"TCON",
        b"TCM" => b"TCOM",
        b"TCR" => b"TCOP",
        b"TPB" => b"TPUB",
        b"TEN" => b"TENC",
        b"TBP" => b"TBPM",
        b"TKE" => b"TKEY",
        b"TLE" => b"TLEN",
        b"TXX" => b"TXXX",
        b"COM" => b"COMM",
        b"ULT" => b"USLT",
        b"WXX" => b"WXXX",
        b"TCP" => b"TCMP",
        _ => return None,
    };
    Some(*name)
}

/// A v2.2 picture frame as an `APIC` frame.
fn upgrade_pic(data: &[u8]) -> Option<Frame> {
    // encoding, 3-letter image format, picture type, description, picture
    let (&enc, rest) = data.split_first()?;
    let (fmt, rest) = rest.split_at_checked(3)?;
    let mime: &[u8] = match fmt {
        b"PNG" => b"image/png",
        _ => b"image/jpeg",
    };
    let mut out = alloc::vec![enc];
    out.extend_from_slice(mime);
    out.push(0);
    out.extend_from_slice(rest);
    Some(Frame { id: *b"APIC", flags: 0, data: out })
}

/// The tag at the start of the file: its frames (as v2.4 frames) and how many bytes it takes. `None` if there is no tag.
fn read_tag(file: &[u8]) -> Result<Option<(Vec<Frame>, usize)>, Error> {
    if file.len() < 10 || &file[..3] != b"ID3" {
        return Ok(None);
    }
    let (major, flags) = (file[3], file[5]);
    if !(2..=4).contains(&major) || file[6..10].iter().any(|b| b & 0x80 != 0) {
        return Err(Error::Unsupported("an ID3 tag of a version this writer does not know"));
    }
    let size = syncsafe(&file[6..10]);
    let footer = if major == 4 && flags & 0x10 != 0 { 10 } else { 0 };
    let total = 10 + size + footer;
    if file.len() < 10 + size {
        return Err(Error::Corrupt("the ID3 tag is cut short"));
    }
    let mut body: Vec<u8> = file[10..10 + size].to_vec();
    let tag_unsync = flags & 0x80 != 0;
    if tag_unsync && major < 4 {
        body = deunsync(&body);
    }
    let mut pos = 0usize;
    if flags & 0x40 != 0 && major >= 3 {
        // The extended header (its flags say nothing the rewritten tag needs).
        pos = if major == 4 {
            syncsafe(body.get(0..4).ok_or(Error::Corrupt("the extended header is cut short"))?)
        } else {
            4 + u32::from_be_bytes(
                body.get(0..4)
                    .ok_or(Error::Corrupt("the extended header is cut short"))?
                    .try_into()
                    .unwrap_or([0; 4]),
            ) as usize
        };
        pos = pos.min(body.len());
    }
    let (idlen, hdrlen) = if major == 2 { (3, 6) } else { (4, 10) };
    let mut frames = Vec::new();
    while pos + hdrlen <= body.len() {
        let id = &body[pos..pos + idlen];
        if id[0] == 0 {
            break; // padding
        }
        if !id.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return Err(Error::Corrupt("a frame has a name that is not one"));
        }
        let size = match major {
            2 => u32::from_be_bytes([0, body[pos + 3], body[pos + 4], body[pos + 5]]) as usize,
            4 => syncsafe(&body[pos + 4..pos + 8]),
            _ => u32::from_be_bytes([body[pos + 4], body[pos + 5], body[pos + 6], body[pos + 7]]) as usize,
        };
        let raw_flags = if major >= 3 { u16::from_be_bytes([body[pos + 8], body[pos + 9]]) } else { 0 };
        let start = pos + hdrlen;
        let end = start
            .checked_add(size)
            .filter(|&e| e <= body.len())
            .ok_or(Error::Corrupt("a frame is cut short"))?;
        pos = end;
        let data = body[start..end].to_vec();
        match major {
            4 => {
                let mut data = data;
                let mut flags = raw_flags;
                // A whole-tag unsynchronisation flag means every frame is unsynchronised: undo it per frame.
                if tag_unsync {
                    data = deunsync(&data);
                    flags &= !0x02;
                }
                frames.push(Frame { id: id.try_into().unwrap_or(*b"XXXX"), flags, data });
            }
            3 => {
                // Compressed, encrypted or grouped frames cannot be moved to v2.4 (the flags mean other things there): dropped.
                if raw_flags & 0x00E0 != 0 {
                    continue;
                }
                let st = raw_flags >> 8;
                let flags = (st & 0x80) << 7 | (st & 0x40) << 7 | (st & 0x20) << 7;
                // v2.3 status flags 0x80/0x40/0x20 are v2.4's 0x40/0x20/0x10.
                frames.push(Frame { id: id.try_into().unwrap_or(*b"XXXX"), flags: flags & 0x7000, data });
            }
            _ => {
                if id == b"PIC" {
                    frames.extend(upgrade_pic(&data));
                } else if let Some(new_id) = upgrade_id(id) {
                    frames.push(Frame { id: new_id, flags: 0, data });
                }
            }
        }
    }
    Ok(Some((frames, total)))
}

fn current(frames: &[Frame], id: &[u8; 4]) -> Option<String> {
    frames.iter().find(|f| &f.id == id).map(|f| text_of(&f.data)).filter(|s| !s.is_empty())
}

fn replace_text(frames: &mut Vec<Frame>, ids: &[&[u8; 4]], change: &Change<String>) {
    let write = match change {
        Change::Keep => return,
        Change::Clear => None,
        Change::Set(v) if v.is_empty() => None,
        Change::Set(v) => Some(v),
    };
    frames.retain(|f| !ids.contains(&&f.id));
    if let Some(v) = write {
        frames.push(text_frame(ids[0], v));
    }
}

pub(crate) fn edit(file: &[u8], edit: &Edit) -> Result<Vec<u8>, Error> {
    let (mut frames, old_len) = read_tag(file)?.unwrap_or((Vec::new(), 0));
    // The year: v2.3's TYER, TDAT, TIME and friends are not v2.4 frames. What they said becomes a TDRC unless the year is edited.
    let year_now = current(&frames, b"TDRC").or_else(|| current(&frames, b"TYER"));
    frames.retain(|f| !matches!(&f.id, b"TYER" | b"TDAT" | b"TIME" | b"TRDA" | b"TSIZ" | b"TORY"));
    if let (Some(y), true) = (year_now, frames.iter().all(|f| &f.id != b"TDRC")) {
        frames.push(text_frame(b"TDRC", &y));
    }
    replace_text(&mut frames, &[b"TIT2"], &edit.title);
    replace_text(&mut frames, &[b"TPE1"], &edit.artist);
    replace_text(&mut frames, &[b"TALB"], &edit.album);
    replace_text(&mut frames, &[b"TPE2"], &edit.album_artist);
    replace_text(&mut frames, &[b"TCON"], &edit.genre);
    if !edit.year.is_keep() {
        let y = match &edit.year {
            Change::Set(y) if *y > 0 => Change::Set(y.to_string()),
            _ => Change::Clear,
        };
        replace_text(&mut frames, &[b"TDRC"], &y);
    }
    for (id, no, total) in
        [(b"TRCK", &edit.track_no, &edit.track_total), (b"TPOS", &edit.disc_no, &edit.disc_total)]
    {
        if no.is_keep() && total.is_keep() {
            continue;
        }
        let now = current(&frames, id).map(|s| parse_pair(&s)).unwrap_or((0, 0));
        let change = match merged_pair(now, no, total) {
            Some(p) => Change::Set(pair_text(p)),
            None => Change::Clear,
        };
        replace_text(&mut frames, &[id], &change);
    }
    match &edit.cover {
        Change::Keep => {}
        Change::Clear => frames.retain(|f| &f.id != b"APIC"),
        Change::Set(c) => {
            frames.retain(|f| &f.id != b"APIC");
            frames.push(picture_frame(c));
        }
    }
    // Write the v2.4 tag.
    let mut body = Vec::new();
    for f in &frames {
        body.extend_from_slice(&f.id);
        body.extend_from_slice(&put_syncsafe(f.data.len())?);
        body.extend_from_slice(&f.flags.to_be_bytes());
        body.extend_from_slice(&f.data);
    }
    let mut out = Vec::with_capacity(file.len() - old_len + body.len() + 10);
    if !body.is_empty() {
        out.extend_from_slice(b"ID3\x04\x00\x00");
        out.extend_from_slice(&put_syncsafe(body.len())?);
        out.extend_from_slice(&body);
    }
    out.extend_from_slice(&file[old_len..]);
    update_v1(&mut out, edit);
    Ok(out)
}

/// An ID3v1 tag at the very end (128 bytes) is kept in step: a field this edit changes or clears is changed or cleared there too, or
/// players that read it first (and this library, when the v2 tag has nothing) would still show the old value.
fn update_v1(out: &mut [u8], e: &Edit) {
    let n = out.len();
    if n < 128 || &out[n - 128..n - 125] != b"TAG" {
        return;
    }
    let tag = &mut out[n - 128..];
    let text = |slot: &mut [u8], c: &Change<String>| match c {
        Change::Keep => {}
        Change::Clear => slot.fill(0),
        Change::Set(v) => {
            slot.fill(0);
            for (d, ch) in slot.iter_mut().zip(v.chars()) {
                *d = if (ch as u32) < 256 && ch != '\0' { ch as u32 as u8 } else { b'?' };
            }
        }
    };
    text(&mut tag[3..33], &e.title);
    text(&mut tag[33..63], &e.artist);
    text(&mut tag[63..93], &e.album);
    match &e.year {
        Change::Keep => {}
        Change::Set(y) if (1..10_000).contains(y) => {
            tag[93..97].copy_from_slice(alloc::format!("{y:04}").as_bytes())
        }
        _ => tag[93..97].fill(0),
    }
    match &e.track_no {
        Change::Keep => {}
        Change::Set(t) if (1..256).contains(t) => {
            tag[125] = 0;
            tag[126] = *t as u8;
        }
        _ => {
            if tag[125] == 0 {
                tag[126] = 0;
            }
        }
    }
    // The genre is a number from a fixed list: a changed genre is "unknown" here (the v2 tag has the real one).
    if !e.genre.is_keep() {
        tag[127] = 255;
    }
}
