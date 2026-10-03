mod common;
use jpeg::{EncodeSettings, PixelFormat, Subsampling};

#[test]
fn smoke() {
    let (w, h) = (97, 61);
    let rgb = common::picture(w, h);
    for sub in [Subsampling::S444, Subsampling::S422, Subsampling::S420, Subsampling::S440] {
        let base = EncodeSettings { quality: 90, subsampling: sub, optimize_huffman: false, ..Default::default() };
        let f0 = jpeg::encode(&rgb, w as u32, h as u32, PixelFormat::Rgb, &base).unwrap();
        let ref_px = common::strict(&f0).to_rgb8();
        for progressive in [false, true] {
            for arithmetic in [false, true] {
                for ri in [0u16, 3] {
                    let s = EncodeSettings { progressive, arithmetic, restart_interval: ri, optimize_huffman: true, ..base.clone() };
                    let f = jpeg::encode(&rgb, w as u32, h as u32, PixelFormat::Rgb, &s).unwrap();
                    let img = common::strict(&f);
                    let px = img.to_rgb8();
                    let same = px == ref_px;
                    println!("{sub:?} prog={progressive} arith={arithmetic} ri={ri}: {} bytes, {:.2} dB, identical={same}", f.len(), common::psnr(&rgb, &px));
                    assert!(same);
                }
            }
        }
    }
}
