//! Styled subtitles: ASS text (runs with bold, italic, underline, colour and size; alignment and position) and PGS pictures,
//! drawn on the video's picture area. Plain SRT/WebVTT cues keep their pills (`draw_subtitle` in `draw.rs`).
use crate::font::Face;
use crate::gfx::{FrameBuffer, Paint, RectF};
use crate::model::UiModel;
use crate::ui::{Layout, Ui};
use alloc::string::String;
use alloc::vec::Vec;
use libm::{ceilf, floorf};
use rvp_subs::{ImageObject, Rich, Span};
use theme::Rgba;

/// How far italics lean (the tangent of the angle).
const SLANT: f32 = 0.2;

/// `0xRRGGBBAA` as a colour and an opacity.
fn colour(c: u32) -> (Rgba, f32) {
    (Rgba::new((c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8, 255), (c & 255) as f32 / 255.0)
}

/// A word (with the space after it) in one look, as measured.
struct Piece<'a> {
    text: String,
    span: &'a Span,
    px: f32,
    w: f32,
}

/// One wrapped line of pieces.
struct Row<'a> {
    pieces: Vec<Piece<'a>>,
    w: f32,
    h: f32,
}

impl Ui {
    /// Does the model carry cues that need more than the plain pill (styled text or a picture)?
    pub(crate) fn has_styled_cues(model: &UiModel) -> bool {
        model.subtitle_cues.iter().any(|c| c.rich.is_some() || c.image.is_some())
    }

    fn face_for(span: &Span) -> Face {
        if span.bold { Face::SansBold } else { Face::SansMedium }
    }

    /// The area of the picture the subtitles belong to: the letterboxed video, or the whole window without one.
    fn subtitle_frame(&self, l: &Layout, model: &UiModel) -> RectF {
        let (w, h) = model.video_size;
        if model.has_video && w > 0 && h > 0 { self.video_rect(w, h) } else { RectF::new(0.0, 0.0, l.w, l.h) }
    }

    /// Draw the pictures and the styled text of `model.subtitle_cues` (they come in drawing order).
    pub(crate) fn draw_styled_cues(&mut self, fb: &mut FrameBuffer, l: &Layout, model: &UiModel) {
        let frame = self.subtitle_frame(l, model);
        // Cues at the same edge stack: each bottom-aligned one sits above the last, each top-aligned one below it.
        let (mut stacked_bottom, mut stacked_top) = (0.0f32, 0.0f32);
        for cue in &model.subtitle_cues {
            if let Some(img) = &cue.image {
                let (kx, ky) = (frame.w / img.width.max(1) as f32, frame.h / img.height.max(1) as f32);
                for o in &img.objects {
                    let dst = RectF::new(
                        frame.x + o.x as f32 * kx,
                        frame.y + o.y as f32 * ky,
                        o.w as f32 * kx,
                        o.h as f32 * ky,
                    );
                    blit_alpha(fb, dst, o);
                }
            } else if let Some(r) = &cue.rich {
                self.draw_rich(fb, l, frame, r, &mut stacked_bottom, &mut stacked_top);
            }
        }
    }

