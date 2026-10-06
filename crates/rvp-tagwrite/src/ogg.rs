//! Ogg Vorbis and Opus: the comment header is one packet near the start. It is replaced, the header packets are laid out in new pages
//! and the pages after them get their sequence numbers and checksums again; the audio pages' contents are not touched. Other logical
//! streams in a chained file are copied as they are.
use crate::vorbis::Comments;
use crate::{Edit, Error};
use alloc::vec::Vec;

struct Page<'a> {
    raw: &'a [u8],
    header_type: u8,
    serial: u32,
    seq: u32,
    /// Segment lengths.
    segs: &'a [u8],
    data: &'a [u8],
}

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (i, e) in t.iter_mut().enumerate() {
        let mut r = (i as u32) << 24;
        for _ in 0..8 {
            r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
        }
        *e = r;
    }
    t
}

fn crc(table: &[u32; 256], data: &[u8]) -> u32 {
    data.iter().fold(0u32, |c, &b| (c << 8) ^ table[((c >> 24) as u8 ^ b) as usize])
}

fn parse_pages(file: &[u8]) -> Result<Vec<Page<'_>>, Error> {
    let mut pages = Vec::new();
    let mut at = 0;
    while at < file.len() {
        let h = file.get(at..at + 27).ok_or(Error::Corrupt("a page is cut short"))?;
        if &h[..4] != b"OggS" || h[4] != 0 {
            return Err(Error::Corrupt("the stream has bytes between pages"));
        }
        let nsegs = h[26] as usize;
        let segs = file.get(at + 27..at + 27 + nsegs).ok_or(Error::Corrupt("a page is cut short"))?;
        let dlen: usize = segs.iter().map(|&s| s as usize).sum();
        let end = at + 27 + nsegs + dlen;
        let data = file.get(at + 27 + nsegs..end).ok_or(Error::Corrupt("a page is cut short"))?;
        pages.push(Page {
            raw: &file[at..end],
            header_type: h[5],
            serial: u32::from_le_bytes(h[14..18].try_into().unwrap_or([0; 4])),
            seq: u32::from_le_bytes(h[18..22].try_into().unwrap_or([0; 4])),
            segs,
            data,
        });
        at = end;
    }
    Ok(pages)
}

fn lacing(len: usize) -> Vec<u8> {
    let mut v = alloc::vec![255u8; len / 255];
    v.push((len % 255) as u8);
    v
}

/// Lay `packets` out in pages of the given stream: the first alone (it begins the stream), the second on a fresh page, and the rest
/// packed after it; a packet goes on to the next page when the 255 segments are used up (and that page says it is a continuation).
fn paginate(packets: &[Vec<u8>], serial: u32, table: &[u32; 256]) -> Vec<Vec<u8>> {
    struct Cur {
        segs: Vec<u8>,
        data: Vec<u8>,
        continued: bool,
        bos: bool,
        completes: bool,
    }
    let fresh = |continued: bool, bos: bool| Cur {
        segs: Vec::new(),
        data: Vec::new(),
        continued,
        bos,
        completes: false,
    };
    let mut pages: Vec<Cur> = alloc::vec![fresh(false, true)];
    for (pi, p) in packets.iter().enumerate() {
        if pi == 1 {
            pages.push(fresh(false, false));
        }
        let lace = lacing(p.len());
        let (mut seg_at, mut byte_at) = (0usize, 0usize);
        while seg_at < lace.len() {
            if pages.last().is_some_and(|c| c.segs.len() == 255) {
                pages.push(fresh(seg_at > 0, false));
            }
            let Some(cur) = pages.last_mut() else { break };
            let take = (255 - cur.segs.len()).min(lace.len() - seg_at);
            let bytes: usize = lace[seg_at..seg_at + take].iter().map(|&s| s as usize).sum();
            cur.segs.extend_from_slice(&lace[seg_at..seg_at + take]);
            cur.data.extend_from_slice(&p[byte_at..byte_at + bytes]);
            seg_at += take;
            byte_at += bytes;
            if seg_at == lace.len() {
                cur.completes = true;
            }
        }
    }
    pages
        .into_iter()
        .enumerate()
        .map(|(seq, c)| {
            let mut v = Vec::with_capacity(27 + c.segs.len() + c.data.len());
            v.extend_from_slice(b"OggS\0");
            v.push(if c.continued { 1 } else { 0 } | if c.bos { 2 } else { 0 });
            // Header pages carry no audio: granule 0, or -1 for a page on which no packet ends.
            let granule: u64 = if c.completes { 0 } else { u64::MAX };
            v.extend_from_slice(&granule.to_le_bytes());
            v.extend_from_slice(&serial.to_le_bytes());
            v.extend_from_slice(&(seq as u32).to_le_bytes());
            v.extend_from_slice(&[0; 4]);
            v.push(c.segs.len() as u8);
            v.extend_from_slice(&c.segs);
            v.extend_from_slice(&c.data);
            let sum = crc(table, &v);
            v[22..26].copy_from_slice(&sum.to_le_bytes());
            v
        })
        .collect()
}

