//! The SSE2 side: the WebAssembly SIMD128 names the kernels use, as thin wrappers over `core::arch::x86_64`. Each is the lane-for-lane twin of
//! its namesake (the tests at the bottom check every one against plain integer arithmetic), so a kernel written for one runs bit-exactly on the
//! other. SSE2 is part of the x86_64 baseline: the intrinsics below are always there, and since Rust 1.87 calling them needs no `unsafe`
//! (the `unsafe` blocks that remain are the loads and stores through raw pointers, each with its SAFETY note, and the blocks around the
//! intrinsics are allowed to be "unused" for the compilers where they were still required).
#![allow(non_camel_case_types, unused_unsafe, missing_docs, clippy::missing_safety_doc)]

use core::arch::x86_64 as a;

/// A 128-bit vector (sixteen bytes, eight halves, four words ...): what the kernels pass around.
pub type v128 = a::__m128i;

// ---- memory ------------------------------------------------------------------------------------------------------------------------

/// Load 16 bytes from the start of `s` (panics if `s` is shorter).
#[inline(always)]
pub fn load(s: &[u8]) -> v128 {
    let s = &s[..16];
    // SAFETY: `s` has exactly 16 readable bytes, and `_mm_loadu_si128` has no alignment requirement.
    unsafe { a::_mm_loadu_si128(s.as_ptr().cast()) }
}

/// Load 8 bytes into the low half (the high half is zero).
#[inline(always)]
pub fn load8(s: &[u8]) -> v128 {
    let s = &s[..8];
    // SAFETY: 8 readable bytes; `_mm_loadl_epi64` reads exactly 8 and has no alignment requirement.
    unsafe { a::_mm_loadl_epi64(s.as_ptr().cast()) }
}

/// Load 4 bytes into the low lane (the rest is zero).
#[inline(always)]
pub fn load4(s: &[u8]) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        let s = &s[..4];
        let w = i32::from_le_bytes([s[0], s[1], s[2], s[3]]);
        a::_mm_cvtsi32_si128(w)
    }
}

/// Store all 16 bytes to the start of `s`.
#[inline(always)]
pub fn store(s: &mut [u8], v: v128) {
    let s = &mut s[..16];
    // SAFETY: 16 writable bytes; `_mm_storeu_si128` has no alignment requirement.
    unsafe { a::_mm_storeu_si128(s.as_mut_ptr().cast(), v) }
}

/// Store the low 8 bytes.
#[inline(always)]
pub fn store8(s: &mut [u8], v: v128) {
    let s = &mut s[..8];
    // SAFETY: 8 writable bytes; `_mm_storel_epi64` writes exactly 8 and has no alignment requirement.
    unsafe { a::_mm_storel_epi64(s.as_mut_ptr().cast(), v) }
}

/// Store the low 4 bytes.
#[inline(always)]
pub fn store4(s: &mut [u8], v: v128) {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        let s = &mut s[..4];
        s.copy_from_slice(&a::_mm_cvtsi128_si32(v).to_le_bytes());
    }
}

/// Replace the high 8 bytes of `v` with the first 8 bytes of `s`.
#[inline(always)]
pub fn load8_hi(v: v128, s: &[u8]) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_unpacklo_epi64(v, load8(s))
    }
}

// ---- splat ---------------------------------------------------------------------------------------------------------------------------

#[inline(always)]
pub fn u8x16_splat(x: u8) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_set1_epi8(x as i8)
    }
}
#[inline(always)]
pub fn i8x16_splat(x: i8) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_set1_epi8(x)
    }
}
#[inline(always)]
pub fn u16x8_splat(x: u16) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_set1_epi16(x as i16)
    }
}
#[inline(always)]
pub fn i16x8_splat(x: i16) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_set1_epi16(x)
    }
}
#[inline(always)]
pub fn u32x4_splat(x: u32) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_set1_epi32(x as i32)
    }
}
#[inline(always)]
pub fn i32x4_splat(x: i32) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_set1_epi32(x)
    }
}

// ---- bits ------------------------------------------------------------------------------------------------------------------------------

