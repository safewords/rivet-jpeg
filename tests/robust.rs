//! Malformed input: truncation at every length, corrupted bytes, broken
//! syntax. The decoder never panics; a file that ends early decodes as far
//! as it goes; strict mode turns every departure into an error.

mod common;

use jpeg::{DecodeOptions, EncodeSettings, PixelFormat, Subsampling};

fn files() -> Vec<(&'static str, Vec<u8>)> {
    let (w, h) = (61, 45);
    let rgb = common::picture(w, h);
    let enc = |s: EncodeSettings| jpeg::encode(&rgb, w as u32, h as u32, PixelFormat::Rgb, &s).unwrap();
    let corpus = |n: &str| std::fs::read(format!("{}/tests/corpus/{n}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    vec![
        ("baseline", enc(EncodeSettings { optimize_huffman: false, ..Default::default() })),
        ("progressive", enc(EncodeSettings { progressive: true, ..Default::default() })),
        ("restarts", enc(EncodeSettings { restart_interval: 2, subsampling: Subsampling::S444, ..Default::default() })),
        ("arithmetic", enc(EncodeSettings { arithmetic: true, ..Default::default() })),
        ("arithmetic progressive", enc(EncodeSettings { arithmetic: true, progressive: true, restart_interval: 3, ..Default::default() })),
        ("lossless", corpus("gen/lossless8-p4.jpg")),
        ("12-bit", corpus("gen/ext12-colour.jpg")),
    ]
}

#[test]
fn truncated_at_every_length() {
    for (name, f) in files() {
        let full = jpeg::decode(&f).unwrap().to_rgb8();
        let first_scan = f.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        let step = (f.len() / 300).max(1);
        let mut partial_psnr = Vec::new();
        for len in (0..f.len()).step_by(step) {
            match jpeg::decode(&f[..len]) {
                Ok(img) => {
                    assert!(len > first_scan, "{name}: {len} bytes gave a picture before any scan");
                    assert!(!img.complete || len >= f.len() - 2, "{name} at {len}: complete");
                    let px = img.to_rgb8();
                    assert_eq!(px.len(), full.len());
                    if len > f.len() * 9 / 10 {
                        partial_psnr.push(common::psnr(&full, &px));
                    }
                }
                Err(e @ (jpeg::Error::Truncated(_) | jpeg::Error::Invalid(_))) => {
                    assert!(len <= first_scan + 16, "{name}: {len} bytes refused: {e:?}");
                }
                Err(e) => panic!("{name} at {len}: {e}"),
            }
        }
        // Most of the file gives most of the picture.
        let best = partial_psnr.iter().cloned().fold(0.0, f64::max);
        println!("{name}: {} bytes, best PSNR of the last tenth against the whole: {best:.1} dB", f.len());
        assert!(best > 20.0, "{name}");
        // Strict mode refuses a truncated file.
        let strict = DecodeOptions { strict: true, ..Default::default() };
        assert!(jpeg::decode_with(&f[..f.len() - 10], &strict).is_err(), "{name}");
    }
}

#[test]
fn corrupted_bytes_never_panic() {
    let mut seed = 0x9E37_79B9u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut outcomes = [0usize; 3];
    for (_, f) in files() {
        let iters: usize = std::env::var("JPEG_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(400);
        for _ in 0..iters {
            let mut g = f.clone();
            for _ in 0..(1 + rand() % 4) {
                let at = rand() as usize % g.len();
                match rand() % 3 {
                    0 => g[at] = rand() as u8,
                    1 => g[at] ^= 1 << (rand() % 8),
                    _ => {
                        let n = (rand() as usize % 16).min(g.len() - at);
                        g.drain(at..at + n);
                    }
                }
            }
            match jpeg::decode(&g) {
                Ok(img) => {
                    outcomes[usize::from(!img.complete)] += 1;
                    let _ = img.to_rgb8();
                }
                Err(_) => outcomes[2] += 1,
            }
        }
    }
    println!("complete {}, partial {}, refused {}", outcomes[0], outcomes[1], outcomes[2]);
}

#[test]
fn not_jpeg() {
    for d in [&b""[..], b"\xFF", b"\xFF\xD8", b"\xFF\xD8\xFF\xD9", b"\x89PNG\r\n\x1a\n", b"\xFF\xD8\xFF\xDA\x00\x08\x01\x01\x00\x00\x3F\x00"] {
        assert!(jpeg::decode(d).is_err(), "{d:?}");
    }
}

#[test]
fn strict_mode_rejects_what_lenient_mode_steps_over() {
    let rgb = common::picture(32, 32);
    let f = jpeg::encode(&rgb, 32, 32, PixelFormat::Rgb, &EncodeSettings { restart_interval: 1, ..Default::default() }).unwrap();
    let strict = DecodeOptions { strict: true, ..Default::default() };
    let sos = f.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    // Garbage between two marker segments.
    let mut g = f[..sos].to_vec();
    g.extend_from_slice(b"junk");
    g.extend_from_slice(&f[sos..]);
    assert!(jpeg::decode_with(&g, &strict).is_err());
    let img = jpeg::decode(&g).unwrap();
    assert!(img.complete && !img.warnings.is_empty());
    // A restart marker misnumbered: RST0 becomes RST3.
    let rst = f.iter().enumerate().position(|(i, &b)| i > sos && b == 0xFF && f[i + 1] == 0xD0).unwrap();
    let mut g = f.clone();
    g[rst + 1] = 0xD3;
    assert!(jpeg::decode_with(&g, &strict).is_err());
    let img = jpeg::decode(&g).unwrap();
    assert!(img.warnings.iter().any(|w| w.contains("RST")), "{:?}", img.warnings);
    // Bytes after EOI.
    let mut g = f.clone();
    g.extend_from_slice(b"trailer");
    assert!(jpeg::decode_with(&g, &strict).is_err());
    assert!(jpeg::decode(&g).unwrap().complete);
    // No EOI.
    assert!(jpeg::decode_with(&f[..f.len() - 2], &strict).is_err());
    assert!(jpeg::decode(&f[..f.len() - 2]).unwrap().complete);
}

/// A restart interval that is lost entirely (its bytes and its marker cut
/// out) is skipped by the marker number, and the rest decodes in place.
#[test]
fn lost_restart_interval_resynchronises() {
    let rgb = common::picture(64, 16);
    let f = jpeg::encode(&rgb, 64, 16, PixelFormat::Rgb, &EncodeSettings { restart_interval: 1, subsampling: Subsampling::S444, ..Default::default() }).unwrap();
    let full = jpeg::decode(&f).unwrap().to_rgb8();
    let marks: Vec<usize> = (0..f.len() - 1).filter(|&i| f[i] == 0xFF && (0xD0..=0xD7).contains(&f[i + 1])).collect();
    // Remove the data after RST1 up to and including RST2: interval 2 lost.
    let mut g = f[..marks[1] + 2].to_vec();
    g.extend_from_slice(&f[marks[2] + 2..]);
    let img = jpeg::decode(&g).unwrap();
    assert!(img.warnings.iter().any(|w| w.contains("lost")), "{:?}", img.warnings);
    let px = img.to_rgb8();
    // Columns 24..32 (MCU 3, lost) differ; the MCUs after it are in place.
    let col = |p: &[u8], x0: usize, x1: usize| -> Vec<u8> {
        (0..16).flat_map(|y| p[(y * 64 + x0) * 3..(y * 64 + x1) * 3].to_vec()).collect()
    };
    assert_eq!(col(&px, 32, 64), col(&full, 32, 64));
    assert_eq!(col(&px, 0, 16), col(&full, 0, 16));
}

#[test]
fn dnl_gives_the_height() {
    let rgb = common::picture(40, 24);
    let f = jpeg::encode(&rgb, 40, 24, PixelFormat::Rgb, &EncodeSettings { jfif: false, ..Default::default() }).unwrap();
    let sof = f.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    let mut g = f.clone();
    g[sof + 5] = 0;
    g[sof + 6] = 0;
    // DNL after the scan, before EOI.
    g.truncate(g.len() - 2);
    g.extend_from_slice(&[0xFF, 0xDC, 0, 4, 0, 24, 0xFF, 0xD9]);
    let img = jpeg::decode(&g).unwrap();
    assert_eq!(img.info.height, 24);
    assert_eq!(img.to_rgb8(), jpeg::decode(&f).unwrap().to_rgb8());
}

#[test]
fn hierarchical_is_refused_by_name() {
    let rgb = common::picture(16, 16);
    let f = jpeg::encode(&rgb, 16, 16, PixelFormat::Rgb, &EncodeSettings::default()).unwrap();
    let sof = f.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    let mut g = f.clone();
    g[sof + 1] = 0xC5;
    assert!(matches!(jpeg::decode(&g), Err(jpeg::Error::Unsupported(_))));
}

#[test]
fn pixel_limit() {
    let rgb = common::picture(100, 100);
    let f = jpeg::encode(&rgb, 100, 100, PixelFormat::Rgb, &EncodeSettings::default()).unwrap();
    let opts = DecodeOptions { max_pixels: Some(9_999), ..Default::default() };
    assert!(matches!(jpeg::decode_with(&f, &opts), Err(jpeg::Error::TooLarge { .. })));
    let opts = DecodeOptions { max_pixels: Some(10_000), ..Default::default() };
    assert!(jpeg::decode_with(&f, &opts).is_ok());
    // The headers alone are read without a limit.
    assert_eq!(jpeg::read_info(&f).unwrap().width, 100);
}
