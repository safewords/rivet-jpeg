//! The 8x8 forward and inverse DCT of A.3.3, separably, in `f32`.
//!
//! Each 1-D transform splits into even and odd halves (the cosine basis is
//! symmetric or antisymmetric about the block's centre), so a row costs 32
//! multiplications instead of 64. Accuracy is checked in `tests/dct.rs` by
//! the IEEE 1180-1990 procedure, against a direct `f64` evaluation of the
//! definition.
//!
//! The transforms are written eight lanes at a time (a row of the block, or
//! one value from each of eight rows), so that the compiler turns them into
//! vector code; on x86-64 they are compiled for AVX2 where the processor has
//! it. Every lane performs the same multiplications and additions, in the
//! same order, as the one-row-at-a-time transform (`idct_reference` and
//! `fdct_reference`, kept for the tests), with no fused multiply-add, so the results are
//! identical to the last bit on every processor; the unit tests check this.

use std::sync::OnceLock;

/// Eight values processed together.
type Lanes = [f32; 8];

/// `M[x][u] = C(u)/2 * cos((2x+1) u pi / 16)`, with `C(0) = 1/sqrt(2)`:
/// one row of the 1-D inverse transform `f(x) = sum_u M[x][u] F(u)`.
#[cfg(test)]
fn basis() -> &'static [[f32; 8]; 8] {
    &tables().m
}

/// The basis and the same values laid out for the lane-wise transforms.
struct Tables {
    m: [[f32; 8]; 8],
    /// Inverse rows: for coefficient `2k`, lane `x` holds `M[x][2k]` for
    /// x < 4 and `M[7-x][2k]` (the same value) for x >= 4.
    even: [Lanes; 4],
    /// For coefficient `2k+1`, lane `x` holds `M[x][2k+1]` for x < 4 and
    /// `-M[7-x][2k+1]` for x >= 4, so that `even + odd` gives the scalar
    /// transform's `even - odd` in the upper half exactly.
    odd: [Lanes; 4],
    /// Forward rows: lane `u` holds `M[x][u]`, for each `x` < 4.
    fwd: [Lanes; 4],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut m = [[0f32; 8]; 8];
        for (x, row) in m.iter_mut().enumerate() {
            for (u, v) in row.iter_mut().enumerate() {
                let c = if u == 0 {
                    std::f64::consts::FRAC_1_SQRT_2
                } else {
                    1.0
                };
                *v = (c / 2.0 * (((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0).cos())
                    as f32;
            }
        }
        let even = std::array::from_fn(|k| {
            std::array::from_fn(|x| if x < 4 { m[x][2 * k] } else { m[7 - x][2 * k] })
        });
        let odd = std::array::from_fn(|k| {
            std::array::from_fn(|x| {
                if x < 4 {
                    m[x][2 * k + 1]
                } else {
                    -m[7 - x][2 * k + 1]
                }
            })
        });
        let fwd = std::array::from_fn(|x| m[x]);
        Tables { m, even, odd, fwd }
    })
}

#[inline(always)]
fn add(a: Lanes, b: Lanes) -> Lanes {
    std::array::from_fn(|i| a[i] + b[i])
}

#[inline(always)]
fn sub(a: Lanes, b: Lanes) -> Lanes {
    std::array::from_fn(|i| a[i] - b[i])
}

/// `a * b` lane by lane.
#[inline(always)]
fn mul(a: Lanes, b: Lanes) -> Lanes {
    std::array::from_fn(|i| a[i] * b[i])
}

/// `s * b`, `s` in every lane.
#[inline(always)]
fn scale(s: f32, b: Lanes) -> Lanes {
    std::array::from_fn(|i| s * b[i])
}

/// The inverse transform eight lanes at a time: rows of the result.
#[inline(always)]
fn idct_lanes(coef: &[i32; 64]) -> [Lanes; 8] {
    let t = tables();
    let m = &t.m;
    let f: [Lanes; 8] = std::array::from_fn(|k| std::array::from_fn(|u| coef[k * 8 + u] as f32));
    // Columns: lane u is column u; the scalar transform's
    // `r[0] * f[0] + r[2] * f[2] + ...` for each output row.
    let mut tmp = [[0f32; 8]; 8];
    for y in 0..4 {
        let r = &m[y];
        let even = add(
            add(add(scale(r[0], f[0]), scale(r[2], f[2])), scale(r[4], f[4])),
            scale(r[6], f[6]),
        );
        let odd = add(
            add(add(scale(r[1], f[1]), scale(r[3], f[3])), scale(r[5], f[5])),
            scale(r[7], f[7]),
        );
        tmp[y] = add(even, odd);
        tmp[7 - y] = sub(even, odd);
    }
    // Rows: lane x is output column x.
    let mut out = [[0f32; 8]; 8];
    for (o, r) in out.iter_mut().zip(&tmp) {
        let even = add(
            add(
                add(scale(r[0], t.even[0]), scale(r[2], t.even[1])),
                scale(r[4], t.even[2]),
            ),
            scale(r[6], t.even[3]),
        );
        let odd = add(
            add(
                add(scale(r[1], t.odd[0]), scale(r[3], t.odd[1])),
                scale(r[5], t.odd[2]),
            ),
            scale(r[7], t.odd[3]),
        );
        *o = add(even, odd);
    }
    out
}