#[inline(always)]
pub fn v128_and(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_and_si128(x, y)
    }
}
#[inline(always)]
pub fn v128_or(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_or_si128(x, y)
    }
}
/// `(v1 & c) | (v2 & !c)`, bit by bit.
#[inline(always)]
pub fn v128_bitselect(v1: v128, v2: v128, c: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_or_si128(a::_mm_and_si128(v1, c), a::_mm_andnot_si128(c, v2))
    }
}

// ---- 16-bit lanes ------------------------------------------------------------------------------------------------------------------------

#[inline(always)]
pub fn i16x8_add(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_add_epi16(x, y)
    }
}
#[inline(always)]
pub fn i16x8_sub(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_sub_epi16(x, y)
    }
}
/// The low 16 bits of each product.
#[inline(always)]
pub fn i16x8_mul(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_mullo_epi16(x, y)
    }
}
/// Shift left by `n` bits (taken modulo 16).
#[inline(always)]
pub fn i16x8_shl(x: v128, n: u32) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_sll_epi16(x, a::_mm_cvtsi32_si128((n & 15) as i32))
    }
}
/// Arithmetic shift right (the sign is copied in).
#[inline(always)]
pub fn i16x8_shr(x: v128, n: u32) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_sra_epi16(x, a::_mm_cvtsi32_si128((n & 15) as i32))
    }
}
/// Logical shift right (zeros come in).
#[inline(always)]
pub fn u16x8_shr(x: v128, n: u32) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_srl_epi16(x, a::_mm_cvtsi32_si128((n & 15) as i32))
    }
}
#[inline(always)]
pub fn i16x8_min(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_min_epi16(x, y)
    }
}
#[inline(always)]
pub fn i16x8_max(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_max_epi16(x, y)
    }
}

// ---- 32-bit lanes ------------------------------------------------------------------------------------------------------------------------

#[inline(always)]
pub fn i32x4_add(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_add_epi32(x, y)
    }
}
#[inline(always)]
pub fn i32x4_sub(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_sub_epi32(x, y)
    }
}
/// The low 32 bits of each product (SSE2 has no such multiply: the even and odd lanes go through `pmuludq`, whose low halves are the same
/// for signed and unsigned operands).
#[inline(always)]
pub fn i32x4_mul(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        let even = a::_mm_mul_epu32(x, y);
        let odd = a::_mm_mul_epu32(a::_mm_srli_si128::<4>(x), a::_mm_srli_si128::<4>(y));
        let even = a::_mm_shuffle_epi32::<0b00_00_10_00>(even);
        let odd = a::_mm_shuffle_epi32::<0b00_00_10_00>(odd);
        a::_mm_unpacklo_epi32(even, odd)
    }
}
#[inline(always)]
pub fn i32x4_shr(x: v128, n: u32) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_sra_epi32(x, a::_mm_cvtsi32_si128((n & 31) as i32))
    }
}
#[inline(always)]
pub fn u32x4_shr(x: v128, n: u32) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_srl_epi32(x, a::_mm_cvtsi32_si128((n & 31) as i32))
    }
}
/// `x[2i] * y[2i] + x[2i+1] * y[2i+1]` as 32-bit lanes, from signed 16-bit inputs.
#[inline(always)]
pub fn i32x4_dot_i16x8(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_madd_epi16(x, y)
    }
}

// ---- 8-bit lanes -----------------------------------------------------------------------------------------------------------------------

