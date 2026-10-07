//! Constant tables of the standard: scans, the transform matrix, interpolation filters, intra angles, deblocking thresholds.

/// Up-right diagonal scan (6.5.3) of a `size x size` block as (x, y), `size` up to 8.
pub const fn diag_scan<const N: usize>(size: usize) -> [(u8, u8); N] {
    let mut out = [(0u8, 0u8); N];
    let (mut i, mut x, mut y) = (0usize, 0i32, 0i32);
    while i < size * size {
        while y >= 0 {
            if (x as usize) < size && (y as usize) < size {
                out[i] = (x as u8, y as u8);
                i += 1;
            }
            y -= 1;
            x += 1;
        }
        y = x;
        x = 0;
    }
    out
}

/// Horizontal scan (6.5.4).
pub const fn horizontal_scan<const N: usize>(size: usize) -> [(u8, u8); N] {
    let mut out = [(0u8, 0u8); N];
    let mut i = 0;
    while i < size * size {
        out[i] = ((i % size) as u8, (i / size) as u8);
        i += 1;
    }
    out
}

/// Vertical scan (6.5.5).
pub const fn vertical_scan<const N: usize>(size: usize) -> [(u8, u8); N] {
    let mut out = [(0u8, 0u8); N];
    let mut i = 0;
    while i < size * size {
        out[i] = ((i / size) as u8, (i % size) as u8);
        i += 1;
    }
    out
}

/// `ScanOrder[log2BlockSize][scanIdx]`: scanIdx 0 diagonal, 1 horizontal, 2 vertical; block sizes 1x1 (log2 0) to 8x8 (log2 3), as
/// 64-entry arrays of which the first `size * size` are used.
pub static SCAN: [[[(u8, u8); 64]; 3]; 4] = {
    let mut t = [[[(0u8, 0u8); 64]; 3]; 4];
    let mut l = 0;
    while l < 4 {
        t[l][0] = diag_scan::<64>(1 << l);
        t[l][1] = horizontal_scan::<64>(1 << l);
        t[l][2] = vertical_scan::<64>(1 << l);
        l += 1;
    }
    t
};

/// `ctxIdxMap` for `sig_coeff_flag` in 4x4 blocks (9.3.4.2.5), by `(yC << 2) + xC`.
pub const CTX_IDX_MAP_4X4: [u8; 16] = [0, 1, 4, 5, 2, 3, 4, 5, 6, 6, 8, 8, 7, 7, 8, 8];

/// The 32 distinct magnitudes of the transform matrix: `64 * sqrt(2) * cos(m * pi / 64)`, rounded as the standard has them.
const COS: [i32; 33] = [
    64, 90, 90, 90, 89, 88, 87, 85, 83, 82, 80, 78, 75, 73, 70, 67, 64, 61, 57, 54, 50, 46, 43, 38, 36, 31,
    25, 22, 18, 13, 9, 4, 0,
];

/// The 32x32 transform matrix (8.6.4.2); the smaller sizes use every `32/N`th row and the first `N` columns.
pub static DCT: [[i8; 32]; 32] = {
    let mut m = [[0i8; 32]; 32];
    let mut k = 0;
    while k < 32 {
        let mut n = 0;
        while n < 32 {
            let a = (k * (2 * n + 1)) % 128;
            let v = if k == 0 {
                64
            } else if a <= 32 {
                COS[a]
            } else if a <= 64 {
                -COS[64 - a]
            } else if a <= 96 {
                -COS[a - 64]
            } else {
                COS[128 - a]
            };
            m[k][n] = v as i8;
            n += 1;
        }
        k += 1;
    }
    m
};

/// The 4x4 DST-VII used for intra luma blocks of that size.
pub const DST4: [[i8; 4]; 4] = [[29, 55, 74, 84], [74, 74, 0, -74], [84, -29, -74, 55], [55, -84, 74, -29]];

/// `levelScale` (8.6.3).
pub const LEVEL_SCALE: [i32; 6] = [40, 45, 51, 57, 64, 72];

