//! Intra sample prediction (8.4.4.2): reference sample gathering with substitution and smoothing, then planar, DC or angular.
use crate::tables::{INTRA_ANGLE, INV_ANGLE};

/// The neighbouring samples of an `n x n` block: `left[0]` and `top[0]` are the corner `p[-1][-1]`, `left[1 + y]` is `p[-1][y]` and
/// `top[1 + x]` is `p[x][-1]`, for `x, y` up to `2n - 1`.
pub struct Refs {
    pub left: [i32; 129],
    pub top: [i32; 129],
}

impl Refs {
    pub fn new() -> Self {
        Self { left: [0; 129], top: [0; 129] }
    }
}

/// Fill in the samples that are not available (8.4.4.2.2). `avail_left[i]` and `avail_top[i]` say whether `left[i]` and `top[i]` hold a
/// real sample (index 0 of both is the corner, the same sample).
pub fn substitute(r: &mut Refs, avail_left: &[bool], avail_top: &[bool], n: usize, bit_depth: u32) {
    let total = 4 * n + 1;
    // Linear order from the bottom-left sample up to the corner and on to the top-right sample.
    let get = |r: &Refs, k: usize| -> i32 { if k <= 2 * n { r.left[2 * n - k] } else { r.top[k - 2 * n] } };
    let avail = |k: usize| -> bool { if k <= 2 * n { avail_left[2 * n - k] } else { avail_top[k - 2 * n] } };
    let set = |r: &mut Refs, k: usize, v: i32| {
        if k <= 2 * n {
            r.left[2 * n - k] = v;
        } else {
            r.top[k - 2 * n] = v;
        }
    };
    let Some(first) = (0..total).find(|k| avail(*k)) else {
        let v = 1 << (bit_depth - 1);
        for i in 0..=2 * n {
            r.left[i] = v;
            r.top[i] = v;
        }
        return;
    };
    if first > 0 {
        let v = get(r, first);
        set(r, 0, v);
    }
    let mut prev = get(r, 0);
    for k in 1..total {
        if avail(k) {
            prev = get(r, k);
        } else {
            set(r, k, prev);
        }
    }
    r.top[0] = r.left[0];
}

/// The smoothing filter of 8.4.4.2.3, applied to `r` when the mode and size call for it.
pub fn filter(r: &mut Refs, n: usize, mode: u32, luma: bool, strong_enabled: bool, bit_depth: u32) {
    if !luma || mode == 1 || n == 4 {
        return;
    }
    let min_dist = (mode as i32 - 26).abs().min((mode as i32 - 10).abs());
    let thres = match n {
        8 => 7,
        16 => 1,
        _ => 0,
    };
    if min_dist <= thres {
        return;
    }
    let corner = r.left[0];
    if strong_enabled && n == 32 {
        let lim = 1 << (bit_depth - 5);
        if (corner + r.top[2 * n] - 2 * r.top[n]).abs() < lim
            && (corner + r.left[2 * n] - 2 * r.left[n]).abs() < lim
        {
            let (l63, t63) = (r.left[64], r.top[64]);
            for i in 0..63usize {
                r.left[1 + i] = ((63 - i as i32) * corner + (i as i32 + 1) * l63 + 32) >> 6;
                r.top[1 + i] = ((63 - i as i32) * corner + (i as i32 + 1) * t63 + 32) >> 6;
            }
            return;
        }
    }
    let (l, t) = (r.left, r.top);
    r.left[0] = (l[1] + 2 * corner + t[1] + 2) >> 2;
    r.top[0] = r.left[0];
    for i in 1..2 * n {
        r.left[i] = (l[i + 1] + 2 * l[i] + l[i - 1] + 2) >> 2;
        r.top[i] = (t[i + 1] + 2 * t[i] + t[i - 1] + 2) >> 2;
    }
    // l[1] used the unfiltered corner above; the first entries need the corner in place of index 0.
    r.left[1] = (l[2] + 2 * l[1] + corner + 2) >> 2;
    r.top[1] = (t[2] + 2 * t[1] + corner + 2) >> 2;
}

/// Predict the block into `out` (row-major, stride `n`).
pub fn predict(out: &mut [i32], r: &Refs, n: usize, mode: u32, luma: bool, bit_depth: u32) {
    let log2 = n.trailing_zeros();
    match mode {
        0 => {
            for y in 0..n {
                for x in 0..n {
                    out[y * n + x] = (((n - 1 - x) as i32) * r.left[1 + y]
                        + (x as i32 + 1) * r.top[1 + n]
                        + ((n - 1 - y) as i32) * r.top[1 + x]
                        + (y as i32 + 1) * r.left[1 + n]
                        + n as i32)
                        >> (log2 + 1);
                }
            }
        }
        1 => {
            let mut sum = n as i32;
            for i in 0..n {
                sum += r.top[1 + i] + r.left[1 + i];
            }
            let dc = sum >> (log2 + 1);
            for v in out[..n * n].iter_mut() {
                *v = dc;
            }
            if luma && n < 32 {
                out[0] = (r.left[1] + 2 * dc + r.top[1] + 2) >> 2;
                for x in 1..n {
                    out[x] = (r.top[1 + x] + 3 * dc + 2) >> 2;
                }
                for y in 1..n {
                    out[y * n] = (r.left[1 + y] + 3 * dc + 2) >> 2;
                }
            }
        }
        _ => angular(out, r, n, mode, luma, bit_depth),
    }
}

fn angular(out: &mut [i32], r: &Refs, n: usize, mode: u32, luma: bool, bit_depth: u32) {
    let angle = INTRA_ANGLE[(mode - 2) as usize];
    let vertical = mode >= 18;
    // `main` runs along the prediction direction's reference edge, `side` is the other edge; both indexed from the corner.
    let (main, side) = if vertical { (&r.top, &r.left) } else { (&r.left, &r.top) };
    let mut refv = [0i32; 3 * 64 + 2];
    let off = 64usize; // refv[off + i] = ref[i]
    refv[off..=off + n].copy_from_slice(&main[..=n]);
    if angle < 0 {
        let last = (n as i32 * angle) >> 5;
        if last < -1 {
            let inv = INV_ANGLE[(mode - 11) as usize];
            for i in last..=-1 {
                let idx = (i * inv + 128) >> 8; // sample p[-1][-1 + idx] of the side edge
                refv[(off as i32 + i) as usize] = side[idx as usize];
            }
        }
    } else {
        refv[off + n + 1..=off + 2 * n].copy_from_slice(&main[n + 1..=2 * n]);
    }
    let maxv = (1i32 << bit_depth) - 1;
    for a in 0..n {
        // `a` runs across the prediction direction (y for vertical modes), `b` along it.
        let idx = ((a as i32 + 1) * angle) >> 5;
        let fact = ((a as i32 + 1) * angle) & 31;
        for b in 0..n {
            let base = (off as i32 + b as i32 + idx + 1) as usize;
            let v = if fact != 0 {
                ((32 - fact) * refv[base] + fact * refv[base + 1] + 16) >> 5
            } else {
                refv[base]
            };
            if vertical {
                out[a * n + b] = v;
            } else {
                out[b * n + a] = v;
            }
        }
    }
    if angle == 0 && luma && n < 32 {
        if vertical {
            for y in 0..n {
                out[y * n] = (r.top[1] + ((r.left[1 + y] - r.left[0]) >> 1)).clamp(0, maxv);
            }
        } else {
            for x in 0..n {
                out[x] = (r.left[1] + ((r.top[1 + x] - r.top[0]) >> 1)).clamp(0, maxv);
            }
        }
    }
}
