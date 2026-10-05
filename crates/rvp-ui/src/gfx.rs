//! Software drawing into an RGBA8 framebuffer: anti-aliased rounded rectangles, gradients, glows, text masks
//! and a bilinear picture scaler. Everything is deterministic integer/`f32` math (no `std`), so the same
//! pixels come out in a browser and in Rusty Bucket.
use alloc::vec::Vec;
use libm::{cosf, expf, floorf, sinf, sqrtf};
use theme::{GradientStop, LinearGradient, RadialGradient, Rgba};

/// A rectangle in physical pixels, `f32` so layouts can place edges between pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RectF {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl RectF {
    /// A rectangle from its origin and size.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge.
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Horizontal centre.
    pub fn cx(&self) -> f32 {
        self.x + self.w * 0.5
    }

    /// Vertical centre.
    pub fn cy(&self) -> f32 {
        self.y + self.h * 0.5
    }

    /// True if the point lies inside.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Grow (or shrink, when negative) on every side.
    pub fn inflate(&self, d: f32) -> Self {
        Self::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }

    /// Scale about the centre.
    pub fn scaled(&self, k: f32) -> Self {
        Self::new(self.cx() - self.w * k * 0.5, self.cy() - self.h * k * 0.5, self.w * k, self.h * k)
    }
}

/// `c` with its alpha multiplied by `k` (0.0..=1.0).
pub fn fade(c: Rgba, k: f32) -> Rgba {
    Rgba::new(c.r, c.g, c.b, (c.a as f32 * k.clamp(0.0, 1.0) + 0.5) as u8)
}

/// Linear interpolation between two colours (straight alpha).
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t + 0.5) as u8;
    Rgba::new(l(a.r, b.r), l(a.g, b.g), l(a.b, b.b), l(a.a, b.a))
}

/// How a shape is filled.
#[derive(Debug, Clone, Copy)]
pub enum Paint {
    /// One colour.
    Solid(Rgba),
    /// Top to bottom.
    Vertical(Rgba, Rgba),
    /// Left to right.
    Horizontal(Rgba, Rgba),
    /// A design-system gradient (CSS angle semantics) stretched over the shape's box.
    Gradient(&'static LinearGradient),
    /// A gradient over the same box with its alpha scaled (used to dim a brand gradient).
    GradientFaded(&'static LinearGradient, f32),
}

fn stops_at(stops: &[GradientStop], t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let mut prev = stops[0];
    if t <= prev.pos {
        return prev.color;
    }
    for &s in &stops[1..] {
        if t <= s.pos {
            let span = (s.pos - prev.pos).max(1e-6);
            return mix(prev.color, s.color, (t - prev.pos) / span);
        }
        prev = s;
    }
    prev.color
}

/// A [`Paint`] resolved against a box, so evaluating a pixel needs no trigonometry.
struct Painter {
    paint: Paint,
    bx: RectF,
    dx: f32,
    dy: f32,
    inv_len: f32,
}

impl Painter {
    fn new(paint: Paint, bx: RectF) -> Self {
        let (mut dx, mut dy, mut inv_len) = (0.0, 1.0, 1.0 / bx.h.max(1.0));
        if let Paint::Gradient(g) | Paint::GradientFaded(g, _) = paint {
            let a = g.angle_deg * core::f32::consts::PI / 180.0;
            dx = sinf(a);
            dy = -cosf(a);
            let len = (bx.w * dx).abs() + (bx.h * dy).abs();
            inv_len = 1.0 / len.max(1.0);
        } else if let Paint::Horizontal(..) = paint {
            dx = 1.0;
            dy = 0.0;
            inv_len = 1.0 / bx.w.max(1.0);
        }
        Self { paint, bx, dx, dy, inv_len }
    }

    fn at(&self, px: f32, py: f32) -> Rgba {
        match self.paint {
            Paint::Solid(c) => c,
            Paint::Vertical(a, b) => mix(a, b, (py - self.bx.y) * self.inv_len),
            Paint::Horizontal(a, b) => mix(a, b, (px - self.bx.x) * self.inv_len),
            Paint::Gradient(g) => stops_at(g.stops, self.t(px, py)),
            Paint::GradientFaded(g, k) => fade(stops_at(g.stops, self.t(px, py)), k),
        }
    }

    fn t(&self, px: f32, py: f32) -> f32 {
        ((px - self.bx.cx()) * self.dx + (py - self.bx.cy()) * self.dy) * self.inv_len + 0.5
    }
}

/// Signed distance from a point to a rounded rectangle (negative inside).
fn sd_rrect(px: f32, py: f32, r: &RectF, radius: f32) -> f32 {
    let hw = r.w * 0.5;
    let hh = r.h * 0.5;
    let rad = radius.min(hw).min(hh).max(0.0);
    let qx = (px - r.cx()).abs() - (hw - rad);
    let qy = (py - r.cy()).abs() - (hh - rad);
    if qx > 0.0 && qy > 0.0 { sqrtf(qx * qx + qy * qy) - rad } else { qx.max(qy) - rad }
}

/// An RGBA8 framebuffer.
#[derive(Debug, Clone)]
pub struct FrameBuffer {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA, straight alpha (always opaque once `clear`ed).
    pub pixels: Vec<u8>,
}

impl FrameBuffer {
    /// A buffer filled with the page background (`ink-900`).
    pub fn new(width: u32, height: u32) -> Self {
        let mut fb = Self { width, height, pixels: alloc::vec![0; width as usize * height as usize * 4] };
        fb.clear(theme::tokens::BG_PAGE);
        fb
    }

    /// Fill everything with `c`.
    pub fn clear(&mut self, c: Rgba) {
        let row = self.width as usize * 4;
        if row == 0 || self.pixels.len() < row {
            return;
        }
        for px in self.pixels[..row].chunks_exact_mut(4) {
            px.copy_from_slice(&[c.r, c.g, c.b, c.a]);
        }
        for y in 1..self.height as usize {
            self.pixels.copy_within(0..row, y * row);
        }
    }

    /// Change the size (contents become the page background).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(width as usize * height as usize * 4, 0);
        self.clear(theme::tokens::BG_PAGE);
    }

    /// Fill a rectangle, clipped to the buffer, with `c` written as is (no blending).
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, c: Rgba) {
        let x0 = x.max(0) as u32;
        let y0 = y.max(0) as u32;
        let x1 = (x as i64 + w as i64).clamp(0, self.width as i64) as u32;
        let y1 = (y as i64 + h as i64).clamp(0, self.height as i64) as u32;
        for yy in y0..y1 {
            for xx in x0..x1 {
                let i = (yy as usize * self.width as usize + xx as usize) * 4;
                self.pixels[i..i + 4].copy_from_slice(&[c.r, c.g, c.b, c.a]);
            }
        }
    }