#[inline(always)]
pub fn u8x16_add(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_add_epi8(x, y)
    }
}
#[inline(always)]
pub fn u8x16_sub(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_sub_epi8(x, y)
    }
}
#[inline(always)]
pub fn u8x16_sub_sat(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_subs_epu8(x, y)
    }
}
/// `(x + y + 1) >> 1` per byte.
#[inline(always)]
pub fn u8x16_avgr(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_avg_epu8(x, y)
    }
}
/// 0xFF in the bytes where `x < y` (unsigned), else 0.
#[inline(always)]
pub fn u8x16_lt(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        let le = a::_mm_cmpeq_epi8(a::_mm_min_epu8(x, y), x);
        a::_mm_andnot_si128(a::_mm_cmpeq_epi8(x, y), le)
    }
}
/// Logical shift right of each byte (SSE2 shifts words: the bits that crossed from the neighbour are masked off).
#[inline(always)]
pub fn u8x16_shr(x: v128, n: u32) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        let n = n & 7;
        let shifted = a::_mm_srl_epi16(x, a::_mm_cvtsi32_si128(n as i32));
        a::_mm_and_si128(shifted, a::_mm_set1_epi8((0xFFu32 >> n) as u8 as i8))
    }
}
/// The signed 16-bit lanes of `x` then `y` as unsigned bytes, saturated.
#[inline(always)]
pub fn u8x16_narrow_i16x8(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_packus_epi16(x, y)
    }
}
/// The signed 32-bit lanes of `x` then `y` as signed 16-bit lanes, saturated.
#[inline(always)]
pub fn i16x8_narrow_i32x4(x: v128, y: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_packs_epi32(x, y)
    }
}

// ---- widening ------------------------------------------------------------------------------------------------------------------------

#[inline(always)]
pub fn u16x8_extend_low_u8x16(x: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_unpacklo_epi8(x, a::_mm_setzero_si128())
    }
}
#[inline(always)]
pub fn u16x8_extend_high_u8x16(x: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_unpackhi_epi8(x, a::_mm_setzero_si128())
    }
}
#[inline(always)]
pub fn u32x4_extend_low_u16x8(x: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_unpacklo_epi16(x, a::_mm_setzero_si128())
    }
}
#[inline(always)]
pub fn u32x4_extend_high_u16x8(x: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_unpackhi_epi16(x, a::_mm_setzero_si128())
    }
}
#[inline(always)]
pub fn i32x4_extend_low_i16x8(x: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_srai_epi32::<16>(a::_mm_unpacklo_epi16(x, x))
    }
}
#[inline(always)]
pub fn i32x4_extend_high_i16x8(x: v128) -> v128 {
    // SAFETY: SSE2 is part of the x86_64 baseline, so these intrinsics are always available; no pointers are involved.
    unsafe {
        a::_mm_srai_epi32::<16>(a::_mm_unpackhi_epi16(x, x))
    }
}

// ---- shuffles --------------------------------------------------------------------------------------------------------------------------

/// Pick bytes of `x` (indices 0..16) and `y` (16..32) as `I0..I15` say. The patterns the kernels use (interleaves, halves, a duplicate of
/// the high half) are single SSE2 instructions, found at compile time; any other pattern is done through memory, which is correct but slow.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub fn i8x16_shuffle<
    const I0: usize,
    const I1: usize,
    const I2: usize,
    const I3: usize,
    const I4: usize,
    const I5: usize,
    const I6: usize,
    const I7: usize,
    const I8: usize,
    const I9: usize,
    const I10: usize,
    const I11: usize,
    const I12: usize,
    const I13: usize,
    const I14: usize,
    const I15: usize,
