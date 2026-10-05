//! PGS (HDMV Presentation Graphic Stream) bitmap subtitles, as Matroska stores them (`S_HDMV/PGS`): every block
//! is one display set, a run of segments without the file header (`type`, 16-bit size, payload) that ends with an
//! END segment.
//!
//! A display set carries a presentation composition (which objects go where, with optional cropping), windows,
//! palettes (YCbCr plus alpha) and run-length coded objects. [`PgsDecoder`] keeps the state of the epoch (palettes
//! and objects survive between display sets, as a normal-case set may only update the palette or add an object)
//! and turns each set into an [`Update`]: the picture to show, or "clear the screen".
use crate::{Image, ImageObject};
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;

const SEG_PDS: u8 = 0x14;
const SEG_ODS: u8 = 0x15;
const SEG_PCS: u8 = 0x16;
const SEG_WDS: u8 = 0x17;
const SEG_END: u8 = 0x80;

/// Largest object accepted (each side), and the most pixels in one.
const MAX_SIDE: u32 = 4096;
const MAX_PIXELS: u32 = 4 << 20;
/// Most compressed bytes kept for one object, and most objects and palettes kept.
const MAX_RLE: usize = 4 << 20;
const MAX_OBJECTS: usize = 64;

/// What a display set does to the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// Nothing to change (a set that carries no usable composition).
    None,
    /// Remove what is shown.
    Clear,
    /// Show this picture.
    Show(Rc<Image>),
}

#[derive(Clone)]
struct Obj {
    w: u32,
    h: u32,
    /// Run-length coded pixels.
    rle: Vec<u8>,
    complete: bool,
}

#[derive(Clone, Copy)]
struct Comp {
    object: u16,
    x: u32,
    y: u32,
    crop: Option<(u32, u32, u32, u32)>,
}

/// State of one PGS track.
#[derive(Default)]
pub struct PgsDecoder {
    palettes: BTreeMap<u8, Vec<[u8; 4]>>,
    objects: BTreeMap<u16, Obj>,
    width: u32,
    height: u32,
    palette_id: u8,
    comps: Vec<Comp>,
    have_pcs: bool,
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]) as u32)
}

/// BT.709 (HD) or BT.601 limited-range YCbCr to RGB.
fn ycc_to_rgb(y: u8, cr: u8, cb: u8, hd: bool) -> [u8; 3] {
    let y = (y as f32 - 16.0).max(0.0) * 1.164_383;
    let (cr, cb) = (cr as f32 - 128.0, cb as f32 - 128.0);
    let (kr, kb, kg1, kg2) = if hd {
        (1.792_741, 2.112_402, 0.213_249, 0.532_909)
    } else {
        (1.596_027, 2.017_232, 0.391_762, 0.812_968)
    };
    let c = |v: f32| (v + 0.5).clamp(0.0, 255.0) as u8;
    [c(y + kr * cr), c(y - kg1 * cb - kg2 * cr), c(y + kb * cb)]
}

impl PgsDecoder {
    /// A decoder with no state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything (a seek that jumps over an epoch start is repaired by the next one).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Decode one display set (the payload of one Matroska block).
    pub fn decode(&mut self, data: &[u8]) -> Update {
        let mut at = 0usize;
        let mut update = Update::None;
        while at + 3 <= data.len() {
            let ty = data[at];
            let size = u16::from_be_bytes([data[at + 1], data[at + 2]]) as usize;
            let body = &data[at + 3..(at + 3 + size).min(data.len())];
            at += 3 + size;
            match ty {
                SEG_PCS => self.pcs(body),
                SEG_PDS => self.pds(body),
                SEG_ODS => self.ods(body),
                SEG_WDS => {}
                SEG_END => {
                    update = self.compose();
                    break;
                }
                _ => {}
            }
        }
        // A set whose END is missing (cut off) still shows what it had.
        if matches!(update, Update::None) && self.have_pcs && at >= data.len() && !data.is_empty() {
            update = self.compose();
        }
        update
    }

    fn pcs(&mut self, b: &[u8]) {
        let (Some(w), Some(h)) = (be16(b, 0), be16(b, 2)) else { return };
        // b[4] frame rate, b[5..7] composition number, b[7] state, b[8] palette update, b[9] palette id, b[10] count.
        let (Some(&state), Some(&palette_id), Some(&n)) = (b.get(7), b.get(9), b.get(10)) else { return };
        self.width = w;
        self.height = h;
        if state & 0x80 != 0 {
            // Epoch start: nothing from before is kept.
            self.objects.clear();
            self.palettes.clear();
        }
        self.palette_id = palette_id;
        self.comps.clear();
        self.have_pcs = true;
        let mut at = 11;
        for _ in 0..n.min(8) {
            let (Some(object), Some(&_window), Some(&flags), Some(x), Some(y)) =
                (be16(b, at), b.get(at + 2), b.get(at + 3), be16(b, at + 4), be16(b, at + 6))
            else {
                break;
            };
            at += 8;
            let crop = if flags & 0x80 != 0 {
                let (Some(cx), Some(cy), Some(cw), Some(ch)) =
                    (be16(b, at), be16(b, at + 2), be16(b, at + 4), be16(b, at + 6))
                else {
                    break;
                };
                at += 8;
                Some((cx, cy, cw, ch))
            } else {
                None
            };
            self.comps.push(Comp { object: object as u16, x, y, crop });
        }
    }