    fn draw_rich(
        &mut self,
        fb: &mut FrameBuffer,
        l: &Layout,
        frame: RectF,
        r: &Rich,
        stacked_bottom: &mut f32,
        stacked_top: &mut f32,
    ) {
        let s = l.s;
        let (play_w, play_h) = (r.play_res.0.max(1) as f32, r.play_res.1.max(1) as f32);
        let k = frame.w / play_w;
        let (ml, mr) = (r.margin_l.max(0) as f32 * k, r.margin_r.max(0) as f32 * k);
        let mv = r.margin_v.max(0) as f32 * frame.h / play_h;
        let anchor = r.pos_frac();
        let max_w = if anchor.is_some() { frame.w * 0.9 } else { (frame.w - ml - mr).max(frame.w * 0.3) };
        let h_align = (r.align.clamp(1, 9) - 1) % 3; // 0 left, 1 centre, 2 right
        let v_align = (r.align.clamp(1, 9) - 1) / 3; // 0 bottom, 1 middle, 2 top

        // Wrap: words in their runs, packed into rows no wider than the cue may be.
        let mut rows: Vec<Row<'_>> = Vec::new();
        for line in &r.lines {
            let mut row = Row { pieces: Vec::new(), w: 0.0, h: 0.0 };
            for span in line {
                let px = (span.size_permille as f32 * 0.001 * frame.h).clamp(10.0 * s, frame.h * 0.25);
                let face = Self::face_for(span);
                for word in span.text.split_inclusive(' ') {
                    let w = self.fonts.measure(face, px, word, 0.0);
                    if !row.pieces.is_empty()
                        && row.w + w - trailing_space(word, &mut self.fonts, face, px) > max_w
                    {
                        rows.push(core::mem::replace(&mut row, Row { pieces: Vec::new(), w: 0.0, h: 0.0 }));
                    }
                    row.w += w;
                    row.h = row.h.max(px * 1.25);
                    row.pieces.push(Piece { text: String::from(word), span, px, w });
                }
            }
            if row.pieces.is_empty() {
                // An empty line still takes room.
                row.h = 20.0 * s;
            }
            rows.push(row);
        }
        if rows.is_empty() {
            return;
        }
        let block_h: f32 = rows.iter().map(|r| r.h).sum();

        // Where the block goes.
        let (ax, mut top) = match anchor {
            Some((fx, fy)) => {
                let (x, y) = (frame.x + fx * frame.w, frame.y + fy * frame.h);
                (
                    x,
                    match v_align {
                        0 => y - block_h,
                        1 => y - block_h * 0.5,
                        _ => y,
                    },
                )
            }
            None => {
                let x = match h_align {
                    0 => frame.x + ml,
                    1 => frame.x + frame.w * 0.5 + (ml - mr) * 0.5,
                    _ => frame.right() - mr,
                };
                let top = match v_align {
                    0 => {
                        let mut bottom = frame.bottom() - mv;
                        if self.controls_alpha > 0.05 {
                            bottom = bottom.min(l.bar_top + 40.0 * s);
                        }
                        bottom - block_h - *stacked_bottom
                    }
                    1 => frame.y + (frame.h - block_h) * 0.5,
                    _ => frame.y + mv + *stacked_top,
                };
                match v_align {
                    0 => *stacked_bottom += block_h + 2.0 * s,
                    2 => *stacked_top += block_h + 2.0 * s,
                    _ => {}
                }
                (x, top)
            }
        };
        top = top.max(0.0);

        let (outline, _) = colour(r.outline);
        for row in &rows {
            let mut x = match h_align {
                0 => ax,
                1 => ax - row.w * 0.5,
                _ => ax - row.w,
            };
            let baseline = top + row.h * 0.8;
            for p in &row.pieces {
                let (c, a) = colour(p.span.colour);
                let face = Self::face_for(p.span);
                // Outline: the text again around itself, in the outline colour.
                let o = (p.px * 0.055).max(1.0);
                if r.outline & 255 > 0 && a > 0.0 {
                    for (dx, dy) in
                        [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o), (-o, -o), (o, -o), (-o, o), (o, o)]
                    {
                        self.draw_piece(fb, face, p, x + dx, baseline + dy, outline, a * 0.9);
                    }
                }
                self.draw_piece(fb, face, p, x, baseline, c, a);
                let thick = (p.px * 0.06).max(1.0);
                if p.span.underline {
                    let y = baseline + p.px * 0.12;
                    fb.fill_rrect(RectF::new(x, y, p.w, thick), 0.0, Paint::Solid(c), a);
                }
                if p.span.strike {
                    let y = baseline - p.px * 0.3;
                    fb.fill_rrect(RectF::new(x, y, p.w, thick), 0.0, Paint::Solid(c), a);
                }
                x += p.w;
            }
            top += row.h;
        }
    }

    fn draw_piece(
        &mut self,
        fb: &mut FrameBuffer,
        face: Face,
        p: &Piece<'_>,
        x: f32,
        baseline: f32,
        c: Rgba,
        a: f32,
    ) {
        if p.span.italic {
            self.fonts.draw_slanted(fb, face, p.px, x, baseline, &p.text, c, a, SLANT);
        } else {
            self.fonts.draw(fb, face, p.px, x, baseline, &p.text, c, a, 0.0);
        }
    }
}

/// Width of the space a word ends with (it does not count towards the line's width when the line ends there).
fn trailing_space(word: &str, fonts: &mut crate::font::Fonts, face: Face, px: f32) -> f32 {
    if word.ends_with(' ') { fonts.measure(face, px, " ", 0.0) } else { 0.0 }
}

