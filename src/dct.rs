//! The 8x8 forward and inverse DCT of A.3.3, separably, in `f32`.
//!
//! Each 1-D transform splits into even and odd halves (the cosine basis is
//! symmetric or antisymmetric about the block's centre), so a row costs 32
//! multiplications instead of 64. Accuracy is checked in `tests/dct.rs` by
//! the IEEE 1180-1990 procedure, against a direct `f64` evaluation of the
//! definition.

use std::sync::OnceLock;

/// `M[x][u] = C(u)/2 * cos((2x+1) u pi / 16)`, with `C(0) = 1/sqrt(2)`:
/// one row of the 1-D inverse transform `f(x) = sum_u M[x][u] F(u)`.
fn basis() -> &'static [[f32; 8]; 8] {
    static M: OnceLock<[[f32; 8]; 8]> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = [[0f32; 8]; 8];
        for (x, row) in m.iter_mut().enumerate() {
            for (u, v) in row.iter_mut().enumerate() {
                let c = if u == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
                *v = (c / 2.0 * (((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0).cos()) as f32;
            }
        }
        m
    })
}

/// 1-D inverse transform of `F[0..8]` (read with `stride`) into `out`.
#[inline]
fn idct_1d(m: &[[f32; 8]; 8], f: [f32; 8]) -> [f32; 8] {
    let mut out = [0f32; 8];
    for x in 0..4 {
        let r = &m[x];
        let even = r[0] * f[0] + r[2] * f[2] + r[4] * f[4] + r[6] * f[6];
        let odd = r[1] * f[1] + r[3] * f[3] + r[5] * f[5] + r[7] * f[7];
        out[x] = even + odd;
        out[7 - x] = even - odd;
    }
    out
}

/// Inverse DCT of dequantised coefficients in natural order. Returns the
/// reconstructed values before the level shift, unrounded.
pub(crate) fn idct(coef: &[i32; 64]) -> [f32; 64] {
    let m = basis();
    let mut tmp = [0f32; 64];
    // Columns first: column u of the coefficient block gives column x.
    for u in 0..8 {
        let col = [
            coef[u],
            coef[8 + u],
            coef[16 + u],
            coef[24 + u],
            coef[32 + u],
            coef[40 + u],
            coef[48 + u],
            coef[56 + u],
        ];
        if col[1..].iter().all(|&c| c == 0) {
            let dc = col[0] as f32 * m[0][0];
            for y in 0..8 {
                tmp[y * 8 + u] = dc;
            }
            continue;
        }
        let out = idct_1d(m, col.map(|c| c as f32));
        for y in 0..8 {
            tmp[y * 8 + u] = out[y];
        }
    }
    let mut out = [0f32; 64];
    for y in 0..8 {
        let row: [f32; 8] = tmp[y * 8..y * 8 + 8].try_into().unwrap_or([0.0; 8]);
        let r = idct_1d(m, row);
        out[y * 8..y * 8 + 8].copy_from_slice(&r);
    }
    out
}

/// Inverse DCT to samples: rounds, adds `level` (2^(P-1)) and clamps to
/// `0..=max`, writing 8 rows of 8 into `dst` at `stride`.
pub(crate) fn idct_to_samples(coef: &[i32; 64], level: i32, max: i32, dst: &mut [u16], stride: usize) {
    // A block with only its DC term is flat.
    if coef[1..].iter().all(|&c| c == 0) {
        let v = ((coef[0] as f32) / 8.0).round() as i32 + level;
        let v = v.clamp(0, max) as u16;
        for y in 0..8 {
            dst[y * stride..y * stride + 8].fill(v);
        }
        return;
    }
    let out = idct(coef);
    for y in 0..8 {
        for x in 0..8 {
            let v = out[y * 8 + x].round() as i32 + level;
            dst[y * stride + x] = v.clamp(0, max) as u16;
        }
    }
}

/// Forward DCT (A.3.3) of 64 level-shifted samples in natural order.
pub(crate) fn fdct(s: &[f32; 64]) -> [f32; 64] {
    let m = basis();
    // F(u) = sum_x M[x][u] f(x): the transpose of the inverse.
    let fwd = |f: [f32; 8]| -> [f32; 8] {
        let mut out = [0f32; 8];
        let mut sum = [0f32; 4];
        let mut diff = [0f32; 4];
        for x in 0..4 {
            sum[x] = f[x] + f[7 - x];
            diff[x] = f[x] - f[7 - x];
        }
        for u in 0..8 {
            let src = if u % 2 == 0 { &sum } else { &diff };
            out[u] = m[0][u] * src[0] + m[1][u] * src[1] + m[2][u] * src[2] + m[3][u] * src[3];
        }
        out
    };
    let mut tmp = [0f32; 64];
    for y in 0..8 {
        let row: [f32; 8] = s[y * 8..y * 8 + 8].try_into().unwrap_or([0.0; 8]);
        tmp[y * 8..y * 8 + 8].copy_from_slice(&fwd(row));
    }
    let mut out = [0f32; 64];
    for u in 0..8 {
        let col = [tmp[u], tmp[8 + u], tmp[16 + u], tmp[24 + u], tmp[32 + u], tmp[40 + u], tmp[48 + u], tmp[56 + u]];
        let r = fwd(col);
        for v in 0..8 {
            out[v * 8 + u] = r[v];
        }
    }
    out
}

/// For the accuracy tests: the inverse transform rounded to integers,
/// before any level shift or clamping.
#[doc(hidden)]
pub fn idct_rounded(coef: &[i32; 64]) -> [i32; 64] {
    idct(coef).map(|v| v.round() as i32)
}

/// For the accuracy tests: the forward transform, unrounded.
#[doc(hidden)]
pub fn fdct_f32(samples: &[f32; 64]) -> [f32; 64] {
    fdct(samples)
}
