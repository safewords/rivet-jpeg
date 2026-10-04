//! Helpers shared by the integration tests.
#![allow(dead_code)]

/// A deterministic test picture: smooth gradients, a few hard edges, and
/// some texture, so every coefficient band carries something.
pub fn picture(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 3);
    let mut seed = 0x1234_5678u32;
    for y in 0..h {
        for x in 0..w {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed % 9) as i32 - 4;
            let fx = x as f64 / w as f64;
            let fy = y as f64 / h as f64;
            let mut r = 255.0 * fx;
            let mut g = 255.0 * fy;
            let mut b = 128.0 + 100.0 * ((fx * 12.0).sin() * (fy * 9.0).cos());
            if (x / 13 + y / 11) % 5 == 0 {
                r = 255.0 - r;
                g = 40.0;
            }
            if ((x as i64 - w as i64 / 2).pow(2) + (y as i64 - h as i64 / 2).pow(2))
                < (w.min(h) as i64 / 4).pow(2)
            {
                b = 230.0;
                r *= 0.5;
            }
            for v in [r, g, b] {
                out.push((v as i32 + noise).clamp(0, 255) as u8);
            }
        }
    }
    out
}

/// PSNR in dB between two equal-length 8-bit buffers.
pub fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mse: f64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
        .sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

/// Decode strictly: every file this crate writes must pass.
pub fn strict(file: &[u8]) -> jpeg::Image {
    let opts = jpeg::DecodeOptions {
        strict: true,
        ..Default::default()
    };
    let img = jpeg::decode_with(file, &opts).expect("strict decode");
    assert!(
        img.complete && img.warnings.is_empty(),
        "{:?}",
        img.warnings
    );
    img
}