/// Inverse DCT of dequantised coefficients in natural order. Returns the
/// reconstructed values before the level shift, unrounded.
pub(crate) fn idct(coef: &[i32; 64]) -> [f32; 64] {
    let rows = crate::simd::with_wide_vectors(|| idct_lanes(coef));
    let mut out = [0f32; 64];
    for (o, r) in out.as_chunks_mut::<8>().0.iter_mut().zip(&rows) {
        *o = *r;
    }
    out
}

/// Inverse DCT to samples: rounds, adds `level` (2^(P-1)) and clamps to
/// `0..=max`, writing 8 rows of 8 into `dst` at `stride`.
pub(crate) fn idct_to_samples(
    coef: &[i32; 64],
    level: i32,
    max: i32,
    dst: &mut [u16],
    stride: usize,
) {
    // A block with only its DC term is flat.
    if coef[1..].iter().all(|&c| c == 0) {
        let v = ((coef[0] as f32) / 8.0).round() as i32 + level;
        let v = v.clamp(0, max) as u16;
        for y in 0..8 {
            dst[y * stride..y * stride + 8].fill(v);
        }
        return;
    }
    crate::simd::with_wide_vectors(|| {
        let rows = idct_lanes(coef);
        for (y, r) in rows.iter().enumerate() {
            let d = &mut dst[y * stride..y * stride + 8];
            let v: [i32; 8] = std::array::from_fn(|x| (r[x].round() as i32 + level).clamp(0, max));
            for (o, v) in d.iter_mut().zip(v) {
                *o = v as u16;
            }
        }
    })
}

/// The forward transform eight lanes at a time: rows of the result.
#[inline(always)]
fn fdct_lanes(s: &[f32; 64]) -> [Lanes; 8] {
    let t = tables();
    let m = &t.m;
    // Rows: lane u is output coefficient u; the even coefficients take the
    // row's mirrored sums, the odd ones its differences.
    let mut tmp = [[0f32; 8]; 8];
    for (y, o) in tmp.iter_mut().enumerate() {
        let f = &s[y * 8..y * 8 + 8];
        let src: [Lanes; 4] = std::array::from_fn(|x| {
            let (sum, diff) = (f[x] + f[7 - x], f[x] - f[7 - x]);
            std::array::from_fn(|u| if u % 2 == 0 { sum } else { diff })
        });
        *o = add(
            add(
                add(mul(t.fwd[0], src[0]), mul(t.fwd[1], src[1])),
                mul(t.fwd[2], src[2]),
            ),
            mul(t.fwd[3], src[3]),
        );
    }
    // Columns: lane u is column u.
    let sum: [Lanes; 4] = std::array::from_fn(|x| add(tmp[x], tmp[7 - x]));
    let diff: [Lanes; 4] = std::array::from_fn(|x| sub(tmp[x], tmp[7 - x]));
    std::array::from_fn(|v| {
        let src = if v % 2 == 0 { &sum } else { &diff };
        add(
            add(
                add(scale(m[0][v], src[0]), scale(m[1][v], src[1])),
                scale(m[2][v], src[2]),
            ),
            scale(m[3][v], src[3]),
        )
    })
}

/// Forward DCT (A.3.3) of 64 level-shifted samples in natural order.
pub(crate) fn fdct(s: &[f32; 64]) -> [f32; 64] {
    let rows = crate::simd::with_wide_vectors(|| fdct_lanes(s));
    let mut out = [0f32; 64];
    for (o, r) in out.as_chunks_mut::<8>().0.iter_mut().zip(&rows) {
        *o = *r;
    }
    out
}

/// Forward DCT and quantisation: `round(F / q)` for each coefficient, in
/// natural order.
pub(crate) fn fdct_quantise(s: &[f32; 64], q: &[u16; 64]) -> [i16; 64] {
    crate::simd::with_wide_vectors(|| {
        let rows = fdct_lanes(s);
        let mut out = [0i16; 64];
        for (v, r) in rows.iter().enumerate() {
            let qv: Lanes = std::array::from_fn(|u| f32::from(q[v * 8 + u]));
            for u in 0..8 {
                out[v * 8 + u] = (r[u] / qv[u]).round() as i16;
            }
        }
        out
    })
}

