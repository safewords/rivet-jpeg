//! The encoder: round trips through this crate's strict decoder, the size
//! and PSNR at each quality, metadata, every picture size edge case.

mod common;

use jpeg::{EncodeSettings, PixelFormat, Subsampling};

/// The IJG test picture, a photograph, cropped by 3 columns and 5 lines
/// (to 224x144): the picture is itself a decoded JPEG, and re-encoding it
/// on its own block grid would nearly reproduce it at its own quality.
fn photo() -> (Vec<u8>, u32, u32) {
    let ppm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/corpus/ijg-testorig.ppm"
    ))
    .unwrap();
    let full = &ppm[ppm.len() - 227 * 149 * 3..];
    let mut out = Vec::with_capacity(224 * 144 * 3);
    for y in 5..149 {
        out.extend_from_slice(&full[(y * 227 + 3) * 3..(y * 227 + 227) * 3]);
    }
    (out, 224, 144)
}

#[test]
fn quality_sweep() {
    let (rgb, w, h) = photo();
    println!(
        "{:<8} {:>4} {:>12} {:>12} {:>12} {:>9}",
        "chroma", "q", "baseline", "optimised", "progressive", "PSNR dB"
    );
    for sub in [Subsampling::S444, Subsampling::S420] {
        let mut last_psnr = 0.0;
        let mut last_size = 0;
        for q in [5u8, 10, 25, 50, 75, 85, 90, 95, 100] {
            let plain = EncodeSettings {
                quality: q,
                subsampling: sub,
                optimize_huffman: false,
                ..Default::default()
            };
            let opt = EncodeSettings {
                optimize_huffman: true,
                ..plain.clone()
            };
            let prog = EncodeSettings {
                progressive: true,
                ..plain.clone()
            };
            let fa = jpeg::encode(&rgb, w, h, PixelFormat::Rgb, &plain).unwrap();
            let fb = jpeg::encode(&rgb, w, h, PixelFormat::Rgb, &opt).unwrap();
            let fc = jpeg::encode(&rgb, w, h, PixelFormat::Rgb, &prog).unwrap();
            let pa = common::strict(&fa).to_rgb8();
            assert_eq!(common::strict(&fb).to_rgb8(), pa);
            assert_eq!(common::strict(&fc).to_rgb8(), pa);
            let psnr = common::psnr(&rgb, &pa);
            println!(
                "{:<8} {q:>4} {:>12} {:>12} {:>12} {psnr:>9.2}",
                format!("{sub:?}"),
                fa.len(),
                fb.len(),
                fc.len()
            );
            assert!(psnr > last_psnr, "PSNR rises with quality");
            assert!(fa.len() > last_size, "size rises with quality");
            assert!(fb.len() <= fa.len(), "optimised tables are never larger");
            last_psnr = psnr;
            last_size = fa.len();
        }
        assert!(last_psnr > 40.0);
    }
}

#[test]
fn grey_and_rgba() {
    let (rgb, w, h) = photo();
    let grey: Vec<u8> = rgb
        .chunks(3)
        .map(|p| {
            ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u8
        })
        .collect();
    for progressive in [false, true] {
        let s = EncodeSettings {
            quality: 90,
            progressive,
            ..Default::default()
        };
        let f = jpeg::encode(&grey, w, h, PixelFormat::Luma, &s).unwrap();
        let img = common::strict(&f);
        assert_eq!(img.info.colour_space, jpeg::ColourSpace::Grey);
        let p = common::psnr(&grey, &img.to_luma8());
        println!(
            "grey progressive={progressive}: {} bytes, {p:.2} dB",
            f.len()
        );
        assert!(p > 38.0);
    }
    let rgba: Vec<u8> = rgb.chunks(3).flat_map(|p| [p[0], p[1], p[2], 7]).collect();
    let s = EncodeSettings::default();
    assert_eq!(
        jpeg::encode(&rgba, w, h, PixelFormat::Rgba, &s).unwrap(),
        jpeg::encode(&rgb, w, h, PixelFormat::Rgb, &s).unwrap(),
        "alpha is ignored"
    );
}

