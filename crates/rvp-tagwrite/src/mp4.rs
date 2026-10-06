//! MP4 and M4A: the tags are the `ilst` atom in `moov/udta/meta`. The atom is rebuilt (atoms this writer does not know are carried
//! over as they are) and the boxes around it get their new sizes. When the movie box moves the media data (it grew, or shrank by
//! less than a free box can fill), every chunk offset in `stco` and `co64` is moved with it; when it shrank enough, a `free` box
//! takes the difference, so nothing after it moves at all.
use crate::{Change, Cover, Edit, Error, merged_pair};
use alloc::string::ToString;
use alloc::vec::Vec;

struct Bx<'a> {
    ty: [u8; 4],
    /// The whole box, header included.
    whole: &'a [u8],
    /// Where the body starts in `whole`.
    hdr: usize,
}

impl<'a> Bx<'a> {
    fn body(&self) -> &'a [u8] {
        &self.whole[self.hdr..]
    }
}

fn boxes(data: &[u8]) -> Result<Vec<Bx<'_>>, Error> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < data.len() {
        let h = data.get(at..at + 8).ok_or(Error::Corrupt("a box is cut short"))?;
        let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let ty: [u8; 4] = [h[4], h[5], h[6], h[7]];
        let (size, hdr) = match size32 {
            0 => ((data.len() - at) as u64, 8),
            1 => {
                let l = data.get(at + 8..at + 16).ok_or(Error::Corrupt("a box is cut short"))?;
                (u64::from_be_bytes(l.try_into().unwrap_or([0; 8])), 16)
            }
            n => (n, 8),
        };
        if size < hdr as u64 || at as u64 + size > data.len() as u64 {
            return Err(Error::Corrupt("a box does not fit in the file"));
        }
        let end = at + size as usize;
        out.push(Bx { ty, whole: &data[at..end], hdr });
        at = end;
    }
    Ok(out)
}

fn mk(ty: &[u8; 4], body: &[u8]) -> Result<Vec<u8>, Error> {
    let total = body.len() + 8;
    if total > u32::MAX as usize {
        return Err(Error::Unsupported("a box would be bigger than 4 GB"));
    }
    let mut v = Vec::with_capacity(total);
    v.extend_from_slice(&(total as u32).to_be_bytes());
    v.extend_from_slice(ty);
    v.extend_from_slice(body);
    Ok(v)
}

/// A `data` box: type indicator, no locale, payload.
fn data_box(indicator: u32, payload: &[u8]) -> Result<Vec<u8>, Error> {
    let mut b = indicator.to_be_bytes().to_vec(); // version 0 and the 3-byte type
    b.extend_from_slice(&[0; 4]); // locale
    b.extend_from_slice(payload);
    mk(b"data", &b)
}

fn item(ty: &[u8; 4], indicator: u32, payload: &[u8]) -> Result<Vec<u8>, Error> {
    mk(ty, &data_box(indicator, payload)?)
}

const NAM: [u8; 4] = [0xA9, b'n', b'a', b'm'];
const ART: [u8; 4] = [0xA9, b'A', b'R', b'T'];
const ALB: [u8; 4] = [0xA9, b'a', b'l', b'b'];
const DAY: [u8; 4] = [0xA9, b'd', b'a', b'y'];
const GEN: [u8; 4] = [0xA9, b'g', b'e', b'n'];

/// The payload of an item's first `data` box.
fn payload(item_body: &[u8]) -> Option<&[u8]> {
    let b = boxes(item_body).ok()?;
    let d = b.iter().find(|x| &x.ty == b"data")?;
    d.body().get(8..)
}

