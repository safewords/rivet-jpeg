//! The decoder against files from elsewhere (tests/corpus, see its README):
//! every file decodes without error, completely; where a reference
//! rendering exists, the pixels are compared with it; lossless files must
//! reproduce their source exactly.

use std::path::{Path, PathBuf};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
}

struct Ref {
    w: usize,
    h: usize,
    c: usize,
    bits: u32,
    samples: Vec<u16>,
}

fn read_ref(p: &Path) -> Ref {
    let d = std::fs::read(p).unwrap();
    let nl = d.iter().position(|&b| b == b'\n').unwrap();
    let hdr = std::str::from_utf8(&d[..nl]).unwrap();
    let f: Vec<&str> = hdr.split_whitespace().collect();
    assert_eq!(f[0], "REF");
    let (w, h, c, bits): (usize, usize, usize, u32) = (
        f[1].parse().unwrap(),
        f[2].parse().unwrap(),
        f[3].parse().unwrap(),
        f[4].parse().unwrap(),
    );
    let body = &d[nl + 1..];
    let samples: Vec<u16> = if bits <= 8 {
        body.iter().map(|&b| u16::from(b)).collect()
    } else {
        body.as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_be_bytes(*b))
            .collect()
    };
    assert_eq!(samples.len(), w * h * c, "{}", p.display());
    Ref {
        w,
        h,
        c,
        bits,
        samples,
    }
}

/// Max |diff|, mean |diff| and PSNR (against the full scale of `bits`).
fn compare(a: &[u16], b: &[u16], bits: u32) -> (u32, f64, f64) {
    assert_eq!(a.len(), b.len());
    let mut max = 0u32;
    let mut sum = 0f64;
    let mut sq = 0f64;
    for (&x, &y) in a.iter().zip(b) {
        let d = u32::from(x.abs_diff(y));
        max = max.max(d);
        sum += f64::from(d);
        sq += f64::from(d) * f64::from(d);
    }
    let n = a.len() as f64;
    let peak = f64::from((1u32 << bits) - 1);
    let psnr = if sq == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (peak * peak / (sq / n)).log10()
    };
    (max, sum / n, psnr)
}

/// The samples interleaved per pixel at the file's own precision: every
/// component for lossless (no colour conversion), RGB / grey / CMYK ink
/// otherwise, as the reference has them.
fn rendering(img: &jpeg::Image, r: &Ref, lossless: bool) -> Vec<u16> {
    let (w, h) = (img.info.width as usize, img.info.height as usize);
    if lossless {
        let mut out = vec![0u16; w * h * img.planes.len()];
        for (ci, p) in img.planes.iter().enumerate() {
            for y in 0..h {
                for x in 0..w {
                    out[(y * w + x) * img.planes.len() + ci] = p.row(y)[x];
                }
            }
        }
        return out;
    }
    match r.c {
        1 => {
            if r.bits <= 8 {
                img.to_luma8().into_iter().map(u16::from).collect()
            } else {
                img.planes[0]
                    .data
                    .chunks(img.planes[0].stride)
                    .take(h)
                    .flat_map(|row| row[..w].to_vec())
                    .collect()
            }
        }
        3 => {
            if r.bits <= 8 {
                img.to_rgb8().into_iter().map(u16::from).collect()
            } else {
                let max = (1u32 << r.bits) - 1;
                img.to_rgb16()
                    .into_iter()
                    .map(|v| ((u32::from(v) * max + 32767) / 65535) as u16)
                    .collect()
            }
        }
        4 => {
            // libjpeg returns CMYK as stored; Adobe stores it inverted.
            let inverted = img.info.adobe.is_some();
            img.to_cmyk8()
                .unwrap()
                .into_iter()
                .map(|v| u16::from(if inverted { 255 - v } else { v }))
                .collect()
        }
        _ => unreachable!(),
    }
}

