//! Intra prediction (8.3): 4x4, 8x8, 16x16 luma and 8x8 chroma, working on neighbour samples handed in by the
//! caller together with availability flags.
use crate::error::{Error, Result};

/// Neighbour availability bit: the row above (and, for NxN blocks, the row above and to the right).
pub const AV_TOP: u8 = 1;
/// Neighbour availability bit: the column to the left.
pub const AV_LEFT: u8 = 2;
/// Neighbour availability bit: the sample above and to the left.
pub const AV_TOPLEFT: u8 = 4;

const ERR: Error = Error::Invalid("intra prediction mode uses unavailable neighbours");

/// Predict an `N`x`N` block (N = 4 or 8) in directional modes 0 to 8.
///
/// `top` holds `2 * N` samples (the top-right ones already substituted if unavailable), `left` holds `N`, `tl` is
/// the corner sample; for N = 8 the caller passes the filtered reference samples. The prediction is written to `dst`
/// with the given `stride`.
pub fn predict_nxn<const N: usize>(
    dst: &mut [u8],
    stride: usize,
    mode: u8,
    top: &[u8],
    left: &[u8],
    tl: u8,
    avail: u8,
) -> Result<()> {
    let have_top = avail & AV_TOP != 0;
    let have_left = avail & AV_LEFT != 0;
    let have_tl = avail & AV_TOPLEFT != 0;
    let n = N as i32;
    let t = |i: i32| -> i32 { if i < 0 { tl as i32 } else { top[i as usize] as i32 } };
    let l = |j: i32| -> i32 { if j < 0 { tl as i32 } else { left[j as usize] as i32 } };
    let shift = if N == 4 { 3 } else { 4 };
    match mode {
        0 => {
            if !have_top {
                return Err(ERR);
            }
            for y in 0..N {
                dst[y * stride..y * stride + N].copy_from_slice(&top[..N]);
            }
        }
        1 => {
            if !have_left {
                return Err(ERR);
            }
            for y in 0..N {
                dst[y * stride..y * stride + N].fill(left[y]);
            }
        }
        2 => {
            let st: i32 = top[..N].iter().map(|&v| v as i32).sum();
            let sl: i32 = left[..N].iter().map(|&v| v as i32).sum();
            let v = match (have_top, have_left) {
                (true, true) => (st + sl + n) >> shift,
                (false, true) => (sl + n / 2) >> (shift - 1),
                (true, false) => (st + n / 2) >> (shift - 1),
                (false, false) => 128,
            } as u8;
            for y in 0..N {
                dst[y * stride..y * stride + N].fill(v);
            }
        }
        3 => {
            if !have_top {
                return Err(ERR);
            }
            for y in 0..n {
                for x in 0..n {
                    let v = if x == n - 1 && y == n - 1 {
                        (t(2 * n - 2) + 3 * t(2 * n - 1) + 2) >> 2
                    } else {
                        (t(x + y) + 2 * t(x + y + 1) + t(x + y + 2) + 2) >> 2
                    };
                    dst[y as usize * stride + x as usize] = v as u8;
                }
            }
        }
        4 => {
            if !(have_top && have_left && have_tl) {
                return Err(ERR);
            }
            for y in 0..n {
                for x in 0..n {
                    let v = if x > y {
                        (t(x - y - 2) + 2 * t(x - y - 1) + t(x - y) + 2) >> 2
                    } else if x < y {
                        (l(y - x - 2) + 2 * l(y - x - 1) + l(y - x) + 2) >> 2
                    } else {
                        (t(0) + 2 * (tl as i32) + l(0) + 2) >> 2
                    };
                    dst[y as usize * stride + x as usize] = v as u8;
                }
            }
        }
        5 => {
            if !(have_top && have_left && have_tl) {
                return Err(ERR);
            }
            for y in 0..n {
                for x in 0..n {
                    let z = 2 * x - y;
                    let v = if z >= 0 && z % 2 == 0 {
                        (t(x - (y >> 1) - 1) + t(x - (y >> 1)) + 1) >> 1
                    } else if z >= 0 {
                        (t(x - (y >> 1) - 2) + 2 * t(x - (y >> 1) - 1) + t(x - (y >> 1)) + 2) >> 2
                    } else if z == -1 {
                        (l(0) + 2 * (tl as i32) + t(0) + 2) >> 2
                    } else {
                        (l(y - 2 * x - 1) + 2 * l(y - 2 * x - 2) + l(y - 2 * x - 3) + 2) >> 2
                    };
                    dst[y as usize * stride + x as usize] = v as u8;
                }
            }
        }
        6 => {
            if !(have_top && have_left && have_tl) {
                return Err(ERR);
            }
            for y in 0..n {
                for x in 0..n {
                    let z = 2 * y - x;
                    let v = if z >= 0 && z % 2 == 0 {
                        (l(y - (x >> 1) - 1) + l(y - (x >> 1)) + 1) >> 1
                    } else if z >= 0 {
                        (l(y - (x >> 1) - 2) + 2 * l(y - (x >> 1) - 1) + l(y - (x >> 1)) + 2) >> 2
                    } else if z == -1 {
                        (l(0) + 2 * (tl as i32) + t(0) + 2) >> 2
                    } else {
                        (t(x - 2 * y - 1) + 2 * t(x - 2 * y - 2) + t(x - 2 * y - 3) + 2) >> 2
                    };
                    dst[y as usize * stride + x as usize] = v as u8;
                }
            }
        }
        7 => {
            if !have_top {
                return Err(ERR);
            }
            for y in 0..n {
                for x in 0..n {
                    let v = if y % 2 == 0 {
                        (t(x + (y >> 1)) + t(x + (y >> 1) + 1) + 1) >> 1
                    } else {
                        (t(x + (y >> 1)) + 2 * t(x + (y >> 1) + 1) + t(x + (y >> 1) + 2) + 2) >> 2
                    };
                    dst[y as usize * stride + x as usize] = v as u8;
                }
            }
        }
        8 => {
            if !have_left {
                return Err(ERR);
            }
            for y in 0..n {
                for x in 0..n {
                    let z = x + 2 * y;
                    let v = if z > 2 * n - 3 {
                        l(n - 1)
                    } else if z == 2 * n - 3 {
                        (l(n - 2) + 3 * l(n - 1) + 2) >> 2
                    } else if z % 2 == 0 {
                        (l(y + (x >> 1)) + l(y + (x >> 1) + 1) + 1) >> 1
                    } else {
                        (l(y + (x >> 1)) + 2 * l(y + (x >> 1) + 1) + l(y + (x >> 1) + 2) + 2) >> 2
                    };
                    dst[y as usize * stride + x as usize] = v as u8;
                }
            }
        }
        _ => return Err(Error::Invalid("intra NxN prediction mode out of range")),
    }
    Ok(())
}

