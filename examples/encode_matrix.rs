//! Encode a picture (a binary PPM) in every mode the encoder has, and write
//! each file with this crate's own decode of it beside it
//! (`<name>.jpg`, `<name>.rgb`): `encode_matrix in.ppm out_dir`. Used by
//! `tools/check_encoder.py` to have another decoder read the same files.

use jpeg::{EncodeSettings, PixelFormat, Subsampling};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let ppm = std::fs::read(&a[1]).expect("read the PPM");
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while ppm[i].is_ascii_whitespace() {
            i += 1;
        }
        let s = i;
        while !ppm[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(String::from_utf8_lossy(&ppm[s..i]).to_string());
    }
    let (w, h): (u32, u32) = (fields[1].parse().unwrap(), fields[2].parse().unwrap());
    let rgb = &ppm[i + 1..i + 1 + (w * h * 3) as usize];
    let grey: Vec<u8> = rgb.chunks(3).map(|p| ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u8).collect();
    let out = std::path::Path::new(&a[2]);
    std::fs::create_dir_all(out).unwrap();
    let icc: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
    let subs = [("444", Subsampling::S444), ("422", Subsampling::S422), ("420", Subsampling::S420), ("440", Subsampling::S440)];
    for (sn, sub) in subs {
        for (mode, progressive, arithmetic, optimise) in [
            ("baseline-std", false, false, false),
            ("baseline-opt", false, false, true),
            ("progressive", true, false, true),
            ("arith", false, true, false),
            ("arith-progressive", true, true, false),
        ] {
            for q in [30u8, 75, 95] {
                for ri in [0u16, 5] {
                    let s = EncodeSettings {
                        quality: q,
                        subsampling: sub,
                        progressive,
                        arithmetic,
                        optimize_huffman: optimise,
                        restart_interval: ri,
                        icc_profile: (q == 75).then(|| icc.clone()),
                        ..Default::default()
                    };
                    let f = jpeg::encode(rgb, w, h, PixelFormat::Rgb, &s).unwrap();
                    let name = format!("{mode}-{sn}-q{q}-ri{ri}");
                    std::fs::write(out.join(format!("{name}.jpg")), &f).unwrap();
                    std::fs::write(out.join(format!("{name}.rgb")), jpeg::decode(&f).unwrap().to_rgb8()).unwrap();
                }
            }
        }
    }
    for progressive in [false, true] {
        let s = EncodeSettings { quality: 85, progressive, ..Default::default() };
        let f = jpeg::encode(&grey, w, h, PixelFormat::Luma, &s).unwrap();
        let name = format!("grey-{}", if progressive { "progressive" } else { "baseline" });
        std::fs::write(out.join(format!("{name}.jpg")), &f).unwrap();
        std::fs::write(out.join(format!("{name}.rgb")), jpeg::decode(&f).unwrap().to_rgb8()).unwrap();
    }
}
