//! Print what a JPEG file's headers say, decode it, and optionally write
//! the pixels as a binary PPM: `cargo run --example jpeginfo -- in.jpg [out.ppm]`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: jpeginfo <file.jpg> [out.ppm]");
        std::process::exit(2);
    };
    let data = std::fs::read(path).expect("read the file");
    match jpeg::decode(&data) {
        Ok(img) => {
            let i = &img.info;
            println!(
                "{path}: {}x{} {:?} {:?} {}-bit {:?} components={:?} restart={} scans={} complete={} orientation={:?} icc={:?} exif={:?}",
                i.width,
                i.height,
                i.process,
                i.coding,
                i.precision,
                i.colour_space,
                i.components
                    .iter()
                    .map(|c| (c.id, c.h, c.v))
                    .collect::<Vec<_>>(),
                i.restart_interval,
                i.scans,
                img.complete,
                i.orientation,
                i.icc_profile.as_ref().map(Vec::len),
                i.exif.as_ref().map(Vec::len),
            );
            for w in &img.warnings {
                println!("  warning: {w}");
            }
            if let Some(out) = args.get(1) {
                let mut f = format!("P6\n{} {}\n255\n", i.width, i.height).into_bytes();
                f.extend(img.to_rgb8());
                std::fs::write(out, f).expect("write the PPM");
            }
        }
        Err(e) => {
            println!("{path}: error: {e}");
            std::process::exit(1);
        }
    }
}
