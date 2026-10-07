//! Inverse transforms (8.6.4): the DCT of 4 to 32 points and the 4-point DST, with the intermediate clipping and the final rounding.
use crate::tables::{DCT, DST4};

/// The inverse 2-D transform of the `n x n` block `coef` (row-major, `[y * n + x]`), `n = 1 << log2`. `max_x` and `max_y` bound the
/// non-zero coefficients (inclusive). The residual is written to `out` (row-major). `bit_depth` sets the final shift.
pub fn inverse(
    coef: &[i32],
    out: &mut [i32],
    log2: u32,
    dst: bool,
    bit_depth: u32,
    max_x: usize,
    max_y: usize,
) {
    let n = 1usize << log2;
    let step = 32 >> log2;
    let mat = |j: usize, i: usize| -> i32 { if dst { DST4[j][i] as i32 } else { DCT[j * step][i] as i32 } };
    // First stage: columns.  g[y][x] = clip((sum_j T[j][y] * d[j][x] + 64) >> 7)
    let mut g = [0i32; 32 * 32];
    for x in 0..=max_x.min(n - 1) {
        for y in 0..n {
            let mut s = 0i32;
            for j in 0..=max_y.min(n - 1) {
                s += mat(j, y) * coef[j * n + x];
            }
            g[y * n + x] = ((s + 64) >> 7).clamp(-32768, 32767);
        }
    }
    // Second stage: rows.  r[y][x] = (sum_j T[j][x] * g[y][j] + rnd) >> shift
    let shift = 20 - bit_depth;
    let rnd = 1i32 << (shift - 1);
    for y in 0..n {
        for x in 0..n {
            let mut s = 0i32;
            for j in 0..=max_x.min(n - 1) {
                s += mat(j, x) * g[y * n + j];
            }
            out[y * n + x] = (s + rnd) >> shift;
        }
    }
}

/// Transform skip (8.6.4.2 with `transform_skip_flag`): the coefficients are the residual, scaled to the transform's output range.
pub fn skip(coef: &[i32], out: &mut [i32], log2: u32, bit_depth: u32) {
    let n = 1usize << log2;
    let shift = 20 - bit_depth;
    let rnd = 1i32 << (shift - 1);
    let ts_shift = 5 + log2;
    for i in 0..n * n {
        out[i] = ((coef[i] << ts_shift) + rnd) >> shift;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn a_dc_coefficient_gives_a_flat_block() {
        for log2 in 2..=5u32 {
            let n = 1usize << log2;
            let mut coef = vec![0i32; n * n];
            coef[0] = 64;
            let mut out = vec![0i32; n * n];
            inverse(&coef, &mut out, log2, false, 8, 0, 0);
            assert!(out.iter().all(|v| *v == out[0]), "log2 {log2}");
            // 64 * 64 * 64 >> 7 >> 12 = 2 (rounded)
            assert_eq!(out[0], (((64 * 64 + 64) >> 7) * 64 + (1 << 11)) >> 12);
        }
    }

    #[test]
    fn the_bounds_only_skip_zeros() {
        let n = 8;
        let mut coef = vec![0i32; n * n];
        coef[0] = 100;
        coef[1] = -50;
        coef[8] = 30;
        coef[9] = 12;
        let (mut a, mut b) = (vec![0i32; n * n], vec![0i32; n * n]);
        inverse(&coef, &mut a, 3, false, 8, 1, 1);
        inverse(&coef, &mut b, 3, false, 8, 7, 7);
        assert_eq!(a, b);
    }
}
