//! Lucide icons (ISC): SVG path data parsed at runtime into polylines and rasterised to coverage masks that
//! are cached per size. Stroked like Lucide (width 2 on a 24 grid, round caps and joins); solid transport
//! glyphs (play, pause, skip) are also filled.
use crate::gfx::FrameBuffer;
use crate::icon_data as d;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use libm::{atan2f, ceilf, cosf, floorf, sinf, sqrtf};
use theme::Rgba;

/// An icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum Icon {
    Play,
    Pause,
    SkipBack,
    SkipForward,
    Rewind,
    FastForward,
    Volume2,
    Volume1,
    VolumeX,
    Maximize,
    Minimize,
    FolderOpen,
    Subtitles,
    AudioLines,
    Gauge,
    X,
    Check,
    ChevronRight,
    CircleAlert,
    Settings,
    Loader,
    Film,
    List,
    Music,
    Disc3,
    MicVocal,
    ListMusic,
    Search,
    ListPlus,
    Plus,
    Shuffle,
    Repeat,
    Repeat1,
    RepeatOff,
    Library,
    FolderPlus,
    Download,
    Upload,
    Trash2,
    Pencil,
    ArrowUp,
    ArrowDown,
    ChevronLeft,
    Ellipsis,
    Folder,
    Info,
    Sparkles,
    ListEnd,
    RefreshCw,
    TriangleAlert,
    ChevronDown,
    AudioWaveform,
    LayoutGrid,
    House,
}

impl Icon {
    /// The repeat button's icon for a mode (0 off, 1 all, 2 one): each state has its own.
    pub fn for_repeat(mode: u8) -> Icon {
        match mode {
            1 => Icon::Repeat,
            2 => Icon::Repeat1,
            _ => Icon::RepeatOff,
        }
    }

    fn paths(self) -> &'static [&'static str] {
        match self {
            Icon::Play => d::PLAY,
            Icon::Pause => d::PAUSE,
            Icon::SkipBack => d::SKIP_BACK,
            Icon::SkipForward => d::SKIP_FORWARD,
            Icon::Rewind => d::REWIND,
            Icon::FastForward => d::FAST_FORWARD,
            Icon::Volume2 => d::VOLUME_2,
            Icon::Volume1 => d::VOLUME_1,
            Icon::VolumeX => d::VOLUME_X,
            Icon::Maximize => d::MAXIMIZE,
            Icon::Minimize => d::MINIMIZE,
            Icon::FolderOpen => d::FOLDER_OPEN,
            Icon::Subtitles => d::SUBTITLES,
            Icon::List => d::LIST,
            Icon::AudioLines => d::AUDIO_LINES,
            Icon::Gauge => d::GAUGE,
            Icon::X => d::X,
            Icon::Check => d::CHECK,
            Icon::ChevronRight => d::CHEVRON_RIGHT,
            Icon::CircleAlert => d::CIRCLE_ALERT,
            Icon::Settings => d::SETTINGS_2,
            Icon::Loader => d::LOADER_CIRCLE,
            Icon::Film => d::FILM,
            Icon::Music => d::MUSIC,
            Icon::Disc3 => d::DISC_3,
            Icon::MicVocal => d::MIC_VOCAL,
            Icon::ListMusic => d::LIST_MUSIC,
            Icon::Search => d::SEARCH,
            Icon::ListPlus => d::LIST_PLUS,
            Icon::Plus => d::PLUS,
            Icon::Shuffle => d::SHUFFLE,
            Icon::Repeat => d::REPEAT,
            Icon::Repeat1 => d::REPEAT_1,
            Icon::RepeatOff => d::REPEAT_OFF,
            Icon::Library => d::LIBRARY,
            Icon::FolderPlus => d::FOLDER_PLUS,
            Icon::Download => d::DOWNLOAD,
            Icon::Upload => d::UPLOAD,
            Icon::Trash2 => d::TRASH_2,
            Icon::Pencil => d::PENCIL,
            Icon::ArrowUp => d::ARROW_UP,
            Icon::ArrowDown => d::ARROW_DOWN,
            Icon::ChevronLeft => d::CHEVRON_LEFT,
            Icon::Ellipsis => d::ELLIPSIS,
            Icon::Folder => d::FOLDER,
            Icon::Info => d::INFO,
            Icon::Sparkles => d::SPARKLES,
            Icon::ListEnd => d::LIST_END,
            Icon::RefreshCw => d::REFRESH_CW,
            Icon::TriangleAlert => d::TRIANGLE_ALERT,
            Icon::ChevronDown => d::CHEVRON_DOWN,
            Icon::AudioWaveform => d::AUDIO_WAVEFORM,
            Icon::LayoutGrid => d::LAYOUT_GRID,
            Icon::House => d::HOUSE,
        }
    }
}

