//! The Rusty Wave logo (the official icon art), drawn into the UI: the library rail, the empty screens.
//!
//! `tools/gen-brand.py` writes the art as raw premultiplied RGBA, 64 x 64 and 192 x 192 (the smaller one so the
//! rail mark is not shrunk more than about 2x by the bilinear filter). Both are square with a transparent border.
use crate::gfx::{FrameBuffer, RectF};

static LOGO_64: &[u8] = include_bytes!("../assets/logo-64.rgba");
static LOGO_192: &[u8] = include_bytes!("../assets/logo-192.rgba");

/// Draw the logo into `dst` (a square; the art keeps its transparent margin inside it), faded by `opacity`.
pub(crate) fn draw(fb: &mut FrameBuffer, dst: RectF, opacity: f32) {
    if dst.w <= 80.0 {
        fb.blit_premul(dst, LOGO_64, 64, opacity);
    } else {
        fb.blit_premul(dst, LOGO_192, 192, opacity);
    }
}
