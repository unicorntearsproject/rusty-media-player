//! FLAC: a list of metadata blocks in front of the audio. The comment block is edited, pictures are replaced or dropped, every
//! other block (stream info, seek table, cue sheet, applications, padding) is kept as it is.
use crate::vorbis::{Comments, picture_block};
use crate::{Change, Edit, Error};
use alloc::vec::Vec;

const STREAMINFO: u8 = 0;
const VORBIS_COMMENT: u8 = 4;
const PICTURE: u8 = 6;

fn block(ty: u8, last: bool, body: &[u8]) -> Result<Vec<u8>, Error> {
    if body.len() >= 1 << 24 {
        return Err(Error::Unsupported("a metadata block would be bigger than FLAC allows"));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.push(ty | if last { 0x80 } else { 0 });
    out.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    out.extend_from_slice(body);
    Ok(out)
}

pub(crate) fn edit(file: &[u8], edit: &Edit) -> Result<Vec<u8>, Error> {
    // Some taggers put an ID3v2 tag in front of the stream: it stays where it is.
    let mut start = 0;
    if file.starts_with(b"ID3") && file.len() >= 10 {
        let size = file[6..10].iter().fold(0usize, |a, &b| a << 7 | (b & 0x7F) as usize);
        let footer = if file[5] & 0x10 != 0 { 10 } else { 0 };
        start = 10 + size + footer;
    }
    if file.get(start..start + 4) != Some(b"fLaC") {
        return Err(Error::Corrupt("not a FLAC stream"));
    }
    let mut at = start + 4;
    let mut blocks: Vec<(u8, &[u8])> = Vec::new();
    loop {
        let h = file.get(at..at + 4).ok_or(Error::Corrupt("the metadata is cut short"))?;
        let (last, ty) = (h[0] & 0x80 != 0, h[0] & 0x7F);
        let len = u32::from_be_bytes([0, h[1], h[2], h[3]]) as usize;
        let body = file.get(at + 4..at + 4 + len).ok_or(Error::Corrupt("a metadata block is cut short"))?;
        blocks.push((ty, body));
        at += 4 + len;
        if last {
            break;
        }
    }
    if blocks.first().map(|b| b.0) != Some(STREAMINFO) {
        return Err(Error::Corrupt("the stream info block is not first"));
    }
    // The comments: edit what is there, or start a block.
    let mut comments = match blocks.iter().find(|b| b.0 == VORBIS_COMMENT) {
        Some((_, body)) => Comments::parse(body)?.0,
        None => Comments::empty(),
    };
    comments.apply(edit);
    let drop_pictures = !edit.cover.is_keep();
    if drop_pictures {
        // A cover that sits inside the comments (the Ogg way, which some taggers also use in FLAC) goes too.
        comments.remove_pictures();
    }
    let comment_bytes = comments.to_bytes();
    // Rebuild the blocks: the comment block in the place of the old one (or after the stream info), the new cover after it.
    let mut out_blocks: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut wrote_comments = false;
    for (ty, body) in &blocks {
        match *ty {
            VORBIS_COMMENT => {
                if !wrote_comments {
                    out_blocks.push((VORBIS_COMMENT, comment_bytes.clone()));
                    wrote_comments = true;
                }
            }
            PICTURE if drop_pictures => {}
            t => {
                out_blocks.push((t, body.to_vec()));
                if t == STREAMINFO && !wrote_comments && !blocks.iter().any(|b| b.0 == VORBIS_COMMENT) {
                    out_blocks.push((VORBIS_COMMENT, comment_bytes.clone()));
                    wrote_comments = true;
                }
            }
        }
    }
    if let Change::Set(c) = &edit.cover {
        // Before any padding, so the padding stays last.
        let at = out_blocks.iter().rposition(|b| b.0 != 1).map_or(out_blocks.len(), |i| i + 1);
        out_blocks.insert(at, (PICTURE, picture_block(c)));
    }
    let mut out = Vec::with_capacity(file.len() + comment_bytes.len() + 64);
    out.extend_from_slice(&file[..start + 4]);
    let n = out_blocks.len();
    for (i, (ty, body)) in out_blocks.iter().enumerate() {
        out.extend_from_slice(&block(*ty, i + 1 == n, body)?);
    }
    out.extend_from_slice(&file[at..]);
    Ok(out)
}