/// Draw an RGBA picture (straight alpha) into `dst` with alpha blending: nearest when it is enlarged, a box average when
/// it is reduced.
fn blit_alpha(fb: &mut FrameBuffer, dst: RectF, o: &ImageObject) {
    if o.w == 0 || o.h == 0 || dst.w < 1.0 || dst.h < 1.0 || o.rgba.len() < o.w as usize * o.h as usize * 4 {
        return;
    }
    let x0 = (floorf(dst.x) as i32).max(0);
    let y0 = (floorf(dst.y) as i32).max(0);
    let x1 = (ceilf(dst.right()) as i32).min(fb.width as i32);
    let y1 = (ceilf(dst.bottom()) as i32).min(fb.height as i32);
    let (sx, sy) = (o.w as f32 / dst.w, o.h as f32 / dst.h);
    for y in y0..y1 {
        let a0 = (((y as f32 - dst.y) * sy).max(0.0) as usize).min(o.h as usize - 1);
        let a1 = (ceilf((y as f32 + 1.0 - dst.y) * sy) as usize).clamp(a0 + 1, o.h as usize);
        for x in x0..x1 {
            let b0 = (((x as f32 - dst.x) * sx).max(0.0) as usize).min(o.w as usize - 1);
            let b1 = (ceilf((x as f32 + 1.0 - dst.x) * sx) as usize).clamp(b0 + 1, o.w as usize);
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for yy in a0..a1 {
                for xx in b0..b1 {
                    let p = &o.rgba[(yy * o.w as usize + xx) * 4..][..4];
                    let pa = p[3] as u32;
                    r += p[0] as u32 * pa;
                    g += p[1] as u32 * pa;
                    b += p[2] as u32 * pa;
                    a += pa;
                    n += 1;
                }
            }
            if a == 0 {
                continue;
            }
            fb.blend(
                x,
                y,
                Rgba::new((r / a) as u8, (g / a) as u8, (b / a) as u8, 255),
                a as f32 / (n as f32 * 255.0),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MediaState;
    use alloc::rc::Rc;
    use rvp_subs::{Cue, Image};

    fn span(text: &str, bold: bool, italic: bool, colour: u32) -> Span {
        Span { text: text.into(), bold, italic, underline: false, strike: false, colour, size_permille: 70 }
    }

    fn rich(align: u8, pos: Option<(i32, i32)>, spans: Vec<Span>) -> Cue {
        let mut c = Cue::new(0, 1, "x".into());
        c.rich = Some(Rich {
            lines: alloc::vec![spans],
            align,
            pos,
            play_res: (320, 240),
            margin_l: 10,
            margin_r: 10,
            margin_v: 10,
            layer: 0,
            outline: 0x0000_00FF,
        });
        c
    }

    fn model(cues: Vec<Cue>) -> UiModel {
        UiModel { state: MediaState::Paused, has_video: false, subtitle_cues: cues, ..Default::default() }
    }

    /// Pixels (of a 1280 x 720 window) that are bright red, as a bounding box.
    fn bbox(fb: &FrameBuffer, pred: impl Fn(&[u8]) -> bool) -> Option<(u32, u32, u32, u32)> {
        let mut b: Option<(u32, u32, u32, u32)> = None;
        for y in 0..fb.height {
            for x in 0..fb.width {
                let i = ((y * fb.width + x) * 4) as usize;
                if pred(&fb.pixels[i..i + 4]) {
                    b = Some(match b {
                        None => (x, y, x, y),
                        Some((a, bb, c, d)) => (a.min(x), bb.min(y), c.max(x), d.max(y)),
                    });
                }
            }
        }
        b
    }

    fn render(m: &UiModel) -> FrameBuffer {
        let mut ui = Ui::default();
        let l = ui.layout(m);
        let mut fb = FrameBuffer::new(1280, 720);
        fb.clear(Rgba::new(0, 0, 0, 255));
        ui.draw_styled_cues(&mut fb, &l, m);
        fb
    }

    #[test]
    fn colour_alignment_and_position_place_the_text() {
        let red = |p: &[u8]| p[0] > 200 && p[1] < 60 && p[2] < 60;
        // Bottom centre, red.
        let m = model(alloc::vec![rich(2, None, alloc::vec![span("Hello there", true, false, 0xFF00_00FF)])]);
        let b = bbox(&render(&m), red).expect("red text");
        let (cx, bottom) = ((b.0 + b.2) / 2, b.3);
        assert!((cx as i32 - 640).abs() < 40, "centred, {cx}");
        assert!(bottom > 500 && bottom < 720, "near the bottom: {bottom}");
        // Top right at a position: the right edge is at 75% of the width, the top at 25% of the height.
        let m = model(alloc::vec![rich(
            9,
            Some((240, 60)),
            alloc::vec![span("Corner", false, false, 0xFF00_00FF)]
        )]);
        let b = bbox(&render(&m), red).expect("red text");
        assert!(b.2 <= 960 + 2 && b.2 > 900, "right edge at 75%: {}", b.2);
        assert!(b.1 >= 180 - 2 && b.1 < 230, "top at 25%: {}", b.1);
        // Left, middle.
        let m = model(alloc::vec![rich(4, None, alloc::vec![span("Left", false, false, 0xFF00_00FF)])]);
        let b = bbox(&render(&m), red).expect("red text");
        assert!(b.0 < 80 && b.1 > 250 && b.3 < 480, "left edge, middle of the height: {b:?}");
    }

    #[test]
    fn italics_lean_and_runs_keep_their_colours() {
        let red = |p: &[u8]| p[0] > 200 && p[1] < 60 && p[2] < 60;
        let plain = bbox(
            &render(&model(alloc::vec![rich(2, None, alloc::vec![span("HIH", false, false, 0xFF00_00FF)])])),
            red,
        )
        .unwrap();
        let slanted = bbox(
            &render(&model(alloc::vec![rich(2, None, alloc::vec![span("HIH", false, true, 0xFF00_00FF)])])),
            red,
        )
        .unwrap();
        assert!(
            slanted.2 - slanted.0 > plain.2 - plain.0,
            "the leaning text is wider: {plain:?} {slanted:?}"
        );
        // Two runs: red then green, side by side.
        let m = model(alloc::vec![rich(
            2,
            None,
            alloc::vec![span("RED ", false, false, 0xFF00_00FF), span("GREEN", false, false, 0x00FF_00FF)]
        )]);
        let fb = render(&m);
        let r = bbox(&fb, |p| p[0] > 200 && p[1] < 60).unwrap();
        let g = bbox(&fb, |p| p[1] > 200 && p[0] < 60).unwrap();
        assert!(r.2 < g.0, "red text is left of green: {r:?} {g:?}");
    }

    #[test]
    fn two_bottom_cues_stack_instead_of_overlapping() {
        let white = |p: &[u8]| p[0] > 200 && p[1] > 200 && p[2] > 200;
        let one = bbox(
            &render(&model(alloc::vec![rich(2, None, alloc::vec![span("One", false, false, 0xFFFF_FFFF)])])),
            white,
        )
        .unwrap();
        let both = bbox(
            &render(&model(alloc::vec![
                rich(2, None, alloc::vec![span("One", false, false, 0xFFFF_FFFF)]),
                rich(2, None, alloc::vec![span("Two", false, false, 0xFFFF_FFFF)])
            ])),
            white,
        )
        .unwrap();
        assert!(both.1 < one.1 - 20, "the second sits above the first: {one:?} {both:?}");
    }

    #[test]
    fn a_picture_cue_is_drawn_where_it_says() {
        // 20x10 half-transparent green at (100, 50) of a 320x240 picture.
        let mut c = Cue::new(0, 1, String::new());
        c.image = Some(Rc::new(Image {
            width: 320,
            height: 240,
            objects: alloc::vec![ImageObject {
                x: 100,
                y: 50,
                w: 20,
                h: 10,
                rgba: [0, 255, 0, 128].repeat(200)
            }],
        }));
        let m = model(alloc::vec![c]);
        let fb = render(&m);
        // The window is 1280 x 720 and there is no video: the picture area is the window, so the factor is 4 by 3.
        let b = bbox(&fb, |p| p[1] > 100).expect("green");
        assert_eq!((b.0, b.1), (400, 150));
        assert_eq!((b.2 + 1, b.3 + 1), (480, 180));
        let i = ((160 * 1280 + 420) * 4) as usize;
        assert!(
            (fb.pixels[i + 1] as i32 - 128).abs() <= 2,
            "half transparent over black: {}",
            fb.pixels[i + 1]
        );
    }
}