    /// The pixel at (x, y).
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        Rgba::new(self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3])
    }

    /// Source-over blend of `c` at (x, y) with extra coverage `cov` (0.0..=1.0). Out-of-bounds is ignored.
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, c: Rgba, cov: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let a = c.a as f32 * (1.0 / 255.0) * cov;
        if a <= 0.002 {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let d = &mut self.pixels[i..i + 4];
        if a >= 0.998 {
            d[0] = c.r;
            d[1] = c.g;
            d[2] = c.b;
        } else {
            d[0] = (d[0] as f32 + (c.r as f32 - d[0] as f32) * a + 0.5) as u8;
            d[1] = (d[1] as f32 + (c.g as f32 - d[1] as f32) * a + 0.5) as u8;
            d[2] = (d[2] as f32 + (c.b as f32 - d[2] as f32) * a + 0.5) as u8;
        }
        d[3] = 255;
    }

    /// Blend a rectangle with square corners (a scrim, a divider). Left and right edges snap to whole pixels;
    /// top and bottom keep their fractional coverage. Solid and vertical paints take a per-row fast path.
    pub fn fill_rect_paint(&mut self, r: RectF, paint: Paint, opacity: f32) {
        let x0 = (libm::roundf(r.x) as i32).clamp(0, self.width as i32);
        let x1 = (libm::roundf(r.right()) as i32).clamp(0, self.width as i32);
        let y0 = (floorf(r.y) as i32).max(0);
        let y1 = (r.bottom() as i32 + 1).min(self.height as i32);
        if x1 <= x0 {
            return;
        }
        let p = Painter::new(paint, r);
        let stride = self.width as usize * 4;
        for y in y0..y1 {
            let cy = ((y as f32 + 1.0).min(r.bottom()) - (y as f32).max(r.y)).max(0.0);
            if cy <= 0.0 {
                continue;
            }
            match paint {
                Paint::Solid(_) | Paint::Vertical(..) => {
                    let c = p.at(r.x, y as f32 + 0.5);
                    let a = (c.a as f32 * cy * opacity + 0.5) as i32;
                    if a <= 0 {
                        continue;
                    }
                    let a = a + (a >> 7); // 0..=256
                    let (cr, cg, cb) = (c.r as i32, c.g as i32, c.b as i32);
                    let row = &mut self.pixels
                        [y as usize * stride + x0 as usize * 4..y as usize * stride + x1 as usize * 4];
                    for d in row.chunks_exact_mut(4) {
                        d[0] = (d[0] as i32 + (((cr - d[0] as i32) * a) >> 8)) as u8;
                        d[1] = (d[1] as i32 + (((cg - d[1] as i32) * a) >> 8)) as u8;
                        d[2] = (d[2] as i32 + (((cb - d[2] as i32) * a) >> 8)) as u8;
                        d[3] = 255;
                    }
                }
                _ => {
                    for x in x0..x1 {
                        let c = p.at(x as f32 + 0.5, y as f32 + 0.5);
                        self.blend(x, y, c, cy * opacity);
                    }
                }
            }
        }
    }

    /// Fill an anti-aliased rounded rectangle. Only the rim and the corners are evaluated pixel by pixel; the inside, where
    /// coverage is one, is filled a row at a time.
    pub fn fill_rrect(&mut self, r: RectF, radius: f32, paint: Paint, opacity: f32) {
        let x0 = (floorf(r.x) as i32 - 1).max(0);
        let y0 = (floorf(r.y) as i32 - 1).max(0);
        let x1 = (r.right() as i32 + 2).min(self.width as i32);
        let y1 = (r.bottom() as i32 + 2).min(self.height as i32);
        let p = Painter::new(paint, r);
        let rad = radius.min(r.w * 0.5).min(r.h * 0.5).max(0.0);
        // Pixels whose centre is more than a pixel inside the shape are fully covered.
        let (ix0, ix1) = (libm::ceilf(r.x + rad + 1.0) as i32, floorf(r.right() - rad - 1.0) as i32);
        let (iy0, iy1) = (libm::ceilf(r.y + 1.0) as i32, floorf(r.bottom() - 1.0) as i32);
        let flat = matches!(paint, Paint::Solid(_) | Paint::Vertical(..));
        let stride = self.width as usize * 4;
        for y in y0..y1 {
            let inside_rows = y >= iy0 && y < iy1;
            // The corner rows (within `rad` of top or bottom) are not covered across the inner span.
            let straight =
                inside_rows && (y as f32 + 0.5) >= r.y + rad && (y as f32 + 0.5) <= r.bottom() - rad;
            let (sx0, sx1) =
                if straight && flat && ix1 > ix0 { (ix0.max(x0), ix1.min(x1)) } else { (x1, x1) };
            let edge = |me: &mut Self, from: i32, to: i32| {
                for x in from..to {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let cov = 0.5 - sd_rrect(fx, fy, &r, radius);
                    if cov > 0.0 {
                        me.blend(x, y, p.at(fx, fy), cov.min(1.0) * opacity);
                    }
                }
            };
            if sx1 > sx0 {
                edge(self, x0, sx0);
                let c = p.at(r.x, y as f32 + 0.5);
                let a = (c.a as f32 * opacity + 0.5) as i32;
                if a > 0 {
                    let a = a + (a >> 7);
                    let (cr, cg, cb) = (c.r as i32, c.g as i32, c.b as i32);
                    let row = &mut self.pixels
                        [y as usize * stride + sx0 as usize * 4..y as usize * stride + sx1 as usize * 4];
                    for d in row.chunks_exact_mut(4) {
                        d[0] = (d[0] as i32 + (((cr - d[0] as i32) * a) >> 8)) as u8;
                        d[1] = (d[1] as i32 + (((cg - d[1] as i32) * a) >> 8)) as u8;
                        d[2] = (d[2] as i32 + (((cb - d[2] as i32) * a) >> 8)) as u8;
                        d[3] = 255;
                    }
                }
                edge(self, sx1, x1);
            } else {
                edge(self, x0, x1);
            }
        }
    }

    /// Stroke the inside edge of a rounded rectangle. Rows in the straight part of the sides only look at the pixels near the
    /// two edges, so a big rectangle costs its perimeter, not its area.
    pub fn stroke_rrect(&mut self, r: RectF, radius: f32, width: f32, color: Rgba, opacity: f32) {
        let x0 = (floorf(r.x) as i32 - 1).max(0);
        let y0 = (floorf(r.y) as i32 - 1).max(0);
        let x1 = (r.right() as i32 + 2).min(self.width as i32);
        let y1 = (r.bottom() as i32 + 2).min(self.height as i32);
        let rad = radius.min(r.w * 0.5).min(r.h * 0.5).max(0.0);
        let band = libm::ceilf(width + 2.0) as i32;
        for y in y0..y1 {
            let fy = y as f32 + 0.5;
            let margin = rad.max(width + 2.0);
            let straight = fy >= r.y + margin && fy <= r.bottom() - margin && r.w > 2.0 * (band as f32 + 2.0);
            let mut x = x0;
            while x < x1 {
                // In a straight row jump from the left band to the right one.
                if straight && x == x0 + band + 1 {
                    x = (x1 - band - 1).max(x);
                }
                let d = sd_rrect(x as f32 + 0.5, fy, &r, radius);
                let cov = (0.5 - d).clamp(0.0, 1.0) * (d + width + 0.5).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, color, cov * opacity);
                }
                x += 1;
            }
        }
    }

    /// A soft glow around a rounded rectangle, like CSS `box-shadow: 0 0 <blur>px <color>`: half strength at
    /// the edge, fading outwards. Draw it before the shape itself.
    pub fn glow_rrect(&mut self, r: RectF, radius: f32, blur: f32, color: Rgba, opacity: f32) {
        self.shadow_rrect(r, radius, 0.0, blur, color, opacity);
    }

    /// A drop shadow: the shape offset by `dy`, blurred.
    pub fn shadow_rrect(&mut self, r: RectF, radius: f32, dy: f32, blur: f32, color: Rgba, opacity: f32) {
        let sigma = (blur * 0.5).max(0.5);
        let reach = blur * 1.5 + 1.0;
        let sh = RectF::new(r.x, r.y + dy, r.w, r.h);
        let x0 = (floorf(sh.x - reach) as i32).max(0);
        let y0 = (floorf(sh.y - reach) as i32).max(0);
        let x1 = ((sh.right() + reach) as i32 + 1).min(self.width as i32);
        let y1 = ((sh.bottom() + reach) as i32 + 1).min(self.height as i32);
        let k = 1.702 / sigma;
        // The shadow is smooth, so it is evaluated on a coarse lattice and interpolated.
        let step = if blur >= 16.0 {
            4
        } else if blur >= 8.0 {
            2
        } else {
            1
        };
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let nx = ((x1 - x0) / step + 2) as usize;
        let ny = ((y1 - y0) / step + 2) as usize;
        let mut lat: Vec<f32> = Vec::with_capacity(nx * ny);
        for j in 0..ny as i32 {
            let fy = (y0 + j * step) as f32 + 0.5;
            for i in 0..nx as i32 {
                let d = sd_rrect((x0 + i * step) as f32 + 0.5, fy, &sh, radius);
                lat.push(1.0 / (1.0 + expf(d * k)));
            }
        }
        let inv = 1.0 / step as f32;
        for y in y0..y1 {
            let (j, fy) = (((y - y0) / step) as usize, ((y - y0) % step) as f32 * inv);
            for x in x0..x1 {
                let (i, fx) = (((x - x0) / step) as usize, ((x - x0) % step) as f32 * inv);
                let a = lat[j * nx + i] * (1.0 - fx) + lat[j * nx + i + 1] * fx;
                let b = lat[(j + 1) * nx + i] * (1.0 - fx) + lat[(j + 1) * nx + i + 1] * fx;
                let cov = a * (1.0 - fy) + b * fy;
                if cov > 0.004 {
                    self.blend(x, y, color, cov * opacity);
                }
            }
        }
    }

    /// Fill the whole buffer with a CSS-style radial gradient (an ellipse radius given in % of the buffer).
    pub fn fill_radial(&mut self, g: &RadialGradient) {
        let (w, h) = (self.width as f32, self.height as f32);
        let (cx, cy) = (g.cx_pct * 0.01 * w, g.cy_pct * 0.01 * h);
        let (rx, ry) = ((g.rx_pct * 0.01 * w).max(1.0), (g.ry_pct * 0.01 * h).max(1.0));
        for y in 0..self.height {
            let dy = (y as f32 + 0.5 - cy) / ry;
            for x in 0..self.width {
                let dx = (x as f32 + 0.5 - cx) / rx;
                let c = stops_at(g.stops, sqrtf(dx * dx + dy * dy));
                let i = (y as usize * self.width as usize + x as usize) * 4;
                self.pixels[i..i + 4].copy_from_slice(&[c.r, c.g, c.b, 255]);
            }
        }
    }

    /// Blend a coverage mask (`w * h` bytes) tinted with `color` at integer position (x, y).
    pub fn blit_mask(&mut self, x: i32, y: i32, w: u32, h: u32, mask: &[u8], color: Rgba, opacity: f32) {
        for my in 0..h as i32 {
            for mx in 0..w as i32 {
                let m = mask[(my as u32 * w + mx as u32) as usize];
                if m != 0 {
                    self.blend(x + mx, y + my, color, m as f32 * (1.0 / 255.0) * opacity);
                }
            }
        }
    }

    /// Draw a picture (`sw * sh` RGBA, opaque) scaled with bilinear filtering into `dst`.
    ///
    /// Vertical first: each destination row blends two source rows (straight SIMD-friendly byte arithmetic), then
    /// the blended row is resampled to the destination width. Channel arithmetic is 8.8 fixed point with a
    /// truncating shift, exactly the same in the scalar code and in the WebAssembly SIMD128 code.
    pub fn blit_scaled(&mut self, dst: RectF, src: &[u8], sw: u32, sh: u32) {
        if sw == 0 || sh == 0 || dst.w < 1.0 || dst.h < 1.0 || src.len() < sw as usize * sh as usize * 4 {
            return;
        }
        let x0 = dst.x.max(0.0) as i32;
        let y0 = dst.y.max(0.0) as i32;
        let x1 = ((dst.x + dst.w + 0.5) as i32).min(self.width as i32);
        let y1 = ((dst.y + dst.h + 0.5) as i32).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let dw = (x1 - x0) as usize;
        let (sx, sy) = (sw as f32 / dst.w, sh as f32 / dst.h);
        let cols: Vec<Col> = (x0..x1)
            .map(|x| {
                let fx = ((x as f32 + 0.5 - dst.x) * sx - 0.5).clamp(0.0, (sw - 1) as f32);
                let i0 = fx as usize;
                Col::new(i0, (i0 + 1).min(sw as usize - 1), ((fx - i0 as f32) * 256.0) as u32)
            })
            .collect();
        let stride = sw as usize * 4;
        let fb_w = self.width as usize;
        let rows = (y1 - y0) as usize;
        let region = &mut self.pixels[y0 as usize * fb_w * 4..y1 as usize * fb_w * 4];
        // Rows are independent, so the pool takes bands of them (each with its own scratch rows).
        let band_rows = if rvp_core::par::threads() > 1 && rows * dw >= 128 * 1024 {
            rows.div_ceil(rvp_core::par::threads() * 2).max(8)
        } else {
            rows
        };
        rvp_core::par::for_each_chunk_mut(region, band_rows * fb_w * 4, &|b, band| {
            // One pixel of padding: the vector resampler reads the pixel after the last one (its weight is 0).
            let mut vrow: Vec<u8> = alloc::vec![0; stride + 4];
            let mut padded: Vec<u8> = alloc::vec![0; stride + 4];
            for (k, line) in band.chunks_exact_mut(fb_w * 4).enumerate() {
                let y = y0 + (b * band_rows + k) as i32;
                let fy = ((y as f32 + 0.5 - dst.y) * sy - 0.5).clamp(0.0, (sh - 1) as f32);
                let j0 = fy as usize;
                let j1 = (j0 + 1).min(sh as usize - 1);
                let wy = ((fy - j0 as f32) * 256.0) as u32;
                let a = &src[j0 * stride..(j0 + 1) * stride];
                let row: &[u8] = if wy == 0 || j1 == j0 {
                    if (j0 + 1) * stride + 4 <= src.len() {
                        &src[j0 * stride..(j0 + 1) * stride + 4]
                    } else {
                        padded[..stride].copy_from_slice(a);
                        &padded
                    }
                } else {
                    blend_rows(&mut vrow[..stride], a, &src[j1 * stride..(j1 + 1) * stride], wy);
                    &vrow
                };
                resample_row(&mut line[x0 as usize * 4..x0 as usize * 4 + dw * 4], row, &cols);
            }
        });
    }

    /// Like [`FrameBuffer::blit_scaled`], with rounded corners (`radius` = half the size makes a circle): the corners are
    /// blended back over what was there before.
    pub fn blit_scaled_rounded(&mut self, dst: RectF, radius: f32, src: &[u8], sw: u32, sh: u32) {
        let r = radius.min(dst.w * 0.5).min(dst.h * 0.5).max(0.0);
        if r < 0.75 {
            self.blit_scaled(dst, src, sw, sh);
            return;
        }
        let x0 = (floorf(dst.x) as i32).max(0);
        let y0 = (floorf(dst.y) as i32).max(0);
        let x1 = (libm::ceilf(dst.right()) as i32).min(self.width as i32);
        let y1 = (libm::ceilf(dst.bottom()) as i32).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let k = libm::ceilf(r) as i32 + 1;
        // The four corner squares (clipped), with their pixels as they were.
        let squares =
            [(x0, y0), ((x1 - k).max(x0), y0), (x0, (y1 - k).max(y0)), ((x1 - k).max(x0), (y1 - k).max(y0))];
        let mut saved: Vec<(i32, i32, [u8; 3])> = Vec::new();
        for (sx, sy) in squares {
            for y in sy..(sy + k).min(y1) {
                for x in sx..(sx + k).min(x1) {
                    let i = (y as usize * self.width as usize + x as usize) * 4;
                    saved.push((x, y, [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2]]));
                }
            }
        }
        self.blit_scaled(dst, src, sw, sh);
        for (x, y, bg) in saved {
            let cov = (0.5 - sd_rrect(x as f32 + 0.5, y as f32 + 0.5, &dst, r)).clamp(0.0, 1.0);
            if cov < 1.0 {
                let i = (y as usize * self.width as usize + x as usize) * 4;
                for c in 0..3 {
                    self.pixels[i + c] =
                        (bg[c] as f32 + (self.pixels[i + c] as f32 - bg[c] as f32) * cov + 0.5) as u8;
                }
            }
        }
    }

    /// Copy another buffer of the same size over this one.
    pub fn copy_from(&mut self, other: &FrameBuffer) {
        if self.pixels.len() == other.pixels.len() {
            self.pixels.copy_from_slice(&other.pixels);
        }
    }
}