#[test]
fn metadata_round_trips() {
    let (rgb, w, h) = photo();
    // A profile needing three APP2 chunks, and EXIF with orientation 8.
    let icc: Vec<u8> = (0..200_000u32).map(|i| (i * 31 % 251) as u8).collect();
    let mut tiff = b"MM\0*\0\0\0\x08\0\x01".to_vec();
    tiff.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 8, 0, 0, 0, 0, 0, 0]);
    for progressive in [false, true] {
        let s = EncodeSettings {
            progressive,
            icc_profile: Some(icc.clone()),
            exif: Some([b"Exif\0\0".as_slice(), &tiff].concat()),
            ..Default::default()
        };
        let f = jpeg::encode(&rgb, w, h, PixelFormat::Rgb, &s).unwrap();
        let img = common::strict(&f);
        assert_eq!(img.info.icc_profile.as_deref(), Some(icc.as_slice()));
        assert_eq!(img.info.exif.as_deref(), Some(tiff.as_slice()));
        assert_eq!(img.info.orientation, Some(jpeg::Orientation::Rotate270));
        assert!(img.info.jfif.is_some());
        let info = jpeg::read_info(&f).unwrap();
        assert_eq!(info.icc_profile, img.info.icc_profile);
    }
    // Without the prefix, the same file.
    let a = jpeg::encode(
        &rgb,
        w,
        h,
        PixelFormat::Rgb,
        &EncodeSettings {
            exif: Some(tiff.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let b = jpeg::encode(
        &rgb,
        w,
        h,
        PixelFormat::Rgb,
        &EncodeSettings {
            exif: Some([b"Exif\0\0".as_slice(), &tiff].concat()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(a, b);
    let none = jpeg::encode(
        &rgb,
        w,
        h,
        PixelFormat::Rgb,
        &EncodeSettings {
            jfif: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(common::strict(&none).info.jfif.is_none());
}

#[test]
fn every_small_size() {
    for (w, h) in [
        (1, 1),
        (1, 17),
        (17, 1),
        (8, 8),
        (9, 9),
        (15, 16),
        (16, 15),
        (33, 7),
        (300, 2),
    ] {
        let rgb = common::picture(w, h);
        for sub in [
            Subsampling::S444,
            Subsampling::S422,
            Subsampling::S420,
            Subsampling::S440,
        ] {
            for (progressive, arithmetic, ri) in [
                (false, false, 0u16),
                (true, false, 1),
                (false, true, 2),
                (true, true, 0),
            ] {
                let s = EncodeSettings {
                    quality: 95,
                    subsampling: sub,
                    progressive,
                    arithmetic,
                    restart_interval: ri,
                    ..Default::default()
                };
                let f = jpeg::encode(&rgb, w as u32, h as u32, PixelFormat::Rgb, &s).unwrap();
                let img = common::strict(&f);
                assert_eq!((img.info.width, img.info.height), (w as u32, h as u32));
                assert_eq!(img.to_rgb8().len(), w * h * 3);
            }
        }
    }
}

#[test]
fn restart_intervals_appear_where_asked() {
    let (rgb, w, h) = photo();
    for ri in [1u16, 2, 7, 100] {
        let s = EncodeSettings {
            restart_interval: ri,
            subsampling: Subsampling::S420,
            ..Default::default()
        };
        let f = jpeg::encode(&rgb, w, h, PixelFormat::Rgb, &s).unwrap();
        let img = common::strict(&f);
        assert_eq!(img.info.restart_interval, ri);
        let mcus = (w as usize).div_ceil(16) * (h as usize).div_ceil(16);
        let markers = f
            .windows(2)
            .filter(|p| p[0] == 0xFF && (0xD0..=0xD7).contains(&p[1]))
            .count();
        assert_eq!(markers, (mcus - 1) / usize::from(ri), "interval {ri}");
    }
}

#[test]
fn settings_are_checked() {
    let rgb = vec![0u8; 12];
    let s = EncodeSettings::default();
    assert!(matches!(
        jpeg::encode(&rgb, 0, 4, PixelFormat::Rgb, &s),
        Err(jpeg::Error::Config(_))
    ));
    assert!(matches!(
        jpeg::encode(&rgb, 2, 3, PixelFormat::Rgb, &s),
        Err(jpeg::Error::Config(_))
    ));
    assert!(matches!(
        jpeg::encode(&rgb, 70000, 1, PixelFormat::Rgb, &s),
        Err(jpeg::Error::Config(_))
    ));
    let q0 = EncodeSettings {
        quality: 0,
        ..Default::default()
    };
    assert!(matches!(
        jpeg::encode(&rgb, 2, 2, PixelFormat::Rgb, &q0),
        Err(jpeg::Error::Config(_))
    ));
    let big = EncodeSettings {
        exif: Some(vec![0; 70_000]),
        ..Default::default()
    };
    assert!(matches!(
        jpeg::encode(&rgb, 2, 2, PixelFormat::Rgb, &big),
        Err(jpeg::Error::Config(_))
    ));
}