type Pt = (f32, f32);

/// One flattened subpath.
#[derive(Debug, Clone)]
pub(crate) struct Poly {
    pts: Vec<Pt>,
    closed: bool,
}

struct Scan<'a> {
    s: &'a [u8],
    i: usize,
}

impl Scan<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i] == b' ' || self.s[self.i] == b',' || self.s[self.i] == b'\n')
        {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.s.get(self.i).copied()
    }

    fn number(&mut self) -> f32 {
        self.skip();
        let start = self.i;
        if matches!(self.s.get(self.i), Some(b'-' | b'+')) {
            self.i += 1;
        }
        let mut seen_dot = false;
        while let Some(&c) = self.s.get(self.i) {
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        parse_f32(&self.s[start..self.i])
    }

    fn flag(&mut self) -> bool {
        self.skip();
        let v = self.s.get(self.i) == Some(&b'1');
        self.i += 1;
        v
    }
}

/// A small decimal parser (`core` has no `str::parse::<f32>` problem, but this avoids `&str` round trips).
fn parse_f32(b: &[u8]) -> f32 {
    let (neg, b) = match b.first() {
        Some(b'-') => (true, &b[1..]),
        Some(b'+') => (false, &b[1..]),
        _ => (false, b),
    };
    let (mut int, mut frac, mut scale, mut dot) = (0.0f32, 0.0f32, 1.0f32, false);
    for &c in b {
        if c == b'.' {
            dot = true;
        } else if dot {
            scale *= 0.1;
            frac += (c - b'0') as f32 * scale;
        } else {
            int = int * 10.0 + (c - b'0') as f32;
        }
    }
    let v = int + frac;
    if neg { -v } else { v }
}

fn cubic(out: &mut Vec<Pt>, p0: Pt, p1: Pt, p2: Pt, p3: Pt) {
    const N: usize = 14;
    for i in 1..=N {
        let t = i as f32 / N as f32;
        let u = 1.0 - t;
        let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push((a * p0.0 + b * p1.0 + c * p2.0 + e * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + e * p3.1));
    }
}

fn quad(out: &mut Vec<Pt>, p0: Pt, p1: Pt, p2: Pt) {
    const N: usize = 10;
    for i in 1..=N {
        let t = i as f32 / N as f32;
        let u = 1.0 - t;
        out.push((
            u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
            u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
        ));
    }
}