/// `intraPredAngle` for modes 2 to 34 (Table 8-5).
pub const INTRA_ANGLE: [i32; 33] = [
    32, 26, 21, 17, 13, 9, 5, 2, 0, -2, -5, -9, -13, -17, -21, -26, -32, -26, -21, -17, -13, -9, -5, -2, 0,
    2, 5, 9, 13, 17, 21, 26, 32,
];

/// `invAngle` for modes 11 to 25 (Table 8-6).
pub const INV_ANGLE: [i32; 15] =
    [-4096, -1638, -910, -630, -482, -390, -315, -256, -315, -390, -482, -630, -910, -1638, -4096];

/// The luma interpolation filters for the quarter-sample positions 1 to 3 (8.5.3.3.3.1), 8 taps.
pub const LUMA_FILTER: [[i32; 8]; 4] = [
    [0, 0, 0, 64, 0, 0, 0, 0],
    [-1, 4, -10, 58, 17, -5, 1, 0],
    [-1, 4, -11, 40, 40, -11, 4, -1],
    [0, 1, -5, 17, 58, -10, 4, -1],
];

/// The chroma interpolation filters for the eighth-sample positions 0 to 7 (8.5.3.3.3.2), 4 taps.
pub const CHROMA_FILTER: [[i32; 4]; 8] = [
    [0, 64, 0, 0],
    [-2, 58, 10, -2],
    [-4, 54, 16, -2],
    [-6, 46, 28, -4],
    [-4, 36, 36, -4],
    [-4, 28, 46, -6],
    [-2, 16, 54, -4],
    [-2, 10, 58, -2],
];

/// `QpC` as a function of `qPi` for 4:2:0 (Table 8-10).
pub fn chroma_qp(qpi: i32) -> i32 {
    match qpi {
        i32::MIN..=29 => qpi,
        30..=43 => [29, 30, 31, 32, 33, 33, 34, 34, 35, 35, 36, 36, 37, 37][(qpi - 30) as usize],
        _ => qpi - 6,
    }
}

/// Deblocking `beta'` by Q (Table 8-12).
pub const BETA: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 20, 22,
    24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64,
];

/// Deblocking `tc'` by Q (Table 8-12).
pub const TC: [u8; 54] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3,
    4, 4, 4, 5, 5, 6, 6, 7, 8, 9, 10, 11, 13, 14, 16, 18, 20, 22, 24,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_matrix_has_the_known_rows() {
        assert_eq!(&DCT[1][..8], &[90, 90, 88, 85, 82, 78, 73, 67]);
        assert_eq!(&DCT[2][..8], &[90, 87, 80, 70, 57, 43, 25, 9]);
        assert_eq!(&DCT[4][..4], &[89, 75, 50, 18]);
        assert_eq!(&DCT[8][..2], &[83, 36]);
        assert_eq!(&DCT[16][..4], &[64, -64, -64, 64]);
        assert_eq!(&DCT[31][..3], &[4, -13, 22]);
        // Rows are orthogonal to within the rounding of the integer approximation.
        for a in 0..32 {
            for b in 0..a {
                let dot: i32 = (0..32).map(|n| DCT[a][n] as i32 * DCT[b][n] as i32).sum();
                assert!(dot.abs() < 600, "rows {a} and {b}: {dot}");
            }
        }
    }

    #[test]
    fn scans_visit_every_position_once() {
        for l in 0..4 {
            for s in 0..3 {
                let n = 1usize << l;
                let mut seen = [false; 64];
                for i in 0..n * n {
                    let (x, y) = SCAN[l][s][i];
                    assert!((x as usize) < n && (y as usize) < n);
                    assert!(!seen[y as usize * n + x as usize]);
                    seen[y as usize * n + x as usize] = true;
                }
            }
        }
        // The 4x4 diagonal scan starts (0,0), (0,1), (1,0), (0,2).
        assert_eq!(&SCAN[2][0][..4], &[(0, 0), (0, 1), (1, 0), (0, 2)]);
    }

    #[test]
    fn chroma_qp_follows_the_table() {
        assert_eq!(
            [chroma_qp(29), chroma_qp(30), chroma_qp(34), chroma_qp(43), chroma_qp(44), chroma_qp(51)],
            [29, 29, 33, 37, 38, 45]
        );
    }
}