pub(crate) fn edit(file: &[u8], edit: &Edit) -> Result<Vec<u8>, Error> {
    let pages = parse_pages(file)?;
    let first = pages.first().ok_or(Error::Corrupt("no pages"))?;
    if first.header_type & 2 == 0 {
        return Err(Error::Corrupt("the stream does not begin where it should"));
    }
    let serial = first.serial;
    let opus = first.data.starts_with(b"OpusHead");
    if !opus && !first.data.starts_with(b"\x01vorbis") {
        return Err(Error::Unsupported("only Ogg Vorbis and Ogg Opus can be edited"));
    }
    let needed = if opus { 2 } else { 3 };
    // The header packets: gathered from the pages of this stream until the last one ends.
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut header_pages = 0;
    'pages: for (i, p) in pages.iter().enumerate() {
        if p.serial != serial {
            return Err(Error::Unsupported("a stream that begins with several logical streams at once"));
        }
        let mut off = 0;
        for (si, &s) in p.segs.iter().enumerate() {
            cur.extend_from_slice(&p.data[off..off + s as usize]);
            off += s as usize;
            if s < 255 {
                packets.push(core::mem::take(&mut cur));
                if packets.len() == needed {
                    if si + 1 != p.segs.len() {
                        return Err(Error::Unsupported("the audio starts on the page the headers end on"));
                    }
                    header_pages = i + 1;
                    break 'pages;
                }
            }
        }
    }
    if header_pages == 0 {
        return Err(Error::Corrupt("the headers are cut short"));
    }
    // The comment packet.
    let prefix: &[u8] = if opus { b"OpusTags" } else { b"\x03vorbis" };
    let comment = &packets[1];
    if !comment.starts_with(prefix) {
        return Err(Error::Corrupt("the second packet is not the comment header"));
    }
    let body = &comment[prefix.len()..];
    let (mut comments, used) = Comments::parse(body)?;
    // Opus may carry padding after the comments; Vorbis has a framing bit. Neither is meaningful, and neither is kept.
    let _ = used;
    comments.apply(edit);
    if !edit.cover.is_keep() {
        comments.remove_pictures();
    }
    if let crate::Change::Set(c) = &edit.cover {
        comments.add_picture(c);
    }
    let mut new_comment = prefix.to_vec();
    new_comment.extend_from_slice(&comments.to_bytes());
    if !opus {
        new_comment.push(1); // the framing bit
    }
    packets[1] = new_comment;
    let table = crc_table();
    let new_pages = paginate(&packets, serial, &table);
    let delta = new_pages.len() as i64 - header_pages as i64;
    let mut out = Vec::with_capacity(file.len() + 4096);
    for p in &new_pages {
        out.extend_from_slice(p);
    }
    for p in &pages[header_pages..] {
        if p.serial == serial && delta != 0 {
            let mut raw = p.raw.to_vec();
            let seq = (p.seq as i64 + delta) as u32;
            raw[18..22].copy_from_slice(&seq.to_le_bytes());
            raw[22..26].copy_from_slice(&[0; 4]);
            let sum = crc(&table, &raw);
            raw[22..26].copy_from_slice(&sum.to_le_bytes());
            out.extend_from_slice(&raw);
        } else {
            out.extend_from_slice(p.raw);
        }
    }
    Ok(out)
}