/// One destination column of the picture scaler: the two source columns and the 8.8 weight of the second.
#[derive(Clone, Copy)]
struct Col {
    i0: u32,
    i1: u32,
    w: u32,
    /// `(256 - w) | w << 16`: both weights in one lane for the dot product.
    #[cfg_attr(not(all(target_arch = "wasm32", target_feature = "simd128")), allow(dead_code))]
    wp: u32,
}

impl Col {
    fn new(i0: usize, i1: usize, w: u32) -> Self {
        Self { i0: i0 as u32, i1: i1 as u32, w, wp: (256 - w) | (w << 16) }
    }
}

const RB: u32 = 0x00FF_00FF;
const G: u32 = 0x0000_FF00;

/// Blend packed RGBA pixels `a` and `b` (opaque) with the weight `w` (0..=255) of `b`.
#[inline(always)]
fn lerp_px(a: u32, b: u32, w: u32) -> u32 {
    let rb = (((a & RB) * (256 - w) + (b & RB) * w) >> 8) & RB;
    let g = (((a & G) * (256 - w) + (b & G) * w) >> 8) & G;
    rb | g | 0xFF00_0000
}

/// `out = (a * (256 - w) + b * w) >> 8` per byte, alpha forced to 255 (the picture is opaque).
fn blend_rows(out: &mut [u8], a: &[u8], b: &[u8], w: u32) {
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    {
        use rvp_core::simd::*;
        let (wa, wb) = (u16x8_splat((256 - w) as u16), u16x8_splat(w as u16));
        let n = out.len() & !15;
        for i in (0..n).step_by(16) {
            let (va, vb) = (load(&a[i..]), load(&b[i..]));
            let lo = u16x8_shr(
                i16x8_add(
                    i16x8_mul(u16x8_extend_low_u8x16(va), wa),
                    i16x8_mul(u16x8_extend_low_u8x16(vb), wb),
                ),
                8,
            );
            let hi = u16x8_shr(
                i16x8_add(
                    i16x8_mul(u16x8_extend_high_u8x16(va), wa),
                    i16x8_mul(u16x8_extend_high_u8x16(vb), wb),
                ),
                8,
            );
            store(&mut out[i..], v128_or(u8x16_narrow_i16x8(lo, hi), u32x4_splat(0xFF00_0000)));
        }
        blend_rows_scalar(&mut out[n..], &a[n..], &b[n..], w);
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "simd128")))]
    blend_rows_scalar(out, a, b, w);
}