/// SVG elliptical arc (endpoint form) flattened into `out`.
#[allow(clippy::too_many_arguments)]
fn arc(out: &mut Vec<Pt>, p0: Pt, mut rx: f32, mut ry: f32, rot_deg: f32, large: bool, sweep: bool, p1: Pt) {
    if p0 == p1 {
        return;
    }
    rx = rx.abs();
    ry = ry.abs();
    if rx < 1e-6 || ry < 1e-6 {
        out.push(p1);
        return;
    }
    let phi = rot_deg * core::f32::consts::PI / 180.0;
    let (cp, sp) = (cosf(phi), sinf(phi));
    let (dx, dy) = ((p0.0 - p1.0) * 0.5, (p0.1 - p1.1) * 0.5);
    let (x1p, y1p) = (cp * dx + sp * dy, -sp * dx + cp * dy);
    let lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lam > 1.0 {
        let k = sqrtf(lam);
        rx *= k;
        ry *= k;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut co = if den.abs() < 1e-9 { 0.0 } else { sqrtf((num / den).max(0.0)) };
    if large == sweep {
        co = -co;
    }
    let (cxp, cyp) = (co * rx * y1p / ry, -co * ry * x1p / rx);
    let (cx, cy) = (cp * cxp - sp * cyp + (p0.0 + p1.0) * 0.5, sp * cxp + cp * cyp + (p0.1 + p1.1) * 0.5);
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| atan2f(ux * vy - uy * vx, ux * vx + uy * vy);
    let th1 = ang(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut dth = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
    let tau = 2.0 * core::f32::consts::PI;
    if !sweep && dth > 0.0 {
        dth -= tau;
    } else if sweep && dth < 0.0 {
        dth += tau;
    }
    let steps = (ceilf(dth.abs() / (core::f32::consts::PI / 14.0)) as usize).max(2);
    for i in 1..=steps {
        let th = th1 + dth * i as f32 / steps as f32;
        let (ct, st) = (cosf(th), sinf(th));
        out.push((cp * rx * ct - sp * ry * st + cx, sp * rx * ct + cp * ry * st + cy));
    }
}

/// Parse SVG path data into flattened subpaths.
pub(crate) fn parse_path(d: &str, polys: &mut Vec<Poly>) {
    let mut sc = Scan { s: d.as_bytes(), i: 0 };
    let (mut cur, mut start) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
    let mut cmd = b'M';
    let mut last_c: Option<Pt> = None; // previous cubic control point (for S)
    let mut last_q: Option<Pt> = None; // previous quad control point (for T)
    let mut pts: Vec<Pt> = Vec::new();
    let mut first_after_move = true;
    let finish = |pts: &mut Vec<Pt>, closed: bool, polys: &mut Vec<Poly>| {
        if pts.len() > 1 || (pts.len() == 1 && closed) {
            polys.push(Poly { pts: core::mem::take(pts), closed });
        } else {
            pts.clear();
        }
    };
    while let Some(c) = sc.peek() {
        if c.is_ascii_alphabetic() {
            cmd = c;
            sc.i += 1;
            if cmd == b'z' || cmd == b'Z' {
                finish(&mut pts, true, polys);
                cur = start;
                last_c = None;
                last_q = None;
                first_after_move = true;
                continue;
            }
        } else if cmd == b'M' {
            cmd = b'L';
        } else if cmd == b'm' {
            cmd = b'l';
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { cur } else { (0.0, 0.0) };
        let mut new_c = None;
        let mut new_q = None;
        match cmd.to_ascii_uppercase() {
            b'M' => {
                finish(&mut pts, false, polys);
                let p = (ox + sc.number(), oy + sc.number());
                cur = p;
                start = p;
                pts.push(p);
                first_after_move = false;
            }
            b'L' => {
                let p = (ox + sc.number(), oy + sc.number());
                cur = p;
                pts.push(p);
            }
            b'H' => {
                let x = sc.number() + if rel { cur.0 } else { 0.0 };
                cur.0 = x;
                pts.push(cur);
            }
            b'V' => {
                let y = sc.number() + if rel { cur.1 } else { 0.0 };
                cur.1 = y;
                pts.push(cur);
            }
            b'C' => {
                let p1 = (ox + sc.number(), oy + sc.number());
                let p2 = (ox + sc.number(), oy + sc.number());
                let p3 = (ox + sc.number(), oy + sc.number());
                cubic(&mut pts, cur, p1, p2, p3);
                new_c = Some(p2);
                cur = p3;
            }
            b'S' => {
                let p1 = last_c.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1));
                let p2 = (ox + sc.number(), oy + sc.number());
                let p3 = (ox + sc.number(), oy + sc.number());
                cubic(&mut pts, cur, p1, p2, p3);
                new_c = Some(p2);
                cur = p3;
            }
            b'Q' => {
                let p1 = (ox + sc.number(), oy + sc.number());
                let p2 = (ox + sc.number(), oy + sc.number());
                quad(&mut pts, cur, p1, p2);
                new_q = Some(p1);
                cur = p2;
            }
            b'T' => {
                let p1 = last_q.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1));
                let p2 = (ox + sc.number(), oy + sc.number());
                quad(&mut pts, cur, p1, p2);
                new_q = Some(p1);
                cur = p2;
            }
            b'A' => {
                let (rx, ry, rot) = (sc.number(), sc.number(), sc.number());
                let (large, sweep) = (sc.flag(), sc.flag());
                let p = (ox + sc.number(), oy + sc.number());
                arc(&mut pts, cur, rx, ry, rot, large, sweep, p);
                cur = p;
            }
            _ => break,
        }
        if first_after_move && pts.is_empty() {
            pts.push(cur);
        }
        last_c = new_c;
        last_q = new_q;
    }
    finish(&mut pts, false, polys);
}

fn dist_seg(p: Pt, a: Pt, b: Pt) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let len2 = abx * abx + aby * aby;
    let t = if len2 < 1e-9 { 0.0 } else { (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len2).clamp(0.0, 1.0) };
    let (dx, dy) = (p.0 - (a.0 + abx * t), p.1 - (a.1 + aby * t));
    sqrtf(dx * dx + dy * dy)
}

/// Nonzero winding number of `p` against the closed polylines.
fn winding(polys: &[Poly], p: Pt) -> i32 {
    let mut w = 0;
    for poly in polys {
        let n = poly.pts.len();
        for i in 0..n {
            let (a, b) = (poly.pts[i], poly.pts[(i + 1) % n]);
            if (a.1 <= p.1) != (b.1 <= p.1) {
                let t = (p.1 - a.1) / (b.1 - a.1);
                if a.0 + t * (b.0 - a.0) > p.0 {
                    w += if b.1 > a.1 { 1 } else { -1 };
                }
            }
        }
    }
    w
}

