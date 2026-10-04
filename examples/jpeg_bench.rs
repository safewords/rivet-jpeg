//! Time encoding and decoding a synthetic 4000x3000 picture: `bench`.
//! `cargo run --release --example jpeg_bench`.
//! Baseline, with restart intervals (every 64 MCUs, which a sequential
//! scan's decoder spreads over threads) and progressive; decoding on one
//! thread and on all of them.
use std::time::Instant;

fn best<T>(mut f: impl FnMut() -> T) -> (std::time::Duration, T) {
    let mut t = std::time::Duration::MAX;
    let mut out = None;
    for _ in 0..15 {
        let s = Instant::now();
        let v = f();
        t = t.min(s.elapsed());
        out = Some(v);
    }
    (t, out.expect("ran"))
}

fn main() {
    let (w, h) = (4000usize, 3000usize);
    // Gradients and a pattern under film-grain-like noise: about as much
    // entropy-coded data per pixel as a photograph at quality 85.
    let mut seed = 0x2545_f491u32;
    let rgb: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let n = (seed % 61) as i32 - 30;
            let px = [(x / 16) as i32, (y / 12) as i32, ((x ^ y) & 255) as i32];
            px.map(|v| (v + n).clamp(0, 255) as u8)
        })
        .collect();
    for (progressive, restart_interval) in [(false, 0u16), (false, 64), (true, 0)] {
        let s = jpeg::EncodeSettings {
            quality: 85,
            progressive,
            restart_interval,
            ..Default::default()
        };
        let (te, f) =
            best(|| jpeg::encode(&rgb, w as u32, h as u32, jpeg::PixelFormat::Rgb, &s).unwrap());
        print!(
            "progressive={progressive} restart={restart_interval}: {} bytes, encode {te:?}",
            f.len()
        );
        for threads in [1usize, 0] {
            let opts = jpeg::DecodeOptions {
                threads,
                ..Default::default()
            };
            let (td, _) = best(|| jpeg::decode_with(&f, &opts).unwrap());
            let (tr, px) = best(|| {
                jpeg::decode_with(&f, &opts)
                    .unwrap()
                    .to_rgb8_with_threads(threads)
            });
            print!(
                "; {} decode {td:?}, +RGB {tr:?} ({})",
                if threads == 1 {
                    "1 thread"
                } else {
                    "all threads"
                },
                px.len()
            );
        }
        println!();
    }
}