/// 8x8 reference sample filtering (8.3.2.2.1). `top` has 16 samples, `left` 8; returns the filtered arrays and
/// corner. Unavailable parts are left unfiltered (they are not used).
pub fn filter_8x8_refs(top: &[u8; 16], left: &[u8; 8], tl: u8, avail: u8) -> ([u8; 16], [u8; 8], u8) {
    let have_top = avail & AV_TOP != 0;
    let have_left = avail & AV_LEFT != 0;
    let have_tl = avail & AV_TOPLEFT != 0;
    let mut ft = *top;
    let mut fl = *left;
    let mut ftl = tl;
    if have_top {
        ft[0] = if have_tl {
            ((tl as u32 + 2 * top[0] as u32 + top[1] as u32 + 2) >> 2) as u8
        } else {
            ((3 * top[0] as u32 + top[1] as u32 + 2) >> 2) as u8
        };
        for x in 1..15 {
            ft[x] = ((top[x - 1] as u32 + 2 * top[x] as u32 + top[x + 1] as u32 + 2) >> 2) as u8;
        }
        ft[15] = ((top[14] as u32 + 3 * top[15] as u32 + 2) >> 2) as u8;
    }
    if have_tl {
        ftl = if !have_top || !have_left {
            if have_top {
                ((3 * tl as u32 + top[0] as u32 + 2) >> 2) as u8
            } else if have_left {
                ((3 * tl as u32 + left[0] as u32 + 2) >> 2) as u8
            } else {
                tl
            }
        } else {
            ((top[0] as u32 + 2 * tl as u32 + left[0] as u32 + 2) >> 2) as u8
        };
    }
    if have_left {
        fl[0] = if have_tl {
            ((tl as u32 + 2 * left[0] as u32 + left[1] as u32 + 2) >> 2) as u8
        } else {
            ((3 * left[0] as u32 + left[1] as u32 + 2) >> 2) as u8
        };
        for y in 1..7 {
            fl[y] = ((left[y - 1] as u32 + 2 * left[y] as u32 + left[y + 1] as u32 + 2) >> 2) as u8;
        }
        fl[7] = ((left[6] as u32 + 3 * left[7] as u32 + 2) >> 2) as u8;
    }
    (ft, fl, ftl)
}