>(
    x: v128,
    y: v128,
) -> v128 {
    const fn is(idx: [usize; 16], want: [usize; 16]) -> bool {
        let mut i = 0;
        while i < 16 {
            if idx[i] != want[i] {
                return false;
            }
            i += 1;
        }
        true
    }
    const fn seq(start: usize, step: usize) -> [usize; 16] {
        // bytes interleaved: x[start..] and y[start..] alternately (step = 1: one byte from each, 2: two bytes, 4: four bytes).
        let mut o = [0usize; 16];
        let mut i = 0;
        while i < 16 {
            let group = i / step;
            let within = i % step;
            let from_y = group % 2;
            let n = group / 2;
            o[i] = from_y * 16 + start + n * step + within;
            i += 1;
        }
        o
    }
    let idx = [I0, I1, I2, I3, I4, I5, I6, I7, I8, I9, I10, I11, I12, I13, I14, I15];
    if is(idx, seq(0, 1)) {
        return unsafe { a::_mm_unpacklo_epi8(x, y) };
    }
    if is(idx, seq(8, 1)) {
        return unsafe { a::_mm_unpackhi_epi8(x, y) };
    }
    if is(idx, seq(0, 2)) {
        return unsafe { a::_mm_unpacklo_epi16(x, y) };
    }
    if is(idx, seq(8, 2)) {
        return unsafe { a::_mm_unpackhi_epi16(x, y) };
    }
    if is(idx, seq(0, 4)) {
        return unsafe { a::_mm_unpacklo_epi32(x, y) };
    }
    if is(idx, seq(8, 4)) {
        return unsafe { a::_mm_unpackhi_epi32(x, y) };
    }
    // The low halves of x and y side by side, and the high halves.
    if is(idx, [0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 20, 21, 22, 23]) {
        return unsafe { a::_mm_unpacklo_epi64(x, y) };
    }
    if is(idx, [8, 9, 10, 11, 12, 13, 14, 15, 24, 25, 26, 27, 28, 29, 30, 31]) {
        return unsafe { a::_mm_unpackhi_epi64(x, y) };
    }
    // The high half of x moved down. The upper half of the result is NOT x[0] splatted as the pattern says: it is zero. Kernels using this
    // pattern must not read the upper half (the test below checks only the low eight bytes for it).
    if is(idx, [8, 9, 10, 11, 12, 13, 14, 15, 0, 0, 0, 0, 0, 0, 0, 0]) {
        return unsafe { a::_mm_unpackhi_epi64(x, a::_mm_setzero_si128()) };
    }
    // Bytes of x in an order within each half (the 8-bit transpose helper: 0 4 1 5 2 6 3 7 | 8 12 9 13 10 14 11 15).
    if is(idx, [0, 4, 1, 5, 2, 6, 3, 7, 8, 12, 9, 13, 10, 14, 11, 15]) {
        // Two steps of 16-bit interleaves through the 32-bit lanes: (0 1 2 3 | 4 5 6 7 ...) -> pairs (0 4)(1 5)(2 6)(3 7).
        // SAFETY: register-only SSE2 intrinsics (x86_64 baseline).
        return unsafe {
            let lo = a::_mm_unpacklo_epi8(a::_mm_srli_si128::<0>(x), a::_mm_srli_si128::<4>(x));
            let hi = a::_mm_unpacklo_epi8(a::_mm_srli_si128::<8>(x), a::_mm_srli_si128::<12>(x));
            a::_mm_unpacklo_epi64(lo, hi)
        };
    }
    // Words of x and y: 8 9 24 25 10 11 26 27 ... = the high halves interleaved as 16-bit pairs, the same as seq(8, 2) above when x is
    // used for both; and the pattern with 32-bit pairs taken from the high halves of x and y:
    // 8..11, 24..27, 12..15, 28..31 is unpackhi_epi32.
    // Anything else: through memory.
    let (mut xs, mut ys) = ([0u8; 16], [0u8; 16]);
    // SAFETY: both arrays are 16 bytes.
    unsafe {
        a::_mm_storeu_si128(xs.as_mut_ptr().cast(), x);
        a::_mm_storeu_si128(ys.as_mut_ptr().cast(), y);
    }
    let mut out = [0u8; 16];
    let mut i = 0;
    while i < 16 {
        let k = idx[i];
        out[i] = if k < 16 { xs[k] } else { ys[k - 16] };
        i += 1;
    }
    // SAFETY: `out` is 16 bytes.
    unsafe { a::_mm_loadu_si128(out.as_ptr().cast()) }
}