fn pair_of(items: &[Bx<'_>], ty: &[u8; 4]) -> (u32, u32) {
    items
        .iter()
        .find(|i| &i.ty == ty)
        .and_then(|i| payload(i.body()))
        .filter(|p| p.len() >= 6)
        .map_or((0, 0), |p| {
            (u16::from_be_bytes([p[2], p[3]]) as u32, u16::from_be_bytes([p[4], p[5]]) as u32)
        })
}

fn text_item(ty: &[u8; 4], v: &str) -> Result<Vec<u8>, Error> {
    item(ty, 1, v.as_bytes())
}

/// The new `ilst` body: the old items minus what is edited, plus the new ones.
fn edit_items(old: &[Bx<'_>], e: &Edit) -> Result<Vec<u8>, Error> {
    let mut drop: Vec<[u8; 4]> = Vec::new();
    let mut add: Vec<Vec<u8>> = Vec::new();
    let mut text =
        |ty: [u8; 4], aliases: &[[u8; 4]], c: &Change<alloc::string::String>| -> Result<(), Error> {
            match c {
                Change::Keep => {}
                Change::Clear => {
                    drop.push(ty);
                    drop.extend_from_slice(aliases);
                }
                Change::Set(v) => {
                    drop.push(ty);
                    drop.extend_from_slice(aliases);
                    if !v.is_empty() {
                        add.push(text_item(&ty, v)?);
                    }
                }
            }
            Ok(())
        };
    text(NAM, &[], &e.title)?;
    text(ART, &[], &e.artist)?;
    text(ALB, &[], &e.album)?;
    text(*b"aART", &[], &e.album_artist)?;
    text(GEN, &[*b"gnre"], &e.genre)?;
    match &e.year {
        Change::Keep => {}
        Change::Clear => drop.push(DAY),
        Change::Set(y) => {
            drop.push(DAY);
            if *y > 0 {
                add.push(text_item(&DAY, &y.to_string())?);
            }
        }
    }
    for (ty, no, total, len) in
        [(*b"trkn", &e.track_no, &e.track_total, 8usize), (*b"disk", &e.disc_no, &e.disc_total, 6)]
    {
        if no.is_keep() && total.is_keep() {
            continue;
        }
        drop.push(ty);
        let now = pair_of(old, &ty);
        if let Some((n, t)) = merged_pair(now, no, total) {
            let mut p = alloc::vec![0u8; len];
            p[2..4].copy_from_slice(&(n.min(0xFFFF) as u16).to_be_bytes());
            p[4..6].copy_from_slice(&(t.min(0xFFFF) as u16).to_be_bytes());
            add.push(item(&ty, 0, &p)?);
        }
    }
    match &e.cover {
        Change::Keep => {}
        Change::Clear => drop.push(*b"covr"),
        Change::Set(Cover { mime, data }) => {
            drop.push(*b"covr");
            add.push(item(b"covr", if mime == "image/png" { 14 } else { 13 }, data)?);
        }
    }
    let mut out = Vec::new();
    for i in old {
        if !drop.contains(&i.ty) {
            out.extend_from_slice(i.whole);
        }
    }
    for a in add {
        out.extend_from_slice(&a);
    }
    Ok(out)
}

fn hdlr() -> Result<Vec<u8>, Error> {
    let mut b = alloc::vec![0u8; 8]; // version and flags, pre_defined
    b.extend_from_slice(b"mdir");
    b.extend_from_slice(b"appl");
    b.extend_from_slice(&[0; 9]); // the rest of the reserved words and an empty name
    mk(b"hdlr", &b)
}

/// The `meta` box with its `ilst` replaced (or made), from the old `meta` body (if there was one).
fn new_meta(old_body: Option<&[u8]>, e: &Edit) -> Result<Vec<u8>, Error> {
    let Some(body) = old_body else {
        let mut b = alloc::vec![0u8; 4];
        b.extend_from_slice(&hdlr()?);
        b.extend_from_slice(&mk(b"ilst", &edit_items(&[], e)?)?);
        return mk(b"meta", &b);
    };
    // A full box (version and flags first), or QuickTime's `meta` without them.
    let prefix =
        if body.len() >= 12 && &body[8..12] == b"hdlr" || !(body.len() >= 8 && &body[4..8] == b"hdlr") {
            4
        } else {
            0
        };
    let kids = boxes(body.get(prefix..).ok_or(Error::Corrupt("the meta box is cut short"))?)?;
    let old_items = match kids.iter().find(|k| &k.ty == b"ilst") {
        Some(k) => boxes(k.body())?,
        None => Vec::new(),
    };
    let ilst = mk(b"ilst", &edit_items(&old_items, e)?)?;
    let mut out = body[..prefix].to_vec();
    let mut placed = false;
    for k in &kids {
        if &k.ty == b"ilst" {
            if !placed {
                out.extend_from_slice(&ilst);
                placed = true;
            }
        } else {
            out.extend_from_slice(k.whole);
        }
    }
    if !placed {
        out.extend_from_slice(&ilst);
    }
    mk(b"meta", &out)
}

/// Move every chunk offset at or after `from` by `delta`, in place, through `moov/trak/mdia/minf/stbl/(stco|co64)`.
fn patch_offsets(
    buf: &mut [u8],
    start: usize,
    end: usize,
    depth: u8,
    from: u64,
    delta: i64,
) -> Result<(), Error> {
    let mut at = start;
    while at + 8 <= end {
        let size = u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]) as usize;
        let ty = [buf[at + 4], buf[at + 5], buf[at + 6], buf[at + 7]];
        if size < 8 || at + size > end {
            return Err(Error::Corrupt("a box in the movie box does not fit"));
        }
        match &ty {
            b"trak" | b"mdia" | b"minf" | b"stbl" if depth < 8 => {
                patch_offsets(buf, at + 8, at + size, depth + 1, from, delta)?
            }
            b"stco" | b"co64" => {
                let wide = &ty == b"co64";
                let w = if wide { 8 } else { 4 };
                let n = u32::from_be_bytes([buf[at + 12], buf[at + 13], buf[at + 14], buf[at + 15]]) as usize;
                if at + 16 + n.saturating_mul(w) > at + size {
                    return Err(Error::Corrupt("a chunk offset table is cut short"));
                }
                for i in 0..n {
                    let p = at + 16 + i * w;
                    let v = if wide {
                        u64::from_be_bytes(buf[p..p + 8].try_into().unwrap_or([0; 8]))
                    } else {
                        u32::from_be_bytes(buf[p..p + 4].try_into().unwrap_or([0; 4])) as u64
                    };
                    if v >= from {
                        let nv = (v as i64 + delta) as u64;
                        if wide {
                            buf[p..p + 8].copy_from_slice(&nv.to_be_bytes());
                        } else if nv > u32::MAX as u64 {
                            return Err(Error::Unsupported("a chunk offset would not fit in 32 bits"));
                        } else {
                            buf[p..p + 4].copy_from_slice(&(nv as u32).to_be_bytes());
                        }
                    }
                }
            }
            _ => {}
        }
        at += size;
    }
    Ok(())
}