fn blend_rows_scalar(out: &mut [u8], a: &[u8], b: &[u8], w: u32) {
    for ((o, p), q) in out.chunks_exact_mut(4).zip(a.chunks_exact(4)).zip(b.chunks_exact(4)) {
        let v = lerp_px(
            u32::from_le_bytes([p[0], p[1], p[2], p[3]]),
            u32::from_le_bytes([q[0], q[1], q[2], q[3]]),
            w,
        );
        o.copy_from_slice(&v.to_le_bytes());
    }
}

/// Resample one row of packed RGBA to the destination columns. `line` must hold one pixel more than the source
/// width (the vector code reads the pair `i0, i0 + 1` with one load).
fn resample_row(out: &mut [u8], line: &[u8], cols: &[Col]) {
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    {
        use rvp_core::simd::*;
        let n = cols.len() & !3;
        // One output pixel: its two source pixels are adjacent, so load them together, interleave the bytes as
        // (a, b) pairs per channel and let a dot product apply (256 - w, w).
        for i in (0..n).step_by(4) {
            let c = &cols[i..i + 4];
            let at = |k: usize| &line[c[k].i0 as usize * 4..];
            let x01 = load8_hi(load8(at(0)), at(1));
            let x23 = load8_hi(load8(at(2)), at(3));
            let sh = |x: v128| i8x16_shuffle::<0, 4, 1, 5, 2, 6, 3, 7, 8, 12, 9, 13, 10, 14, 11, 15>(x, x);
            let (t01, t23) = (sh(x01), sh(x23));
            let wp = |k: usize| i32x4_splat(c[k].wp as i32);
            let r0 = u32x4_shr(i32x4_dot_i16x8(u16x8_extend_low_u8x16(t01), wp(0)), 8);
            let r1 = u32x4_shr(i32x4_dot_i16x8(u16x8_extend_high_u8x16(t01), wp(1)), 8);
            let r2 = u32x4_shr(i32x4_dot_i16x8(u16x8_extend_low_u8x16(t23), wp(2)), 8);
            let r3 = u32x4_shr(i32x4_dot_i16x8(u16x8_extend_high_u8x16(t23), wp(3)), 8);
            let packed = u8x16_narrow_i16x8(i16x8_narrow_i32x4(r0, r1), i16x8_narrow_i32x4(r2, r3));
            store(&mut out[i * 4..], v128_or(packed, u32x4_splat(0xFF00_0000)));
        }
        resample_row_scalar(&mut out[n * 4..], line, &cols[n..]);
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "simd128")))]
    resample_row_scalar(out, line, cols);
}