/// Pick 32-bit lanes of `x` (0..4) and `y` (4..8): `I0..I3`. The two patterns the kernels use (a duplicate of the low or the high half) are
/// single instructions; others go through memory.
#[inline(always)]
pub fn i32x4_shuffle<const I0: usize, const I1: usize, const I2: usize, const I3: usize>(x: v128, y: v128) -> v128 {
    if I0 == 0 && I1 == 0 && I2 == 1 && I3 == 1 {
        return unsafe { a::_mm_unpacklo_epi32(x, x) };
    }
    if I0 == 2 && I1 == 2 && I2 == 3 && I3 == 3 {
        return unsafe { a::_mm_unpackhi_epi32(x, x) };
    }
    let (mut xs, mut ys) = ([0i32; 4], [0i32; 4]);
    // SAFETY: both arrays are 16 bytes.
    unsafe {
        a::_mm_storeu_si128(xs.as_mut_ptr().cast(), x);
        a::_mm_storeu_si128(ys.as_mut_ptr().cast(), y);
    }
    let idx = [I0, I1, I2, I3];
    let mut out = [0i32; 4];
    for (o, &k) in out.iter_mut().zip(&idx) {
        *o = if k < 4 { xs[k] } else { ys[k - 4] };
    }
    // SAFETY: `out` is 16 bytes.
    unsafe { a::_mm_loadu_si128(out.as_ptr().cast()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn bytes(v: v128) -> [u8; 16] {
        let mut o = [0u8; 16];
        store(&mut o, v);
        o
    }
    fn h(v: v128) -> [i16; 8] {
        let b = bytes(v);
        core::array::from_fn(|i| i16::from_le_bytes([b[2 * i], b[2 * i + 1]]))
    }
    fn w(v: v128) -> [i32; 4] {
        let b = bytes(v);
        core::array::from_fn(|i| i32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]))
    }
    fn vh(x: [i16; 8]) -> v128 {
        let b: Vec<u8> = x.iter().flat_map(|v| v.to_le_bytes()).collect();
        load(&b)
    }
    fn vw(x: [i32; 4]) -> v128 {
        let b: Vec<u8> = x.iter().flat_map(|v| v.to_le_bytes()).collect();
        load(&b)
    }
    /// A few hundred pseudo-random vectors, including the extremes.
    fn samples() -> Vec<[u8; 16]> {
        let mut seed = 0x9e37_79b9u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut v = vec![[0u8; 16], [0xFF; 16], [0x80; 16], [0x7F; 16], [1; 16]];
        for _ in 0..300 {
            v.push(core::array::from_fn(|_| {
                let r = rnd();
                match r % 7 {
                    0 => 0,
                    1 => 255,
                    2 => 128,
                    3 => 127,
                    _ => (r >> 8) as u8,
                }
            }));
        }
        v
    }

    #[test]
    fn every_operation_is_the_lane_for_lane_twin_of_its_webassembly_namesake() {
        let s = samples();
        for (i, x) in s.iter().enumerate() {
            let y = &s[(i * 7 + 3) % s.len()];
            let (vx, vy) = (load(x), load(y));
            // 8-bit lanes.
            let b = |f: &dyn Fn(u8, u8) -> u8| -> [u8; 16] { core::array::from_fn(|k| f(x[k], y[k])) };
            assert_eq!(bytes(u8x16_add(vx, vy)), b(&|p, q| p.wrapping_add(q)));
            assert_eq!(bytes(u8x16_sub(vx, vy)), b(&|p, q| p.wrapping_sub(q)));
            assert_eq!(bytes(u8x16_sub_sat(vx, vy)), b(&|p, q| p.saturating_sub(q)));
            assert_eq!(bytes(u8x16_avgr(vx, vy)), b(&|p, q| ((p as u16 + q as u16 + 1) >> 1) as u8));
            assert_eq!(bytes(u8x16_lt(vx, vy)), b(&|p, q| if p < q { 255 } else { 0 }));
            for n in 0..8 {
                assert_eq!(bytes(u8x16_shr(vx, n)), core::array::from_fn(|k| x[k] >> n), "u8 shr {n}");
            }
            assert_eq!(bytes(v128_and(vx, vy)), b(&|p, q| p & q));
            assert_eq!(bytes(v128_or(vx, vy)), b(&|p, q| p | q));
            let m = &s[(i * 5 + 1) % s.len()];
            assert_eq!(bytes(v128_bitselect(vx, vy, load(m))), core::array::from_fn(|k| (x[k] & m[k]) | (y[k] & !m[k])));
            // 16-bit lanes.
            let (hx, hy) = (h(vx), h(vy));
            let hh = |f: &dyn Fn(i16, i16) -> i16| -> [i16; 8] { core::array::from_fn(|k| f(hx[k], hy[k])) };
            assert_eq!(h(i16x8_add(vx, vy)), hh(&|p, q| p.wrapping_add(q)));
            assert_eq!(h(i16x8_sub(vx, vy)), hh(&|p, q| p.wrapping_sub(q)));
            assert_eq!(h(i16x8_mul(vx, vy)), hh(&|p, q| p.wrapping_mul(q)));
            assert_eq!(h(i16x8_min(vx, vy)), hh(&|p, q| p.min(q)));
            assert_eq!(h(i16x8_max(vx, vy)), hh(&|p, q| p.max(q)));
            for n in [0u32, 1, 3, 7, 8, 15] {
                assert_eq!(h(i16x8_shl(vx, n)), core::array::from_fn(|k| hx[k].wrapping_shl(n)), "shl {n}");
                assert_eq!(h(i16x8_shr(vx, n)), core::array::from_fn(|k| hx[k] >> n), "shr {n}");
                assert_eq!(h(u16x8_shr(vx, n)), core::array::from_fn(|k| ((hx[k] as u16) >> n) as i16), "ushr {n}");
            }
            // 32-bit lanes.
            let (wx, wy) = (w(vx), w(vy));
            let ww = |f: &dyn Fn(i32, i32) -> i32| -> [i32; 4] { core::array::from_fn(|k| f(wx[k], wy[k])) };
            assert_eq!(w(i32x4_add(vx, vy)), ww(&|p, q| p.wrapping_add(q)));
            assert_eq!(w(i32x4_sub(vx, vy)), ww(&|p, q| p.wrapping_sub(q)));
            assert_eq!(w(i32x4_mul(vx, vy)), ww(&|p, q| p.wrapping_mul(q)));
            for n in [0u32, 1, 5, 16, 31] {
                assert_eq!(w(i32x4_shr(vx, n)), core::array::from_fn(|k| wx[k] >> n), "i32 shr {n}");
                assert_eq!(w(u32x4_shr(vx, n)), core::array::from_fn(|k| ((wx[k] as u32) >> n) as i32), "u32 shr {n}");
            }
            assert_eq!(
                w(i32x4_dot_i16x8(vx, vy)),
                core::array::from_fn(|k| (hx[2 * k] as i32 * hy[2 * k] as i32).wrapping_add(hx[2 * k + 1] as i32 * hy[2 * k + 1] as i32))
            );
            // Narrowing.
            assert_eq!(
                bytes(u8x16_narrow_i16x8(vx, vy)),
                core::array::from_fn(|k| (if k < 8 { hx[k] } else { hy[k - 8] }).clamp(0, 255) as u8)
            );
            assert_eq!(
                h(i16x8_narrow_i32x4(vx, vy)),
                core::array::from_fn(|k| (if k < 4 { wx[k] } else { wy[k - 4] }).clamp(-32768, 32767) as i16)
            );
            // Widening.
            assert_eq!(h(u16x8_extend_low_u8x16(vx)), core::array::from_fn(|k| x[k] as i16));
            assert_eq!(h(u16x8_extend_high_u8x16(vx)), core::array::from_fn(|k| x[k + 8] as i16));
            assert_eq!(w(u32x4_extend_low_u16x8(vx)), core::array::from_fn(|k| hx[k] as u16 as i32));
            assert_eq!(w(u32x4_extend_high_u16x8(vx)), core::array::from_fn(|k| hx[k + 4] as u16 as i32));
            assert_eq!(w(i32x4_extend_low_i16x8(vx)), core::array::from_fn(|k| hx[k] as i32));
            assert_eq!(w(i32x4_extend_high_i16x8(vx)), core::array::from_fn(|k| hx[k + 4] as i32));
            // Splats and the memory helpers.
            assert_eq!(bytes(u8x16_splat(x[0])), [x[0]; 16]);
            assert_eq!(h(i16x8_splat(hx[0])), [hx[0]; 8]);
            assert_eq!(w(i32x4_splat(wx[0])), [wx[0]; 4]);
            assert_eq!(w(u32x4_splat(wx[0] as u32)), [wx[0]; 4]);
            assert_eq!(bytes(load8(x))[..8], x[..8]);
            assert_eq!(bytes(load8(x))[8..], [0; 8]);
            assert_eq!(bytes(load4(x))[..4], x[..4]);
            assert_eq!(bytes(load4(x))[4..], [0; 12]);
            assert_eq!(bytes(load8_hi(vx, y))[..8], x[..8]);
            assert_eq!(bytes(load8_hi(vx, y))[8..], y[..8]);
            let mut out = [0xAAu8; 20];
            store8(&mut out, vx);
            assert_eq!(out[..8], x[..8]);
            assert_eq!(out[8], 0xAA);
            let mut out = [0xAAu8; 8];
            store4(&mut out, vx);
            assert_eq!(out[..4], x[..4]);
            assert_eq!(out[4], 0xAA);
            let _ = (vw([1, 2, 3, 4]), vh([1; 8]));
        }
    }

    macro_rules! shuffle_cases {
        ($x:expr, $y:expr; $($idx:expr),+ $(,)?) => {$(
            {
                const I: [usize; 16] = $idx;
                let got = bytes(i8x16_shuffle::<{I[0]}, {I[1]}, {I[2]}, {I[3]}, {I[4]}, {I[5]}, {I[6]}, {I[7]}, {I[8]}, {I[9]}, {I[10]}, {I[11]}, {I[12]}, {I[13]}, {I[14]}, {I[15]}>(load(&$x), load(&$y)));
                let want: [u8; 16] = core::array::from_fn(|k| if I[k] < 16 { $x[I[k]] } else { $y[I[k] - 16] });
                assert_eq!(got, want, "{:?}", I);
            }
        )+};
    }

    #[test]
    fn every_shuffle_pattern_the_kernels_use_picks_the_bytes_it_says() {
        let x: [u8; 16] = core::array::from_fn(|k| k as u8 * 3 + 1);
        let y: [u8; 16] = core::array::from_fn(|k| 200 - k as u8 * 5);
        shuffle_cases!(x, y;
            [0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23],
            [8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31],
            [0, 1, 16, 17, 2, 3, 18, 19, 4, 5, 20, 21, 6, 7, 22, 23],
            [8, 9, 24, 25, 10, 11, 26, 27, 12, 13, 28, 29, 14, 15, 30, 31],
            [0, 1, 2, 3, 16, 17, 18, 19, 4, 5, 6, 7, 20, 21, 22, 23],
            [8, 9, 10, 11, 24, 25, 26, 27, 12, 13, 14, 15, 28, 29, 30, 31],
            [0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 20, 21, 22, 23],
            [8, 9, 10, 11, 12, 13, 14, 15, 24, 25, 26, 27, 28, 29, 30, 31],
            [0, 4, 1, 5, 2, 6, 3, 7, 8, 12, 9, 13, 10, 14, 11, 15],
            // Patterns no kernel uses: the slow, general way.
            [31, 0, 17, 3, 5, 9, 30, 2, 1, 1, 16, 16, 15, 14, 13, 12],
        );
        let (vx, vy) = (load(&x), load(&y));
        assert_eq!(bytes(i8x16_shuffle::<8, 9, 10, 11, 12, 13, 14, 15, 0, 0, 0, 0, 0, 0, 0, 0>(vx, vy))[..8], x[8..]);
        let xw = w(vx);
        assert_eq!(w(i32x4_shuffle::<0, 0, 1, 1>(vx, vx)), [xw[0], xw[0], xw[1], xw[1]]);
        assert_eq!(w(i32x4_shuffle::<2, 2, 3, 3>(vx, vx)), [xw[2], xw[2], xw[3], xw[3]]);
        let yw = w(vy);
        assert_eq!(w(i32x4_shuffle::<0, 5, 2, 7>(vx, vy)), [xw[0], yw[1], xw[2], yw[3]]);
    }
}