pub(crate) fn edit(file: &[u8], e: &Edit) -> Result<Vec<u8>, Error> {
    let top = boxes(file)?;
    if top.first().map(|b| &b.ty) != Some(b"ftyp") {
        return Err(Error::Corrupt("not an MP4 file"));
    }
    let mut moovs = top.iter().enumerate().filter(|(_, b)| &b.ty == b"moov");
    let (mi, moov) = moovs.next().ok_or(Error::Corrupt("no movie box"))?;
    if moovs.next().is_some() {
        return Err(Error::Corrupt("more than one movie box"));
    }
    let moov_start: usize = top[..mi].iter().map(|b| b.whole.len()).sum();
    let kids = boxes(moov.body())?;
    // The new udta (with the edited meta) and the new moov body.
    let (new_udta, udta_at) = match kids.iter().position(|k| &k.ty == b"udta") {
        Some(i) => {
            let ukids = boxes(kids[i].body())?;
            let meta_body = ukids.iter().find(|k| &k.ty == b"meta").map(|k| k.body());
            let meta = new_meta(meta_body, e)?;
            let mut b = Vec::new();
            let mut placed = false;
            for k in &ukids {
                if &k.ty == b"meta" {
                    if !placed {
                        b.extend_from_slice(&meta);
                        placed = true;
                    }
                } else {
                    b.extend_from_slice(k.whole);
                }
            }
            if !placed {
                b.extend_from_slice(&meta);
            }
            (mk(b"udta", &b)?, Some(i))
        }
        None => (mk(b"udta", &new_meta(None, e)?)?, None),
    };
    let mut new_body = Vec::with_capacity(moov.body().len() + new_udta.len());
    for (i, k) in kids.iter().enumerate() {
        if Some(i) == udta_at {
            new_body.extend_from_slice(&new_udta);
        } else {
            new_body.extend_from_slice(k.whole);
        }
    }
    if udta_at.is_none() {
        new_body.extend_from_slice(&new_udta);
    }
    let mut new_moov = mk(b"moov", &new_body)?;
    let old_len = moov.whole.len();
    // A free box takes up what the movie box gave back, so nothing after it moves.
    let mut pad: Vec<u8> = Vec::new();
    let mut delta = new_moov.len() as i64 - old_len as i64;
    if delta < 0 && -delta >= 8 {
        pad = mk(b"free", &alloc::vec![0u8; (-delta) as usize - 8])?;
        delta = 0;
    }
    if delta != 0 && top[mi + 1..].iter().any(|b| &b.ty == b"mdat") {
        if top.iter().any(|b| &b.ty == b"mfra") {
            return Err(Error::Unsupported("a fragmented file with a random access index"));
        }
        let end = new_moov.len();
        patch_offsets(&mut new_moov, 8, end, 0, (moov_start + old_len) as u64, delta)?;
    }
    let mut out = Vec::with_capacity(file.len() + 64);
    out.extend_from_slice(&file[..moov_start]);
    out.extend_from_slice(&new_moov);
    out.extend_from_slice(&pad);
    out.extend_from_slice(&file[moov_start + old_len..]);
    Ok(out)
}