#[inline]
fn clip1(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Intra 16x16 prediction (8.3.3). `top` is 16 samples, `left` 16.
pub fn predict_16x16(
    dst: &mut [u8],
    stride: usize,
    mode: u8,
    top: &[u8; 16],
    left: &[u8; 16],
    tl: u8,
    avail: u8,
) -> Result<()> {
    let have_top = avail & AV_TOP != 0;
    let have_left = avail & AV_LEFT != 0;
    match mode {
        0 => {
            if !have_top {
                return Err(ERR);
            }
            for y in 0..16 {
                dst[y * stride..y * stride + 16].copy_from_slice(top);
            }
        }
        1 => {
            if !have_left {
                return Err(ERR);
            }
            for y in 0..16 {
                dst[y * stride..y * stride + 16].fill(left[y]);
            }
        }
        2 => {
            let st: i32 = top.iter().map(|&v| v as i32).sum();
            let sl: i32 = left.iter().map(|&v| v as i32).sum();
            let v = match (have_top, have_left) {
                (true, true) => (st + sl + 16) >> 5,
                (false, true) => (sl + 8) >> 4,
                (true, false) => (st + 8) >> 4,
                (false, false) => 128,
            } as u8;
            for y in 0..16 {
                dst[y * stride..y * stride + 16].fill(v);
            }
        }
        3 => {
            if !(have_top && have_left && avail & AV_TOPLEFT != 0) {
                return Err(ERR);
            }
            let t = |i: i32| if i < 0 { tl as i32 } else { top[i as usize] as i32 };
            let l = |i: i32| if i < 0 { tl as i32 } else { left[i as usize] as i32 };
            let mut h = 0;
            let mut v = 0;
            for k in 0..8 {
                h += (k + 1) * (t(8 + k) - t(6 - k));
                v += (k + 1) * (l(8 + k) - l(6 - k));
            }
            let a = 16 * (left[15] as i32 + top[15] as i32);
            let b = (5 * h + 32) >> 6;
            let c = (5 * v + 32) >> 6;
            for y in 0..16i32 {
                for x in 0..16i32 {
                    dst[y as usize * stride + x as usize] = clip1((a + b * (x - 7) + c * (y - 7) + 16) >> 5);
                }
            }
        }
        _ => return Err(Error::Invalid("intra 16x16 prediction mode out of range")),
    }
    Ok(())
}

/// Chroma prediction (8.3.4) for one 8x8 block of 4:2:0. Modes: 0 DC, 1 horizontal, 2 vertical, 3 plane.
pub fn predict_chroma(
    dst: &mut [u8],
    stride: usize,
    mode: u8,
    top: &[u8; 8],
    left: &[u8; 8],
    tl: u8,
    avail: u8,
) -> Result<()> {
    let have_top = avail & AV_TOP != 0;
    let have_left = avail & AV_LEFT != 0;
    match mode {
        0 => {
            for by in 0..2usize {
                for bx in 0..2usize {
                    let st: i32 = top[bx * 4..bx * 4 + 4].iter().map(|&v| v as i32).sum();
                    let sl: i32 = left[by * 4..by * 4 + 4].iter().map(|&v| v as i32).sum();
                    let v = if (bx == 0 && by == 0) || (bx > 0 && by > 0) {
                        match (have_top, have_left) {
                            (true, true) => (st + sl + 4) >> 3,
                            (false, true) => (sl + 2) >> 2,
                            (true, false) => (st + 2) >> 2,
                            (false, false) => 128,
                        }
                    } else if bx > 0 {
                        // top-right block: prefer the row above
                        if have_top {
                            (st + 2) >> 2
                        } else if have_left {
                            (sl + 2) >> 2
                        } else {
                            128
                        }
                    } else {
                        // bottom-left block: prefer the left column
                        if have_left {
                            (sl + 2) >> 2
                        } else if have_top {
                            (st + 2) >> 2
                        } else {
                            128
                        }
                    } as u8;
                    for y in 0..4 {
                        let o = (by * 4 + y) * stride + bx * 4;
                        dst[o..o + 4].fill(v);
                    }
                }
            }
        }
        1 => {
            if !have_left {
                return Err(ERR);
            }
            for y in 0..8 {
                dst[y * stride..y * stride + 8].fill(left[y]);
            }
        }
        2 => {
            if !have_top {
                return Err(ERR);
            }
            for y in 0..8 {
                dst[y * stride..y * stride + 8].copy_from_slice(top);
            }
        }
        3 => {
            if !(have_top && have_left && avail & AV_TOPLEFT != 0) {
                return Err(ERR);
            }
            let t = |i: i32| if i < 0 { tl as i32 } else { top[i as usize] as i32 };
            let l = |i: i32| if i < 0 { tl as i32 } else { left[i as usize] as i32 };
            let mut h = 0;
            let mut v = 0;
            for k in 0..4 {
                h += (k + 1) * (t(4 + k) - t(2 - k));
                v += (k + 1) * (l(4 + k) - l(2 - k));
            }
            let a = 16 * (left[7] as i32 + top[7] as i32);
            let b = (34 * h + 32) >> 6;
            let c = (34 * v + 32) >> 6;
            for y in 0..8i32 {
                for x in 0..8i32 {
                    dst[y as usize * stride + x as usize] = clip1((a + b * (x - 3) + c * (y - 3) + 16) >> 5);
                }
            }
        }
        _ => return Err(Error::Invalid("intra chroma prediction mode out of range")),
    }
    Ok(())
}