    fn pds(&mut self, b: &[u8]) {
        let Some(&id) = b.first() else { return };
        if self.palettes.len() >= MAX_OBJECTS && !self.palettes.contains_key(&id) {
            return;
        }
        // An update changes entries of the palette in place.
        let pal = self.palettes.entry(id).or_insert_with(|| alloc::vec![[0, 0, 0, 0]; 256]);
        for e in b.get(2..).unwrap_or(&[]).chunks_exact(5) {
            // index, Y, Cr, Cb, alpha
            pal[e[0] as usize] = [e[1], e[2], e[3], e[4]];
        }
    }

    fn ods(&mut self, b: &[u8]) {
        let (Some(id), Some(&_ver), Some(&seq)) = (be16(b, 0), b.get(2), b.get(3)) else { return };
        let id = id as u16;
        let payload = b.get(4..).unwrap_or(&[]);
        if seq & 0x80 != 0 {
            // First fragment: 24-bit data length (counting the four bytes of size), width, height, then RLE data.
            if payload.len() < 7 {
                return;
            }
            let (Some(w), Some(h)) = (be16(payload, 3), be16(payload, 5)) else { return };
            if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || w * h > MAX_PIXELS {
                self.objects.remove(&id);
                return;
            }
            if self.objects.len() >= MAX_OBJECTS && !self.objects.contains_key(&id) {
                return;
            }
            let rle = payload[7..].to_vec();
            self.objects.insert(id, Obj { w, h, rle, complete: seq & 0x40 != 0 });
        } else if let Some(o) = self.objects.get_mut(&id) {
            if !o.complete && o.rle.len() + payload.len() <= MAX_RLE {
                o.rle.extend_from_slice(payload);
                o.complete = seq & 0x40 != 0;
            }
        }
    }

    fn compose(&mut self) -> Update {
        if !self.have_pcs {
            return Update::None;
        }
        if self.comps.is_empty() {
            return Update::Clear;
        }
        let hd = self.width >= 1280;
        let mut objects = Vec::new();
        for c in &self.comps {
            let (Some(o), Some(pal)) = (self.objects.get(&c.object), self.palettes.get(&self.palette_id))
            else {
                continue;
            };
            if !o.complete && o.rle.is_empty() {
                continue;
            }
            let Some(io) = render(o, pal, c, hd) else { continue };
            objects.push(io);
        }
        if objects.is_empty() {
            return Update::None;
        }
        Update::Show(Rc::new(Image { width: self.width.max(1), height: self.height.max(1), objects }))
    }
}