#[test]
fn generated_corpus_matches_its_references() {
    let dir = corpus().join("gen");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".jpg"))
        .collect();
    names.sort();
    assert!(names.len() >= 30, "{} files", names.len());
    println!(
        "{:<34} {:>5} {:>6} {:>8} {:>8}",
        "file", "max", "mean", "PSNR dB", "verdict"
    );
    let mut failures = Vec::new();
    for name in &names {
        let data = std::fs::read(dir.join(name)).unwrap();
        let img = jpeg::decode(&data).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            img.complete && img.warnings.is_empty(),
            "{name}: {:?}",
            img.warnings
        );
        let r = read_ref(&dir.join(name.replace(".jpg", ".ref")));
        assert_eq!(
            (r.w, r.h),
            (img.info.width as usize, img.info.height as usize),
            "{name}"
        );
        let lossless = img.info.process == jpeg::Process::Lossless;
        let mine = rendering(&img, &r, lossless);
        let (max, mean, psnr) = compare(&mine, &r.samples, r.bits);
        // Lossless must be exact. The DCT processes: T.81 fixes the
        // reconstruction only to IDCT accuracy, and leaves upsampling and
        // colour conversion to the application, so a different decoder may
        // differ by a few levels after them; within 2 levels per stage.
        let limit = if lossless {
            0
        } else if r.bits > 8 {
            3 << (r.bits - 8)
        } else {
            match r.c {
                1 => 1,
                _ => 4,
            }
        };
        // At a 4:1 sampling ratio this crate interpolates chroma where the
        // reference evidently replicates it (luma agreed within 1 when the
        // planes were compared directly); the RGB can then differ by tens
        // of levels at chroma edges, so that case is held to PSNR alone.
        let odd_ratio = img.info.components.iter().any(|c| {
            let hmax = img.info.components.iter().map(|c| c.h).max().unwrap();
            let vmax = img.info.components.iter().map(|c| c.v).max().unwrap();
            !matches!(hmax / c.h, 1 | 2) || !matches!(vmax / c.v, 1 | 2)
        });
        let ok = if odd_ratio { psnr > 35.0 } else { max <= limit };
        println!(
            "{name:<34} {max:>5} {mean:>6.3} {:>8} {:>8}",
            if psnr.is_finite() {
                format!("{psnr:.2}")
            } else {
                "exact".into()
            },
            if ok { "ok" } else { "FAIL" }
        );
        if !ok {
            failures.push(name.clone());
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn ijg_test_images() {
    let dir = corpus();
    let decode = |n: &str| {
        let img = jpeg::decode(&std::fs::read(dir.join(n)).unwrap()).unwrap();
        assert!(
            img.complete && img.warnings.is_empty(),
            "{n}: {:?}",
            img.warnings
        );
        img
    };
    let orig = decode("ijg-testorig.jpg");
    let ari = decode("ijg-testimgari.jpg");
    let int = decode("ijg-testimgint.jpg");
    assert_eq!(ari.info.coding, jpeg::Coding::Arithmetic);
    // The arithmetic-coded and Huffman-coded IJG files carry the same
    // coefficients: their pixels agree exactly.
    assert_eq!(ari.to_rgb8(), int.to_rgb8());
    // testorig.ppm is the IJG decoder's rendering of testorig.jpg.
    let ppm = std::fs::read(dir.join("ijg-testorig.ppm")).unwrap();
    let body = &ppm[ppm.len() - 227 * 149 * 3..];
    let a: Vec<u16> = orig.to_rgb8().into_iter().map(u16::from).collect();
    let b: Vec<u16> = body.iter().map(|&v| u16::from(v)).collect();
    let (max, mean, psnr) = compare(&a, &b, 8);
    println!("ijg-testorig.jpg against the IJG rendering: max {max}, mean {mean:.3}, {psnr:.2} dB");
    assert!(max <= 4 && psnr > 45.0);
    let m12 = decode("ijg-monkey12.jpg");
    assert_eq!(m12.info.precision, 12);
    assert_eq!(m12.info.icc_profile.as_ref().map(Vec::len), Some(3024));
}

#[test]
fn camera_files() {
    let dir = corpus();
    for (n, process, precision) in [
        ("dng-canon-5d3-lossless14.jpg", jpeg::Process::Lossless, 14),
        (
            "dng-blackmagic-ext12.jpg",
            jpeg::Process::ExtendedSequential,
            12,
        ),
        ("dng-adobe-lossy-tile.jpg", jpeg::Process::Baseline, 8),
    ] {
        let data = std::fs::read(dir.join(n)).unwrap();
        let img = jpeg::decode(&data).unwrap();
        assert!(img.complete, "{n}");
        // Adobe's DNG writer uses an all-1s Huffman code, which T.81
        // reserves; nothing else may be reported.
        assert!(
            img.warnings.iter().all(|w| w.contains("all-1s")),
            "{n}: {:?}",
            img.warnings
        );
        assert_eq!(
            (img.info.process, img.info.precision),
            (process, precision),
            "{n}"
        );
    }
}
