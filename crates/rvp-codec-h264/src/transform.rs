//! Integer transforms and (de)quantisation, forward and inverse (clause 8.5 and the usual encoder inverses).
//!
//! All inverse functions are bit-exact with the specification for conforming input and never panic or
//! overflow for any input: scaled coefficients are saturated to 16 bits, which conforming streams never reach.
//! The forward functions are the encoder-side counterpart (the standard fixes only the inverse).

use crate::params::ScalingMatrices;

/// Zig-zag scan for 4x4 blocks: scan index to raster position (`y * 4 + x`), Figure 8-8.
pub const ZIGZAG_4X4: [u8; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// Zig-zag scan for 8x8 blocks: scan index to raster position (`y * 8 + x`), Figure 8-9.
pub const ZIGZAG_8X8: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7,
    14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39,
    46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// `QPc` as a function of `qPI` for `qPI` 30 to 51 (Table 8-15); below 30 it is the identity.
const QPC_HIGH: [u8; 22] =
    [29, 30, 31, 32, 32, 33, 34, 34, 35, 35, 36, 36, 37, 37, 37, 38, 38, 38, 39, 39, 39, 39];

/// Chroma QP for a luma QP and a `chroma_qp_index_offset` (8-bit video, so `QpBdOffset` is 0).
#[inline]
pub fn chroma_qp(qp_y: i32, offset: i32) -> i32 {
    let qpi = (qp_y + offset).clamp(0, 51);
    if qpi < 30 { qpi } else { QPC_HIGH[(qpi - 30) as usize] as i32 }
}

const NORM_4X4: [[i32; 3]; 6] =
    [[10, 16, 13], [11, 18, 14], [13, 20, 16], [14, 23, 18], [16, 25, 20], [18, 29, 23]];
const NORM_8X8: [[i32; 6]; 6] = [
    [20, 18, 32, 19, 25, 24],
    [22, 19, 35, 21, 28, 26],
    [26, 23, 42, 24, 33, 31],
    [28, 25, 45, 26, 35, 33],
    [32, 28, 51, 30, 40, 38],
    [36, 32, 58, 34, 46, 43],
];

/// `normAdjust4x4(m, i, j)` (8-315).
#[inline]
pub fn norm_adjust_4x4(m: usize, i: usize, j: usize) -> i32 {
    let c = if i % 2 == 0 && j % 2 == 0 {
        0
    } else if i % 2 == 1 && j % 2 == 1 {
        1
    } else {
        2
    };
    NORM_4X4[m][c]
}

/// `normAdjust8x8(m, i, j)` (8-318).
#[inline]
pub fn norm_adjust_8x8(m: usize, i: usize, j: usize) -> i32 {
    let c = if i % 4 == 0 && j % 4 == 0 {
        0
    } else if i % 2 == 1 && j % 2 == 1 {
        1
    } else if i % 4 == 2 && j % 4 == 2 {
        2
    } else if (i % 4 == 0 && j % 2 == 1) || (i % 2 == 1 && j % 4 == 0) {
        3
    } else if (i % 4 == 0 && j % 4 == 2) || (i % 4 == 2 && j % 4 == 0) {
        4
    } else {
        5
    };
    NORM_8X8[m][c]
}

/// `LevelScale4x4` and `LevelScale8x8` for every list and `qP % 6`, indexed by raster position.
#[derive(Clone)]
pub struct LevelScale {
    /// `[list][qp % 6][pos]` for the six 4x4 lists (Intra Y, Cb, Cr, Inter Y, Cb, Cr).
    pub l4: [[[i32; 16]; 6]; 6],
    /// `[list][qp % 6][pos]` for the 8x8 lists (Intra Y, Inter Y).
    pub l8: [[[i32; 64]; 6]; 2],
}

impl LevelScale {
    /// Build the tables from resolved scaling matrices.
    pub fn new(m: &ScalingMatrices) -> Self {
        let mut s = Self { l4: [[[0; 16]; 6]; 6], l8: [[[0; 64]; 6]; 2] };
        for list in 0..6 {
            let mut w = [0i32; 16];
            for (k, &pos) in ZIGZAG_4X4.iter().enumerate() {
                w[pos as usize] = m.l4[list][k] as i32;
            }
            for q in 0..6 {
                for pos in 0..16 {
                    s.l4[list][q][pos] = w[pos] * norm_adjust_4x4(q, pos / 4, pos % 4);
                }
            }
        }
        for (list, src) in [0usize, 1].iter().enumerate() {
            let mut w = [0i32; 64];
            for (k, &pos) in ZIGZAG_8X8.iter().enumerate() {
                w[pos as usize] = m.l8[*src][k] as i32;
            }
            for q in 0..6 {
                for pos in 0..64 {
                    s.l8[list][q][pos] = w[pos] * norm_adjust_8x8(q, pos / 8, pos % 8);
                }
            }
        }
        s
    }
}

#[inline]
fn sat16(v: i32) -> i32 {
    v.clamp(-32768, 32767)
}

/// Scale one 4x4 coefficient (8-336/8-337): `level * ls`, shifted by `qp / 6 - 4`.
#[inline]
pub fn dequant_4x4(level: i32, ls: i32, qp_per: u32) -> i32 {
    let v = level.wrapping_mul(ls);
    sat16(if qp_per >= 4 { v.wrapping_shl(qp_per - 4) } else { (v + (1 << (3 - qp_per))) >> (4 - qp_per) })
}

/// Scale one 8x8 coefficient (8-360/8-361): `level * ls`, shifted by `qp / 6 - 6`.
#[inline]
pub fn dequant_8x8(level: i32, ls: i32, qp_per: u32) -> i32 {
    let v = level.wrapping_mul(ls);
    sat16(if qp_per >= 6 { v.wrapping_shl(qp_per - 6) } else { (v + (1 << (5 - qp_per))) >> (6 - qp_per) })
}

/// One-dimensional inverse 4x4 transform of `d[0..4]` (8-338 to 8-345).
#[inline(always)]
fn idct4_1d(d0: i32, d1: i32, d2: i32, d3: i32) -> [i32; 4] {
    let e0 = d0 + d2;
    let e1 = d0 - d2;
    let e2 = (d1 >> 1) - d3;
    let e3 = d1 + (d3 >> 1);
    [e0 + e3, e1 + e2, e1 - e2, e0 - e3]
}

/// Inverse 4x4 transform in place: input scaled coefficients (raster order), output residual samples.
pub fn idct4x4(d: &mut [i32; 16]) {
    for i in 0..4 {
        let r = idct4_1d(d[i * 4], d[i * 4 + 1], d[i * 4 + 2], d[i * 4 + 3]);
        d[i * 4..i * 4 + 4].copy_from_slice(&r);
    }
    for j in 0..4 {
        let r = idct4_1d(d[j], d[4 + j], d[8 + j], d[12 + j]);
        for i in 0..4 {
            d[i * 4 + j] = (r[i] + 32) >> 6;
        }
    }
}

#[inline(always)]
fn idct8_1d(d: [i32; 8]) -> [i32; 8] {
    let e0 = d[0] + d[4];
    let e1 = -d[3] + d[5] - d[7] - (d[7] >> 1);
    let e2 = d[0] - d[4];
    let e3 = d[1] + d[7] - d[3] - (d[3] >> 1);
    let e4 = (d[2] >> 1) - d[6];
    let e5 = -d[1] + d[7] + d[5] + (d[5] >> 1);
    let e6 = d[2] + (d[6] >> 1);
    let e7 = d[3] + d[5] + d[1] + (d[1] >> 1);
    let f0 = e0 + e6;
    let f1 = e1 + (e7 >> 2);
    let f2 = e2 + e4;
    let f3 = e3 + (e5 >> 2);
    let f4 = e2 - e4;
    let f5 = (e3 >> 2) - e5;
    let f6 = e0 - e6;
    let f7 = e7 - (e1 >> 2);
    [f0 + f7, f2 + f5, f4 + f3, f6 + f1, f6 - f1, f4 - f3, f2 - f5, f0 - f7]
}

/// Inverse 8x8 transform in place (8.5.13.2).
pub fn idct8x8(d: &mut [i32; 64]) {
    for i in 0..8 {
        let mut row = [0i32; 8];
        row.copy_from_slice(&d[i * 8..i * 8 + 8]);
        d[i * 8..i * 8 + 8].copy_from_slice(&idct8_1d(row));
    }
    for j in 0..8 {
        let mut col = [0i32; 8];
        for i in 0..8 {
            col[i] = d[i * 8 + j];
        }
        let r = idct8_1d(col);
        for i in 0..8 {
            d[i * 8 + j] = (r[i] + 32) >> 6;
        }
    }
}

/// Inverse 4x4 Hadamard of the Intra16x16 luma DC coefficients, with scaling (8.5.10). `c` is in raster
/// order of the 4x4 DC matrix; `qp` is `QP'Y` and `ls00` is `LevelScale4x4[qP % 6][0][0]` of the Intra Y list.
pub fn luma_dc_dequant(c: &mut [i32; 16], qp: i32, ls00: i32) {
    let mut t = [0i32; 16];
    // f = A * c * A with A the 4x4 Hadamard matrix.
    for i in 0..4 {
        let (a, b, cc, d) = (c[i * 4], c[i * 4 + 1], c[i * 4 + 2], c[i * 4 + 3]);
        t[i * 4] = a.wrapping_add(b).wrapping_add(cc).wrapping_add(d);
        t[i * 4 + 1] = a.wrapping_add(b).wrapping_sub(cc).wrapping_sub(d);
        t[i * 4 + 2] = a.wrapping_sub(b).wrapping_sub(cc).wrapping_add(d);
        t[i * 4 + 3] = a.wrapping_sub(b).wrapping_add(cc).wrapping_sub(d);
    }
    let per = (qp / 6) as u32;
    for j in 0..4 {
        let (a, b, cc, d) = (t[j], t[4 + j], t[8 + j], t[12 + j]);
        let f = [
            a.wrapping_add(b).wrapping_add(cc).wrapping_add(d),
            a.wrapping_add(b).wrapping_sub(cc).wrapping_sub(d),
            a.wrapping_sub(b).wrapping_sub(cc).wrapping_add(d),
            a.wrapping_sub(b).wrapping_add(cc).wrapping_sub(d),
        ];
        for i in 0..4 {
            let v = f[i].wrapping_mul(ls00);
            c[i * 4 + j] =
                sat16(if per >= 6 { v.wrapping_shl(per - 6) } else { (v + (1 << (5 - per))) >> (6 - per) });
        }
    }
}

/// Inverse 2x2 transform of the chroma DC coefficients (4:2:0), with scaling (8.5.11). `c` is in raster order
/// (`c[0]`, `c[1]` top row); `qp` is `QP'c` and `ls00` is `LevelScale4x4[qP % 6][0][0]` of the chroma list.
pub fn chroma_dc_dequant(c: &mut [i32; 4], qp: i32, ls00: i32) {
    let f = [
        c[0].wrapping_add(c[1]).wrapping_add(c[2]).wrapping_add(c[3]),
        c[0].wrapping_sub(c[1]).wrapping_add(c[2]).wrapping_sub(c[3]),
        c[0].wrapping_add(c[1]).wrapping_sub(c[2]).wrapping_sub(c[3]),
        c[0].wrapping_sub(c[1]).wrapping_sub(c[2]).wrapping_add(c[3]),
    ];
    let per = (qp / 6) as u32;
    for i in 0..4 {
        c[i] = sat16((f[i].wrapping_mul(ls00).wrapping_shl(per)) >> 5);
    }
}

/// Add a 4x4 residual (raster order) to `dst` with clipping to 0..=255.
#[inline]
pub fn add_residual_4x4(res: &[i32; 16], dst: &mut [u8], stride: usize) {
    for y in 0..4 {
        let row = &mut dst[y * stride..y * stride + 4];
        for x in 0..4 {
            row[x] = (row[x] as i32 + res[y * 4 + x]).clamp(0, 255) as u8;
        }
    }
}

/// Add an 8x8 residual (raster order) to `dst` with clipping.
#[inline]
pub fn add_residual_8x8(res: &[i32; 64], dst: &mut [u8], stride: usize) {
    for y in 0..8 {
        let row = &mut dst[y * stride..y * stride + 8];
        for x in 0..8 {
            row[x] = (row[x] as i32 + res[y * 8 + x]).clamp(0, 255) as u8;
        }
    }
}

/// Add the residual of a block whose only non-zero scaled coefficient is the DC `dc`.
#[inline]
pub fn add_dc_4x4(dc: i32, dst: &mut [u8], stride: usize) {
    let v = (dc + 32) >> 6;
    for y in 0..4 {
        let row = &mut dst[y * stride..y * stride + 4];
        for p in row.iter_mut() {
            *p = (*p as i32 + v).clamp(0, 255) as u8;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Forward transforms and quantisation (encoder side).

const FWD_4X4: [[i32; 4]; 4] = [[1, 1, 1, 1], [2, 1, -1, -2], [1, -1, -1, 1], [1, -2, 2, -1]];
const FWD_8X8: [[i32; 8]; 8] = [
    [8, 8, 8, 8, 8, 8, 8, 8],
    [12, 10, 6, 3, -3, -6, -10, -12],
    [8, 4, -4, -8, -8, -4, 4, 8],
    [10, -3, -12, -6, 6, 12, 3, -10],
    [8, -8, -8, 8, 8, -8, -8, 8],
    [6, -12, 3, 10, -10, -3, 12, -6],
    [4, -8, 8, -4, -4, 8, -8, 4],
    [3, -6, 10, -12, 12, -10, 6, -3],
];

/// Forward 4x4 core transform `Cf * X * Cf^T` (raster in, raster out, in place).
pub fn fdct4x4(x: &mut [i32; 16]) {
    let mut t = [0i32; 16];
    for i in 0..4 {
        for j in 0..4 {
            t[i * 4 + j] = (0..4).map(|k| FWD_4X4[i][k] * x[k * 4 + j]).sum();
        }
    }
    for i in 0..4 {
        for j in 0..4 {
            x[i * 4 + j] = (0..4).map(|k| t[i * 4 + k] * FWD_4X4[j][k]).sum();
        }
    }
}

/// Forward 8x8 core transform with the integer matrix whose inverse is the specified 8x8 inverse transform.
pub fn fdct8x8(x: &mut [i32; 64]) {
    let mut t = [0i32; 64];
    for i in 0..8 {
        for j in 0..8 {
            t[i * 8 + j] = (0..8).map(|k| FWD_8X8[i][k] * x[k * 8 + j]).sum();
        }
    }
    for i in 0..8 {
        for j in 0..8 {
            x[i * 8 + j] = (0..8).map(|k| t[i * 8 + k] * FWD_8X8[j][k]).sum();
        }
    }
}

/// Forward 4x4 Hadamard for Intra16x16 DC: `(A * x * A) / 2` (the usual encoder scaling).
pub fn fwd_luma_dc(x: &mut [i32; 16]) {
    let mut t = [0i32; 16];
    for i in 0..4 {
        let (a, b, c, d) = (x[i * 4], x[i * 4 + 1], x[i * 4 + 2], x[i * 4 + 3]);
        t[i * 4] = a + b + c + d;
        t[i * 4 + 1] = a + b - c - d;
        t[i * 4 + 2] = a - b - c + d;
        t[i * 4 + 3] = a - b + c - d;
    }
    for j in 0..4 {
        let (a, b, c, d) = (t[j], t[4 + j], t[8 + j], t[12 + j]);
        x[j] = (a + b + c + d) / 2;
        x[4 + j] = (a + b - c - d) / 2;
        x[8 + j] = (a - b - c + d) / 2;
        x[12 + j] = (a - b + c - d) / 2;
    }
}

/// Forward 2x2 transform for chroma DC.
pub fn fwd_chroma_dc(x: &mut [i32; 4]) {
    let f = [
        x[0] + x[1] + x[2] + x[3],
        x[0] - x[1] + x[2] - x[3],
        x[0] + x[1] - x[2] - x[3],
        x[0] - x[1] - x[2] + x[3],
    ];
    *x = f;
}

/// `n_i^2` of the forward 4x4 basis rows and of the 8x8 rows (in the integer scaling above).
const N2_4: [f64; 4] = [4.0, 10.0, 4.0, 10.0];
const N2_8: [f64; 8] = [512.0, 578.0, 320.0, 578.0, 512.0, 578.0, 320.0, 578.0];

fn round_f(x: f64) -> i64 {
    (x + 0.5) as i64
}

/// Forward quantisation multipliers for the 4x4 transform: `MF` for each raster position so that
/// `level = (|w| * MF + f) >> (15 + qp / 6)`. For the flat matrix these are the classic 13107, 5243, 8066, ...
pub fn quant_mf_4x4(ls: &[i32; 16]) -> [i32; 16] {
    const S: [f64; 4] = [1.0, 0.5, 1.0, 0.5];
    let mut mf = [0i32; 16];
    for pos in 0..16 {
        let (i, j) = (pos / 4, pos % 4);
        let k = 1024.0 / (N2_4[i] * N2_4[j] * S[i] * S[j]);
        mf[pos] = round_f(k * 32768.0 / ls[pos] as f64) as i32;
    }
    mf
}

/// Forward quantisation multipliers for the 8x8 transform: `level = (|w| * MF + f) >> (22 + qp / 6)`.
pub fn quant_mf_8x8(ls: &[i32; 64]) -> [i32; 64] {
    let mut mf = [0i32; 64];
    for pos in 0..64 {
        let (i, j) = (pos / 8, pos % 8);
        let k = 262_144.0 / (N2_8[i] * N2_8[j]);
        mf[pos] = round_f(k * (1u64 << 22) as f64 / ls[pos] as f64) as i32;
    }
    mf
}

/// Quantise forward-transformed coefficients: `sign(w) * ((|w| * mf + f) >> qbits)` with `f = 2^qbits / 3`
/// for intra and `2^qbits / 6` for inter blocks. `qbits` is `15 + qp / 6` (4x4) or `22 + qp / 6` (8x8).
pub fn quantize(w: &[i32], mf: &[i32], qbits: u32, intra: bool, out: &mut [i32]) {
    let f: i64 = if intra { (1i64 << qbits) / 3 } else { (1i64 << qbits) / 6 };
    for i in 0..w.len() {
        let a = (w[i].unsigned_abs() as i64 * mf[i] as i64 + f) >> qbits;
        out[i] = if w[i] < 0 { -(a as i32) } else { a as i32 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_are_permutations() {
        let mut seen = [false; 64];
        for &p in ZIGZAG_8X8.iter() {
            seen[p as usize] = true;
        }
        assert!(seen.iter().all(|&b| b));
        let mut seen = [false; 16];
        for &p in ZIGZAG_4X4.iter() {
            seen[p as usize] = true;
        }
        assert!(seen.iter().all(|&b| b));
    }

    #[test]
    fn chroma_qp_table() {
        assert_eq!(chroma_qp(29, 0), 29);
        assert_eq!(chroma_qp(30, 0), 29);
        assert_eq!(chroma_qp(51, 0), 39);
        assert_eq!(chroma_qp(51, 12), 39);
        assert_eq!(chroma_qp(0, -12), 0);
    }

    #[test]
    fn flat_mf_matches_classic_values() {
        let m = ScalingMatrices::flat();
        let ls = LevelScale::new(&m);
        let mf = quant_mf_4x4(&ls.l4[0][0]);
        assert_eq!((mf[0], mf[5], mf[1]), (13107, 5243, 8066));
        let mf = quant_mf_4x4(&ls.l4[0][5]);
        assert_eq!((mf[0], mf[5], mf[1]), (7282, 2893, 4559));
    }

    #[test]
    fn dc_only_idct_is_flat() {
        let mut d = [0i32; 16];
        d[0] = 64 * 5;
        idct4x4(&mut d);
        assert!(d.iter().all(|&v| v == 5));
        let mut d = [0i32; 64];
        d[0] = 64 * 7;
        idct8x8(&mut d);
        assert!(d.iter().all(|&v| v == 7));
    }

    fn roundtrip_error4(qp: i32) -> i32 {
        let ls = LevelScale::new(&ScalingMatrices::flat());
        let mut x = [0i32; 16];
        let mut seed = 12345u32;
        for v in x.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = ((seed >> 24) as i32) % 100 - 50;
        }
        let orig = x;
        fdct4x4(&mut x);
        let mf = quant_mf_4x4(&ls.l4[0][(qp % 6) as usize]);
        let mut lv = [0i32; 16];
        quantize(&x, &mf, 15 + (qp / 6) as u32, true, &mut lv);
        let mut d = [0i32; 16];
        for p in 0..16 {
            d[p] = dequant_4x4(lv[p], ls.l4[0][(qp % 6) as usize][p], (qp / 6) as u32);
        }
        idct4x4(&mut d);
        (0..16).map(|i| (d[i] - orig[i]).abs()).max().unwrap()
    }

    fn roundtrip_error8(qp: i32) -> i32 {
        let ls = LevelScale::new(&ScalingMatrices::flat());
        let mut x = [0i32; 64];
        let mut seed = 777u32;
        for v in x.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = ((seed >> 24) as i32) % 100 - 50;
        }
        let orig = x;
        fdct8x8(&mut x);
        let mf = quant_mf_8x8(&ls.l8[0][(qp % 6) as usize]);
        let mut lv = [0i32; 64];
        quantize(&x, &mf, 22 + (qp / 6) as u32, true, &mut lv);
        let mut d = [0i32; 64];
        for p in 0..64 {
            d[p] = dequant_8x8(lv[p], ls.l8[0][(qp % 6) as usize][p], (qp / 6) as u32);
        }
        idct8x8(&mut d);
        (0..64).map(|i| (d[i] - orig[i]).abs()).max().unwrap()
    }

    #[test]
    fn forward_inverse_roundtrip_is_close() {
        assert!(roundtrip_error4(4) <= 2, "{}", roundtrip_error4(4));
        assert!(roundtrip_error4(20) <= 12, "{}", roundtrip_error4(20));
        assert!(roundtrip_error8(4) <= 3, "{}", roundtrip_error8(4));
        assert!(roundtrip_error8(20) <= 14, "{}", roundtrip_error8(20));
    }

    #[test]
    fn hadamard_dc_roundtrip() {
        // A flat DC field: only c[0] set; after the inverse Hadamard all 16 are equal.
        let mut c = [0i32; 16];
        c[0] = 16;
        luma_dc_dequant(&mut c, 12, 16 * 10);
        assert!(c.iter().all(|&v| v == c[0]));
        let mut c = [4, 0, 0, 0];
        chroma_dc_dequant(&mut c, 12, 160);
        assert!(c.iter().all(|&v| v == c[0]));
    }
}