/// 1-D inverse transform of `F[0..8]`, one row at a time: the reference
/// the lane-wise transform reproduces.
#[cfg(test)]
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

/// The inverse transform a row or column at a time (columns first).
#[cfg(test)]
fn idct_reference(coef: &[i32; 64]) -> [f32; 64] {
    let m = basis();
    let mut tmp = [0f32; 64];
    for u in 0..8 {
        let col = std::array::from_fn(|k| coef[k * 8 + u] as f32);
        let out = idct_1d(m, col);
        for y in 0..8 {
            tmp[y * 8 + u] = out[y];
        }
    }
    let mut out = [0f32; 64];
    for y in 0..8 {
        let row: [f32; 8] = tmp[y * 8..y * 8 + 8].try_into().unwrap();
        out[y * 8..y * 8 + 8].copy_from_slice(&idct_1d(m, row));
    }
    out
}

/// The forward transform a row or column at a time (rows first).
#[cfg(test)]
fn fdct_reference(s: &[f32; 64]) -> [f32; 64] {
    let m = basis();
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
        let row: [f32; 8] = s[y * 8..y * 8 + 8].try_into().unwrap();
        tmp[y * 8..y * 8 + 8].copy_from_slice(&fwd(row));
    }
    let mut out = [0f32; 64];
    for u in 0..8 {
        let col = std::array::from_fn(|k| tmp[k * 8 + u]);
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn range(&mut self, lo: i32, hi: i32) -> i32 {
            lo + (self.next() % (hi - lo + 1) as u64) as i32
        }
    }

    fn bits(v: &[f32; 64]) -> [u32; 64] {
        v.map(|x| if x == 0.0 { 0 } else { x.to_bits() })
    }

    #[test]
    fn lane_transforms_match_the_reference_exactly() {
        let mut r = Rng(0x5EED_1234_0000_0001);
        for trial in 0..20_000 {
            let mut c = [0i32; 64];
            // Sparse and dense blocks, small and extreme values (12-bit
            // dequantised coefficients can reach about 2^15 * 255).
            let (lo, hi) = match trial % 4 {
                0 => (-300, 300),
                1 => (-2048, 2047),
                2 => (-32768 * 64, 32767 * 64),
                _ => (-5, 5),
            };
            let density = 1 + trial % 64;
            for (k, v) in c.iter_mut().enumerate() {
                if k == 0 || (r.next() % 64) < density as u64 {
                    *v = r.range(lo, hi);
                }
            }
            assert_eq!(
                bits(&idct(&c)),
                bits(&idct_reference(&c)),
                "idct trial {trial}"
            );
            let s: [f32; 64] = std::array::from_fn(|_| {
                r.range(-2048, 2047) as f32 + (r.range(0, 3) as f32) * 0.25
            });
            assert_eq!(
                bits(&fdct(&s)),
                bits(&fdct_reference(&s)),
                "fdct trial {trial}"
            );
            let q: [u16; 64] = std::array::from_fn(|_| r.range(1, 255) as u16);
            let want = fdct_reference(&s);
            let want: [i16; 64] =
                std::array::from_fn(|n| (want[n] / f32::from(q[n])).round() as i16);
            assert_eq!(fdct_quantise(&s, &q), want, "quantise trial {trial}");
        }
        // Ties: values exactly halfway round away from zero.
        let q = [1u16; 64];
        let s = [0.5f32; 64];
        assert_eq!(fdct_quantise(&s, &q), {
            let f = fdct_reference(&s);
            std::array::from_fn(|n| (f[n] / 1.0).round() as i16)
        });
    }

    #[test]
    fn samples_match_the_reference() {
        let mut r = Rng(0xABCD_EF01_2345_6789);
        for trial in 0..5_000 {
            let mut c = [0i32; 64];
            for v in c.iter_mut() {
                if r.next().is_multiple_of(3) {
                    *v = r.range(-1500, 1500);
                }
            }
            for (level, max) in [(128, 255), (2048, 4095)] {
                let mut got = vec![0u16; 8 * 11];
                idct_to_samples(&c, level, max, &mut got, 11);
                let want = idct_reference(&c);
                for y in 0..8 {
                    for x in 0..8 {
                        let w = (want[y * 8 + x].round() as i32 + level).clamp(0, max) as u16;
                        if c[1..].iter().any(|&v| v != 0) {
                            assert_eq!(got[y * 11 + x], w, "trial {trial} ({y}, {x})");
                        }
                    }
                }
            }
        }
    }
}
