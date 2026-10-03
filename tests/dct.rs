//! IDCT and FDCT accuracy, by the procedure of IEEE Std 1180-1990 (the
//! accuracy test T.83 refers decoders to): random blocks in three ranges
//! and both signs are transformed by a double-precision reference FDCT,
//! rounded and clamped to 12 bits, then inverse-transformed by the
//! reference IDCT and by the one under test, each rounded and clamped to
//! -256..255. The errors must meet:
//!
//! - peak error at every position at most 1;
//! - mean square error at every position at most 0.06, overall at most 0.02;
//! - mean error at every position at most 0.015 in magnitude, overall at
//!   most 0.0015;
//! - an all-zero block gives an all-zero output.
//!
//! The random generator is the linear congruential one IEEE 1180 specifies
//! as recalled here (`x = x * 1103515245 + 12345`, 31-bit), which may differ
//! from the standard's text in detail; the criteria are what matter.

use std::f64::consts::PI;

fn reference_fdct(s: &[f64; 64]) -> [f64; 64] {
    let mut out = [0f64; 64];
    for v in 0..8 {
        for u in 0..8 {
            let cu = if u == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
            let cv = if v == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
            let mut sum = 0.0;
            for y in 0..8 {
                for x in 0..8 {
                    sum += s[y * 8 + x]
                        * (((2 * x + 1) * u) as f64 * PI / 16.0).cos()
                        * (((2 * y + 1) * v) as f64 * PI / 16.0).cos();
                }
            }
            out[v * 8 + u] = 0.25 * cu * cv * sum;
        }
    }
    out
}

fn reference_idct(f: &[f64; 64]) -> [f64; 64] {
    let mut out = [0f64; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0.0;
            for v in 0..8 {
                for u in 0..8 {
                    let cu = if u == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
                    let cv = if v == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
                    sum += cu
                        * cv
                        * f[v * 8 + u]
                        * (((2 * x + 1) * u) as f64 * PI / 16.0).cos()
                        * (((2 * y + 1) * v) as f64 * PI / 16.0).cos();
                }
            }
            out[y * 8 + x] = 0.25 * sum;
        }
    }
    out
}

struct Lcg(u64);

impl Lcg {
    /// A value in `low..=high`.
    fn next(&mut self, low: i64, high: i64) -> i64 {
        self.0 = (self.0.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7FFF_FFFF;
        let x = self.0 as f64 / 2147483648.0;
        low + (x * (high - low + 1) as f64).floor() as i64
    }
}

struct Stats {
    peak: [i64; 64],
    se: [f64; 64],
    e: [f64; 64],
    n: usize,
}

fn run(low: i64, high: i64, sign: i64, blocks: usize) -> Stats {
    let mut rng = Lcg(1);
    let mut st = Stats { peak: [0; 64], se: [0.0; 64], e: [0.0; 64], n: blocks };
    for _ in 0..blocks {
        let mut s = [0f64; 64];
        for v in &mut s {
            *v = (rng.next(low, high) * sign) as f64;
        }
        let f = reference_fdct(&s);
        let mut fi = [0i32; 64];
        let mut fr = [0f64; 64];
        for k in 0..64 {
            let c = f[k].round().clamp(-2048.0, 2047.0);
            fi[k] = c as i32;
            fr[k] = c;
        }
        let r = reference_idct(&fr).map(|v| v.round().clamp(-256.0, 255.0) as i64);
        let t = jpeg::dct::idct_rounded(&fi).map(|v| i64::from(v).clamp(-256, 255));
        for k in 0..64 {
            let e = t[k] - r[k];
            st.peak[k] = st.peak[k].max(e.abs());
            st.se[k] += (e * e) as f64;
            st.e[k] += e as f64;
        }
    }
    st
}

#[test]
fn idct_meets_ieee_1180() {
    // The standard uses 10 000 blocks per case; the double-precision
    // reference here is a direct O(64^2) evaluation, so a debug build uses
    // fewer.
    let blocks = if cfg!(debug_assertions) { 1000 } else { 10_000 };
    println!("range       sign  peak  worst pmse  omse     worst pme  ome");
    for (low, high) in [(-256, 255), (-5, 5), (-300, 300)] {
        for sign in [1, -1] {
            let st = run(low, high, sign, blocks);
            let n = st.n as f64;
            let peak = *st.peak.iter().max().unwrap();
            let pmse = st.se.iter().map(|v| v / n).fold(0.0, f64::max);
            let omse = st.se.iter().sum::<f64>() / (n * 64.0);
            let pme = st.e.iter().map(|v| (v / n).abs()).fold(0.0, f64::max);
            let ome = (st.e.iter().sum::<f64>() / (n * 64.0)).abs();
            println!("{low:>5}..{high:<4} {sign:>3}   {peak}     {pmse:.5}     {omse:.6} {pme:.5}    {ome:.6}");
            assert!(peak <= 1, "peak error {peak}");
            assert!(pmse <= 0.06, "position MSE {pmse}");
            assert!(omse <= 0.02, "overall MSE {omse}");
            assert!(pme <= 0.015, "position mean error {pme}");
            assert!(ome <= 0.0015, "overall mean error {ome}");
        }
    }
    assert_eq!(jpeg::dct::idct_rounded(&[0; 64]), [0; 64]);
}

#[test]
fn fdct_matches_the_definition() {
    let mut rng = Lcg(7);
    let mut worst = 0f64;
    for _ in 0..2000 {
        let mut s = [0f32; 64];
        let mut d = [0f64; 64];
        for k in 0..64 {
            let v = rng.next(-128, 127);
            s[k] = v as f32;
            d[k] = v as f64;
        }
        let r = reference_fdct(&d);
        let t = jpeg::dct::fdct_f32(&s);
        for k in 0..64 {
            worst = worst.max((f64::from(t[k]) - r[k]).abs());
        }
    }
    println!("FDCT: worst absolute error against the definition {worst:.2e}");
    // Far below the half-unit at which a quantised coefficient could round
    // differently with a quantiser of 1.
    assert!(worst < 2e-3, "{worst}");
}