/// Rasterise `polys` (24 x 24 grid) to a `size * size` coverage mask.
pub(crate) fn render_mask(polys: &[Poly], size: u32, filled: bool) -> Vec<u8> {
    let s = size as f32 / 24.0;
    let radius = (1.0 * s).max(0.7);
    let n = size as usize;
    let mut mask = alloc::vec![0.0f32; n * n];
    for poly in polys {
        let m = poly.pts.len();
        let segs = if poly.closed { m } else { m.saturating_sub(1) };
        for i in 0..segs {
            let a = (poly.pts[i].0 * s, poly.pts[i].1 * s);
            let b = (poly.pts[(i + 1) % m].0 * s, poly.pts[(i + 1) % m].1 * s);
            let x0 = (floorf(a.0.min(b.0) - radius - 1.0) as i32).max(0);
            let x1 = (ceilf(a.0.max(b.0) + radius + 1.0) as i32).min(n as i32);
            let y0 = (floorf(a.1.min(b.1) - radius - 1.0) as i32).max(0);
            let y1 = (ceilf(a.1.max(b.1) + radius + 1.0) as i32).min(n as i32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let dist = dist_seg((x as f32 + 0.5, y as f32 + 0.5), a, b);
                    let cov = (radius + 0.5 - dist).clamp(0.0, 1.0);
                    let cell = &mut mask[y as usize * n + x as usize];
                    if cov > *cell {
                        *cell = cov;
                    }
                }
            }
        }
    }
    if filled {
        let scaled: Vec<Poly> = polys
            .iter()
            .map(|p| Poly { pts: p.pts.iter().map(|q| (q.0 * s, q.1 * s)).collect(), closed: true })
            .collect();
        for y in 0..n {
            for x in 0..n {
                let mut hit = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let p = (x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0);
                        if winding(&scaled, p) != 0 {
                            hit += 1;
                        }
                    }
                }
                let cov = hit as f32 / 16.0;
                let cell = &mut mask[y * n + x];
                if cov > *cell {
                    *cell = cov;
                }
            }
        }
    }
    mask.iter().map(|&v| (v * 255.0 + 0.5) as u8).collect()
}

/// Caches rasterised icon masks per `(icon, size, filled)`.
#[derive(Default)]
pub struct IconCache {
    masks: BTreeMap<(Icon, u32, bool), Vec<u8>>,
}

impl IconCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Draw `icon` centred on (`cx`, `cy`), `size` pixels square.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        fb: &mut FrameBuffer,
        icon: Icon,
        cx: f32,
        cy: f32,
        size: f32,
        color: Rgba,
        opacity: f32,
        filled: bool,
    ) {
        let px = (size + 0.5) as u32;
        if px == 0 {
            return;
        }
        let mask = self.masks.entry((icon, px, filled)).or_insert_with(|| {
            let mut polys = Vec::new();
            for p in icon.paths() {
                parse_path(p, &mut polys);
            }
            render_mask(&polys, px, filled)
        });
        let x = (cx - px as f32 * 0.5 + 0.5) as i32;
        let y = (cy - px as f32 * 0.5 + 0.5) as i32;
        fb.blit_mask(x, y, px, px, mask, color, opacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lucide_paths() {
        let mut polys = Vec::new();
        for icon in [Icon::Play, Icon::FolderOpen, Icon::Gauge, Icon::Volume2, Icon::Maximize, Icon::Film] {
            polys.clear();
            for p in icon.paths() {
                parse_path(p, &mut polys);
            }
            assert!(!polys.is_empty(), "{icon:?}");
            for p in &polys {
                for q in &p.pts {
                    assert!(q.0 > -1.0 && q.0 < 25.0 && q.1 > -1.0 && q.1 < 25.0, "{icon:?} {q:?}");
                }
            }
        }
    }

    #[test]
    fn filled_play_has_solid_centre_and_empty_corners() {
        let mut polys = Vec::new();
        for p in Icon::Play.paths() {
            parse_path(p, &mut polys);
        }
        let m = render_mask(&polys, 24, true);
        assert_eq!(m[12 * 24 + 11], 255);
        assert_eq!(m[0], 0);
        assert_eq!(m[23 * 24 + 23], 0);
        // Stroke only: the interior stays empty.
        let outline = render_mask(&polys, 24, false);
        assert_eq!(outline[12 * 24 + 11], 0);
    }

    #[test]
    fn arc_circle_closes() {
        let mut polys = Vec::new();
        parse_path("M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0 -20z", &mut polys);
        let all: Vec<_> = polys.iter().flat_map(|p| p.pts.iter()).collect();
        assert!(all.iter().all(|p| ((p.0 - 12.0).powi(2) + (p.1 - 12.0).powi(2)).sqrt() > 9.9));
    }
}
