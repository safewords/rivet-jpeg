//! Time encoding and decoding a synthetic 4000x3000 picture: `bench`.
use std::time::Instant;
fn main() {
    let (w, h) = (4000usize, 3000usize);
    let rgb: Vec<u8> = (0..w * h).flat_map(|i| { let (x, y) = (i % w, i / w); [(x / 16) as u8, (y / 12) as u8, ((x ^ y) & 255) as u8] }).collect();
    for progressive in [false, true] {
        let s = jpeg::EncodeSettings { quality: 85, progressive, ..Default::default() };
        let t = Instant::now();
        let f = jpeg::encode(&rgb, w as u32, h as u32, jpeg::PixelFormat::Rgb, &s).unwrap();
        let te = t.elapsed();
        let t = Instant::now();
        let img = jpeg::decode(&f).unwrap();
        let px = img.to_rgb8();
        println!("progressive={progressive}: {} bytes, encode {:?}, decode+RGB {:?} ({})", f.len(), te, t.elapsed(), px.len());
    }
}
