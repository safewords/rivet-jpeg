//! A sequential Huffman scan's restart intervals are decoded side by side
//! when there are several: the result — samples, warnings, completeness,
//! and the encoder's bytes — must be the same on one thread as on many,
//! for well-formed files and for every way the restart structure can be
//! broken (a marker missing, misnumbered, data before one, a corrupt
//! interval, a truncated scan), where the decoder falls back to the serial
//! loop and its warnings.

mod common;
use jpeg::{DecodeOptions, EncodeSettings, Image, PixelFormat, Subsampling};

fn decode(f: &[u8], threads: usize) -> Option<Image> {
    jpeg::decode_with(
        f,
        &DecodeOptions {
            threads,
            ..Default::default()
        },
    )
    .ok()
}

fn same(f: &[u8], what: &str) {
    let one = decode(f, 1);
    for threads in [2, 5, 0] {
        let many = decode(f, threads);
        match (&one, &many) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                assert!(
                    a.planes == b.planes,
                    "{what}: samples differ at {threads} threads"
                );
                assert_eq!(
                    a.warnings, b.warnings,
                    "{what}: warnings at {threads} threads"
                );
                assert_eq!(a.complete, b.complete, "{what}");
                assert_eq!(
                    a.to_rgb8_with_threads(1),
                    b.to_rgb8_with_threads(threads),
                    "{what}: RGB"
                );
            }
            _ => panic!(
                "{what}: one thread {} but {threads} threads {}",
                one.is_some(),
                many.is_some()
            ),
        }
    }
}

/// Every restart marker's offset in `f` (the position of its 0xFF).
fn markers(f: &[u8]) -> Vec<usize> {
    (0..f.len() - 1)
        .filter(|&i| f[i] == 0xFF && (0xD0..=0xD7).contains(&f[i + 1]))
        .collect()
}

#[test]
fn intervals_decode_the_same_on_any_thread_count() {
    let (w, h) = (640usize, 488usize);
    let rgb = common::picture(w, h);
    let grey: Vec<u8> = rgb.chunks(3).map(|p| p[1]).collect();
    for (format, pixels) in [(PixelFormat::Rgb, &rgb), (PixelFormat::Luma, &grey)] {
        for sub in [Subsampling::S444, Subsampling::S420, Subsampling::S422] {
            for ri in [1u16, 7, 64, 2000] {
                let s = EncodeSettings {
                    quality: 88,
                    subsampling: sub,
                    restart_interval: ri,
                    threads: 1,
                    ..Default::default()
                };
                let f = jpeg::encode(pixels, w as u32, h as u32, format, &s).unwrap();
                let threaded = EncodeSettings {
                    threads: 0,
                    ..s.clone()
                };
                assert_eq!(
                    f,
                    jpeg::encode(pixels, w as u32, h as u32, format, &threaded).unwrap(),
                    "encoder bytes"
                );
                let what = format!("{format:?} {sub:?} ri={ri}");
                same(&f, &what);
                let img = decode(&f, 0).unwrap();
                assert!(
                    img.complete && img.warnings.is_empty(),
                    "{what}: {:?}",
                    img.warnings
                );

                let rst = markers(&f);
                if rst.len() < 3 {
                    continue;
                }
                let mid = rst[rst.len() / 2];
                // A marker missing: its two bytes cut out.
                let mut g = f.clone();
                g.drain(mid..mid + 2);
                same(&g, &format!("{what}, a marker missing"));
                // A marker misnumbered.
                let mut g = f.clone();
                g[mid + 1] = 0xD0 + (g[mid + 1] - 0xD0 + 3) % 8;
                same(&g, &format!("{what}, a marker misnumbered"));
                // Data before a marker.
                let mut g = f.clone();
                g.splice(mid..mid, [0x12, 0x34]);
                same(&g, &format!("{what}, data before a marker"));
                // A corrupt interval.
                let mut g = f.clone();
                for b in &mut g[rst[0] + 2..rst[0] + 12] {
                    *b = if *b == 0xFF {
                        0xFF
                    } else {
                        b.wrapping_mul(31) ^ 0x5A
                    };
                }
                same(&g, &format!("{what}, a corrupt interval"));
                // Truncated mid-scan, and just before the end of the scan.
                same(&f[..mid + 50], &format!("{what}, truncated"));
                same(&f[..f.len() - 4], &format!("{what}, the last bytes gone"));
            }
        }
    }
}