/// Run-length decode an object to RGBA and crop it.
fn render(o: &Obj, pal: &[[u8; 4]], c: &Comp, hd: bool) -> Option<ImageObject> {
    let (w, h) = (o.w as usize, o.h as usize);
    let mut idx = alloc::vec![0u8; w * h];
    let (mut x, mut y) = (0usize, 0usize);
    let d = &o.rle;
    let mut i = 0usize;
    while i < d.len() && y < h {
        let b = d[i];
        i += 1;
        let (run, col) = if b != 0 {
            (1usize, b)
        } else {
            let Some(&n) = d.get(i) else { break };
            i += 1;
            if n == 0 {
                // End of line.
                x = 0;
                y += 1;
                continue;
            }
            let long = n & 0x40 != 0;
            let colour = n & 0x80 != 0;
            let mut run = (n & 0x3F) as usize;
            if long {
                let Some(&l) = d.get(i) else { break };
                i += 1;
                run = run << 8 | l as usize;
            }
            let col = if colour {
                let Some(&cc) = d.get(i) else { break };
                i += 1;
                cc
            } else {
                0
            };
            (run, col)
        };
        let n = run.min(w - x.min(w));
        if n > 0 {
            idx[y * w + x..y * w + x + n].fill(col);
        }
        // Pixels beyond the width are dropped; the line ends at its end-of-line code.
        x += run;
    }
    // Crop.
    let (cx, cy, cw, ch) = match c.crop {
        Some((cx, cy, cw, ch)) => {
            let cx = cx.min(o.w);
            let cy = cy.min(o.h);
            (cx as usize, cy as usize, (cw.min(o.w - cx)) as usize, (ch.min(o.h - cy)) as usize)
        }
        None => (0, 0, w, h),
    };
    if cw == 0 || ch == 0 {
        return None;
    }
    let mut rgba = Vec::with_capacity(cw * ch * 4);
    for row in 0..ch {
        for col in 0..cw {
            let p = idx[(cy + row) * w + cx + col];
            let [yy, cr, cb, a] = pal[p as usize];
            let [r, g, b] = ycc_to_rgb(yy, cr, cb, hd);
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    Some(ImageObject { x: c.x + cx as u32, y: c.y + cy as u32, w: cw as u32, h: ch as u32, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(ty: u8, body: &[u8]) -> Vec<u8> {
        let mut v = alloc::vec![ty];
        v.extend_from_slice(&(body.len() as u16).to_be_bytes());
        v.extend_from_slice(body);
        v
    }

    /// RLE of a w x h picture: left half index 1, right half index 2, `h` rows.
    fn rle(w: usize, h: usize) -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..h {
            // colour run of w/2 with colour 1, then w/2 with colour 2, then end of line.
            v.extend_from_slice(&[0, 0x80 | (w / 2) as u8, 1, 0, 0x80 | (w / 2) as u8, 2, 0, 0]);
        }
        v
    }

    fn display_set(state: u8, comps: &[(u16, u16, u16)], with_objects: bool, w: usize, h: usize) -> Vec<u8> {
        let mut pcs = Vec::new();
        pcs.extend_from_slice(&1920u16.to_be_bytes());
        pcs.extend_from_slice(&1080u16.to_be_bytes());
        pcs.extend_from_slice(&[0x10, 0, 1, state, 0, 0, comps.len() as u8]);
        for &(id, x, y) in comps {
            pcs.extend_from_slice(&id.to_be_bytes());
            pcs.extend_from_slice(&[0, 0]);
            pcs.extend_from_slice(&x.to_be_bytes());
            pcs.extend_from_slice(&y.to_be_bytes());
        }
        let mut out = seg(SEG_PCS, &pcs);
        if with_objects {
            // Palette 0: 1 = white (Y 235), 2 = red-ish, half transparent.
            let pds = [0, 0, 1, 235, 128, 128, 255, 2, 81, 240, 90, 128];
            out.extend(seg(SEG_PDS, &pds));
            let data = rle(w, h);
            let mut ods = alloc::vec![0, 1, 0, 0xC0];
            let len = data.len() as u32 + 4;
            ods.extend_from_slice(&len.to_be_bytes()[1..]);
            ods.extend_from_slice(&(w as u16).to_be_bytes());
            ods.extend_from_slice(&(h as u16).to_be_bytes());
            ods.extend_from_slice(&data);
            out.extend(seg(SEG_ODS, &ods));
        }
        out.extend(seg(SEG_END, &[]));
        out
    }

    #[test]
    fn decodes_a_display_set() {
        let mut d = PgsDecoder::new();
        let Update::Show(img) = d.decode(&display_set(0x80, &[(1, 100, 900)], true, 8, 3)) else {
            panic!("no picture")
        };
        assert_eq!((img.width, img.height), (1920, 1080));
        let o = &img.objects[0];
        assert_eq!((o.x, o.y, o.w, o.h), (100, 900, 8, 3));
        assert_eq!(o.rgba.len(), 8 * 3 * 4);
        // Left half: white, opaque. Right half: Y 81 Cr 240 Cb 90 = red, alpha 128 at index 2? (entry is [Y,Cr,Cb,a]).
        assert_eq!(&o.rgba[0..4], &[255, 255, 255, 255]);
        let r = &o.rgba[7 * 4..8 * 4];
        assert!(r[0] > 200 && r[1] < 60 && r[2] < 60 && r[3] == 128, "{r:?}");
    }

    #[test]
    fn empty_composition_clears_and_normal_sets_reuse_the_epoch() {
        let mut d = PgsDecoder::new();
        assert!(matches!(d.decode(&display_set(0x80, &[(1, 0, 0)], true, 4, 2)), Update::Show(_)));
        // A normal set that only re-composes the object already known.
        assert!(matches!(d.decode(&display_set(0x00, &[(1, 10, 10)], false, 4, 2)), Update::Show(_)));
        assert_eq!(d.decode(&display_set(0x00, &[], false, 4, 2)), Update::Clear);
        // A new epoch forgets the objects.
        assert_eq!(d.decode(&display_set(0x80, &[(1, 0, 0)], false, 4, 2)), Update::None);
    }

    #[test]
    fn hostile_input_does_not_panic() {
        let mut d = PgsDecoder::new();
        for blob in [
            &[][..],
            &[0x16, 0xff, 0xff],
            &[0x15, 0, 4, 0, 1, 0, 0x80],
            &[0x14, 0, 1, 0],
            &[0x16, 0, 3, 1, 2, 3, 0x80, 0, 0],
        ] {
            let _ = d.decode(blob);
        }
        // An object claiming a huge size is refused.
        let mut ods = alloc::vec![0, 1, 0, 0xC0, 0xff, 0xff, 0xff];
        ods.extend_from_slice(&0xffffu16.to_be_bytes());
        ods.extend_from_slice(&0xffffu16.to_be_bytes());
        let _ = d.decode(&seg(SEG_ODS, &ods));
    }
}