fn resample_row_scalar(out: &mut [u8], line: &[u8], cols: &[Col]) {
    let px = |i: u32| -> u32 {
        let k = i as usize * 4;
        u32::from_le_bytes([line[k], line[k + 1], line[k + 2], line[k + 3]])
    };
    for (o, c) in out.chunks_exact_mut(4).zip(cols) {
        o.copy_from_slice(&lerp_px(px(c.i0), px(c.i1), c.w).to_le_bytes());
    }
}

/// Run the WebAssembly SIMD128 self-tests of this crate (0 mismatches expected).
pub fn simd_selftest() -> u32 {
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    {
        let mut bad = 0;
        let mut seed = 0x9e37_79b9u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for (sw, dw) in [(8usize, 5usize), (33, 70), (64, 31), (17, 17), (100, 3)] {
            let line: Vec<u8> = (0..sw * 4 + 4).map(|_| rnd() as u8).collect();
            let cols: Vec<Col> = (0..dw)
                .map(|x| {
                    let f = (x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5;
                    let f = f.clamp(0.0, (sw - 1) as f32);
                    let i0 = f as usize;
                    Col::new(i0, (i0 + 1).min(sw - 1), ((f - i0 as f32) * 256.0) as u32)
                })
                .collect();
            let (mut a, mut b) = (alloc::vec![0u8; dw * 4], alloc::vec![0u8; dw * 4]);
            resample_row(&mut a, &line, &cols);
            resample_row_scalar(&mut b, &line, &cols);
            bad += (a != b) as u32;
            let line = &line[..sw * 4];
            let other: Vec<u8> = (0..sw * 4).map(|_| rnd() as u8).collect();
            for w in [1u32, 77, 128, 255] {
                let (mut a, mut b) = (alloc::vec![0u8; sw * 4], alloc::vec![0u8; sw * 4]);
                blend_rows(&mut a, line, &other, w);
                blend_rows_scalar(&mut b, line, &other, w);
                bad += (a != b) as u32;
            }
        }
        bad
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "simd128")))]
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use theme::tokens;

    #[test]
    fn starts_on_ink_and_fill_rect_clips() {
        let mut fb = FrameBuffer::new(8, 8);
        assert_eq!(fb.pixel(3, 3), tokens::INK_900);
        fb.fill_rect(-2, 6, 100, 100, tokens::MAGENTA_500);
        assert_eq!(fb.pixel(0, 7), tokens::MAGENTA_500);
        assert_eq!(fb.pixel(7, 5), tokens::INK_900);
    }

    #[test]
    fn rounded_rect_is_antialiased_and_clipped() {
        let mut fb = FrameBuffer::new(40, 40);
        fb.fill_rrect(RectF::new(4.5, 4.0, 31.5, 32.0), 12.0, Paint::Solid(tokens::CYAN_500), 1.0);
        assert_eq!(fb.pixel(20, 20), tokens::CYAN_500);
        assert_eq!(fb.pixel(4, 4), tokens::INK_900); // the corner is rounded away
        let edge = fb.pixel(4, 20); // half covered
        assert!(edge.b > tokens::INK_900.b && edge != tokens::CYAN_500);
        fb.fill_rrect(RectF::new(-50.0, -50.0, 200.0, 200.0), 4.0, Paint::Solid(tokens::WHITE), 1.0); // no panic
    }

    #[test]
    fn gradient_runs_end_to_end() {
        let mut fb = FrameBuffer::new(100, 10);
        fb.fill_rect_paint(
            RectF::new(0.0, 0.0, 100.0, 10.0),
            Paint::Horizontal(tokens::WHITE, tokens::INK_900),
            1.0,
        );
        assert!(fb.pixel(1, 5).r > 240 && fb.pixel(98, 5).r < 20);
    }

    #[test]
    fn scaler_preserves_flat_colour() {
        let src = alloc::vec![200u8, 100, 50, 255].repeat(16);
        let mut fb = FrameBuffer::new(20, 20);
        fb.blit_scaled(RectF::new(2.0, 2.0, 16.0, 16.0), &src, 4, 4);
        assert_eq!(fb.pixel(10, 10), Rgba::rgb(200, 100, 50));
        assert_eq!(fb.pixel(0, 0), tokens::INK_900);
    }

    /// The pixel-by-pixel rendering the fast paths replace.
    fn brute_fill(fb: &mut FrameBuffer, r: RectF, radius: f32, c: Rgba, opacity: f32) {
        for y in 0..fb.height as i32 {
            for x in 0..fb.width as i32 {
                let cov = 0.5 - sd_rrect(x as f32 + 0.5, y as f32 + 0.5, &r, radius);
                if cov > 0.0 {
                    fb.blend(x, y, c, cov.min(1.0) * opacity);
                }
            }
        }
    }

    fn brute_stroke(fb: &mut FrameBuffer, r: RectF, radius: f32, width: f32, c: Rgba, opacity: f32) {
        for y in 0..fb.height as i32 {
            for x in 0..fb.width as i32 {
                let d = sd_rrect(x as f32 + 0.5, y as f32 + 0.5, &r, radius);
                let cov = (0.5 - d).clamp(0.0, 1.0) * (d + width + 0.5).clamp(0.0, 1.0);
                if cov > 0.0 {
                    fb.blend(x, y, c, cov * opacity);
                }
            }
        }
    }

    fn max_diff(a: &FrameBuffer, b: &FrameBuffer) -> u8 {
        a.pixels.iter().zip(&b.pixels).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
    }

    #[test]
    fn the_fast_rounded_rectangle_paths_match_the_pixel_by_pixel_ones() {
        let c = Rgba::new(255, 43, 214, 255);
        for (i, (x, y, w, h, rad, wd, op)) in [
            (10.3, 7.8, 90.0, 60.5, 14.0, 2.0, 1.0),
            (5.0, 5.0, 150.0, 20.0, 10.0, 1.0, 0.6),
            (20.5, 3.2, 40.0, 100.0, 40.0, 3.0, 0.9),
            (0.0, 0.0, 200.0, 140.0, 0.0, 1.5, 1.0),
            (-12.0, -8.0, 80.0, 50.0, 12.0, 2.0, 1.0),
            (150.0, 100.0, 100.0, 100.0, 9.0, 2.0, 0.5),
        ]
        .into_iter()
        .enumerate()
        {
            let r = RectF::new(x, y, w, h);
            let (mut a, mut b) = (FrameBuffer::new(200, 140), FrameBuffer::new(200, 140));
            a.fill_rrect(r, rad, Paint::Solid(c), op);
            brute_fill(&mut b, r, rad, c, op);
            assert!(max_diff(&a, &b) <= 2, "fill {i}: {}", max_diff(&a, &b));
            let (mut a, mut b) = (FrameBuffer::new(200, 140), FrameBuffer::new(200, 140));
            a.stroke_rrect(r, rad, wd, c, op);
            brute_stroke(&mut b, r, rad, wd, c, op);
            assert_eq!(max_diff(&a, &b), 0, "stroke {i}");
        }
    }

    #[test]
    fn the_lattice_shadow_stays_close_to_the_exact_one() {
        let r = RectF::new(60.0, 50.0, 120.0, 80.0);
        let col = Rgba::new(5, 2, 15, 200);
        let mut a = FrameBuffer::new(240, 200);
        a.shadow_rrect(r, 18.0, 12.0, 32.0, col, 1.0);
        // Exact reference.
        let mut b = FrameBuffer::new(240, 200);
        let sh = RectF::new(r.x, r.y + 12.0, r.w, r.h);
        let k = 1.702 / 16.0;
        for y in 0..200 {
            for x in 0..240 {
                let d = sd_rrect(x as f32 + 0.5, y as f32 + 0.5, &sh, 18.0);
                let cov = 1.0 / (1.0 + expf(d * k));
                if cov > 0.004 {
                    b.blend(x, y, col, cov);
                }
            }
        }
        assert!(max_diff(&a, &b) <= 3, "{}", max_diff(&a, &b));
    }
}
